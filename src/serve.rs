use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Value, json};

use crate::commands::{self, RefreshScope};
use crate::gauge;
use crate::ops;
use crate::paths;
use crate::pkce;
use crate::provider::{PendingLogin, Provider};
use crate::runs;
use crate::store::Store;

const PAGE: &str = include_str!("serve.html");
const ICON: &str = include_str!("../assets/remuda.svg");

/// `login` is taken out while its `finish` runs; `cancel` stays behind so a
/// cancel can still reach it.
struct Pending {
    provider: &'static str,
    name: String,
    force: bool,
    login: Option<Box<dyn PendingLogin>>,
    cancel: Box<dyn Fn() + Send + Sync>,
}

pub struct Places {
    pub units: Vec<PathBuf>,
    pub snapshot: PathBuf,
}

impl Places {
    pub fn from_env() -> Self {
        Places {
            units: paths::systemd_user_units(),
            snapshot: paths::tokengauge_snapshot(),
        }
    }
}

pub struct App {
    store: Store,
    places: Places,
    providers: Vec<Box<dyn Provider>>,
    token: String,
    port: u16,
    open: Opener,
    pending: Mutex<HashMap<String, Pending>>,
}

/// Opens a sign-in page in a private window; false leaves it to the page.
pub type Opener = Box<dyn Fn(&str) -> bool + Send + Sync>;

pub struct Request<'a> {
    pub method: &'a str,
    pub path: &'a str,
    pub host: Option<&'a str>,
    pub origin: Option<&'a str>,
    pub token: Option<&'a str>,
    pub body: &'a str,
}

#[derive(Debug)]
pub struct Response {
    pub status: u16,
    pub content_type: &'static str,
    pub body: String,
}

impl Response {
    pub(crate) fn json(status: u16, body: Value) -> Self {
        Response {
            status,
            content_type: "application/json",
            body: body.to_string(),
        }
    }
}

fn capture(run: impl FnOnce(&mut dyn Write) -> Result<()>) -> Result<Value> {
    let mut out = Vec::new();
    run(&mut out)?;
    Ok(json!({"message": String::from_utf8_lossy(&out).trim()}))
}

impl App {
    pub fn new(
        store: Store,
        places: Places,
        providers: Vec<Box<dyn Provider>>,
        token: String,
        port: u16,
        open: Opener,
    ) -> Self {
        App {
            store,
            places,
            providers,
            token,
            port,
            open,
            pending: Mutex::new(HashMap::new()),
        }
    }

    fn providers(&self) -> Vec<&dyn Provider> {
        self.providers.iter().map(|p| p.as_ref()).collect()
    }

    fn health(&self) -> Value {
        let unit = "remuda-refresh.timer";
        let dirs = &self.places.units;
        let found = dirs
            .iter()
            .map(|d| d.join(unit))
            .find(|p| p.symlink_metadata().is_ok());
        let masked = found
            .as_ref()
            .is_some_and(|p| std::fs::canonicalize(p).is_ok_and(|t| t == Path::new("/dev/null")));
        let wanted = dirs
            .iter()
            .any(|d| d.join("timers.target.wants").join(unit).metadata().is_ok());
        let timer = match (found, masked, wanted) {
            (_, true, _) => "masked",
            (Some(_), false, true) => "enabled",
            (Some(_), false, false) => "disabled",
            (None, ..) => "absent",
        };
        json!({
            "timer": timer,
            "lastRefresh": runs::last(&self.store),
            "tokengauge": self.places.snapshot.exists(),
            "switchedAt": runs::switches(&self.store),
        })
    }

    /// A page on another site can reach 127.0.0.1 through the user's browser,
    /// and a rebound DNS name can reach it with its own Host. Only a request
    /// that names this listener, and carries the token from the URL the user
    /// was given, is served.
    fn is_local(&self, host: &str) -> bool {
        host == format!("127.0.0.1:{}", self.port) || host == format!("localhost:{}", self.port)
    }

    /// The refusal for a request that may not be served, decided from its head
    /// alone so no body is read first.
    pub fn check(&self, req: &Request) -> Option<Response> {
        if !req.host.is_some_and(|h| self.is_local(h)) {
            return Some(Response::json(403, json!({"error": "not a local request"})));
        }
        if let Some(origin) = req.origin
            && !origin
                .strip_prefix("http://")
                .is_some_and(|h| self.is_local(h))
        {
            return Some(Response::json(
                403,
                json!({"error": "cross-origin request refused"}),
            ));
        }
        if req.path.starts_with("/api/") && req.token != Some(self.token.as_str()) {
            return Some(Response::json(
                401,
                json!({"error": "missing or wrong token"}),
            ));
        }
        None
    }

    pub fn handle(&self, req: &Request) -> Response {
        if let Some(refused) = self.check(req) {
            return refused;
        }
        match (req.method, req.path) {
            ("GET", "/") => Response {
                status: 200,
                content_type: "text/html; charset=utf-8",
                body: PAGE.to_owned(),
            },
            ("GET", "/favicon.svg") => Response {
                status: 200,
                content_type: "image/svg+xml",
                body: ICON.to_owned(),
            },
            (_, path) if path.starts_with("/api/") => match self.api(req.method, path, req.body) {
                Ok(v) => Response::json(200, v),
                Err(e) => Response::json(400, json!({"error": format!("{e:#}")})),
            },
            _ => Response::json(404, json!({"error": "not found"})),
        }
    }

    fn api(&self, method: &str, path: &str, body: &str) -> Result<Value> {
        let body: Value = if body.trim().is_empty() {
            json!({})
        } else {
            serde_json::from_str(body).context("the request body is not JSON")?
        };
        let text = |k: &str| body.get(k).and_then(Value::as_str);
        let flag = |k: &str| body.get(k).and_then(Value::as_bool).unwrap_or(false);
        let spec = || text("name").context("name is required");
        let providers = self.providers();
        let store = &self.store;
        // A sign-in waits on the browser, so only it runs without the lock.
        let _lock = if path.starts_with("/api/login") {
            None
        } else {
            Some(store.lock()?)
        };

        match (method, path) {
            ("GET", "/api/state") => {
                let lives = commands::sync_all(store, &providers)?;
                let mut state = commands::list_json(store, &lives)?;
                state["providers"] = providers
                    .iter()
                    .map(|p| json!({"id": p.id(), "name": p.name()}))
                    .collect();
                state["health"] = self.health();
                state["usage"] = gauge::read(&self.places.snapshot).unwrap_or(Value::Null);
                Ok(state)
            }
            ("POST", "/api/use") => {
                let (p, name) = commands::resolve(store, &providers, spec()?)?;
                Ok(json!({"message": ops::switch(store, p, &name, flag("discard"))?}))
            }
            ("POST", "/api/import") => {
                let p = commands::find(
                    &providers,
                    text("provider").unwrap_or(commands::DEFAULT_PROVIDER),
                )?;
                let state = ops::sync_live(store, p)?;
                capture(|out| commands::import(store, p, &state, spec()?, flag("force"), out))
            }
            ("POST", "/api/label") => {
                let (p, name) = commands::resolve(store, &providers, spec()?)?;
                capture(|out| commands::label(store, p, &name, text("text"), out))
            }
            ("POST", "/api/rename") => {
                let (p, name) = commands::resolve(store, &providers, spec()?)?;
                let to = text("to").context("to is required")?;
                capture(|out| commands::rename(store, p, &name, to, out))
            }
            ("POST", "/api/remove") => {
                let (p, name) = commands::resolve(store, &providers, spec()?)?;
                let state = ops::sync_live(store, p)?;
                capture(|out| commands::remove(store, p, &state, &name, out))
            }
            ("POST", "/api/refresh") => {
                let only = text("name")
                    .map(|s| commands::resolve(store, &providers, s))
                    .transpose()?;
                let lives = commands::sync_all(store, &providers)?;
                let scope = RefreshScope {
                    only: only.as_ref().map(|(p, name)| (*p, name.as_str())),
                    force: flag("force"),
                    within_min: 60,
                };
                let (mut out, mut err) = (Vec::new(), Vec::new());
                let result = commands::refresh(
                    store,
                    &lives,
                    scope,
                    &mut out,
                    &mut err,
                    &mut commands::Report::default(),
                );
                let err = String::from_utf8_lossy(&err).trim().to_owned();
                if let Err(e) = result {
                    bail!("{e}: {err}");
                }
                let out = String::from_utf8_lossy(&out).trim().to_owned();
                Ok(
                    json!({"message": if out.is_empty() { "nothing was due".to_owned() } else { out }}),
                )
            }
            ("POST", "/api/login") => {
                let p = commands::find(
                    &providers,
                    text("provider").unwrap_or(commands::DEFAULT_PROVIDER),
                )?;
                let name = spec()?;
                let force = flag("force");
                let mut pending = self.pending.lock().map_err(|_| anyhow!("poisoned"))?;
                pending.retain(|_, l| {
                    let keep = l.provider != p.id();
                    if !keep {
                        (l.cancel)();
                    }
                    keep
                });
                let login = commands::begin_login(store, p, name, force)?;
                let id = pkce::random()?;
                let url = login.url().to_owned();
                let needs_code = login.needs_code();
                pending.insert(
                    id.clone(),
                    Pending {
                        provider: p.id(),
                        name: name.to_owned(),
                        force,
                        cancel: login.canceller(),
                        login: Some(login),
                    },
                );
                drop(pending);
                let opened = (self.open)(&url);
                Ok(json!({"id": id, "url": url, "needsCode": needs_code, "opened": opened}))
            }
            ("POST", "/api/login/finish") => {
                let id = text("id").context("id is required")?;
                let (provider, name, force, login) = {
                    let mut all = self.pending.lock().map_err(|_| anyhow!("poisoned"))?;
                    let pending = all
                        .get_mut(id)
                        .context("that sign-in is no longer open; start it again")?;
                    let login = pending
                        .login
                        .take()
                        .context("that sign-in is already finishing")?;
                    (pending.provider, pending.name.clone(), pending.force, login)
                };
                let done = login.finish(text("code"));
                if let Ok(mut all) = self.pending.lock() {
                    all.remove(id);
                }
                let p = commands::find(&providers, provider)?;
                let entry = commands::save_login(store, p, &name, force, done?)?;
                Ok(json!({"message": commands::stored_message(p, &entry)}))
            }
            ("POST", "/api/login/cancel") => {
                if let Some(id) = text("id")
                    && let Some(pending) = self
                        .pending
                        .lock()
                        .map_err(|_| anyhow!("poisoned"))?
                        .remove(id)
                {
                    (pending.cancel)();
                }
                Ok(json!({}))
            }
            _ => bail!("no such endpoint: {method} {path}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::claude::{Api, Claude};
    use crate::ops::testing::*;
    use std::sync::Arc;

    const PORT: u16 = 7429;
    const TOKEN: &str = "t0ken";

    fn places(e: &Env) -> Places {
        Places {
            units: vec![e.tmp.path().join("units"), e.tmp.path().join("global")],
            snapshot: e.tmp.path().join("tokengauge-usage.json"),
        }
    }

    fn app(e: &Env, api_base: &str) -> App {
        app_opening(e, api_base, Box::new(|_| false))
    }

    fn app_opening(e: &Env, api_base: &str, open: Opener) -> App {
        let providers: Vec<Box<dyn Provider>> =
            vec![Box::new(Claude::at(e.tmp.path(), Api::local(api_base)))];
        App::new(
            Store::open(e.store.root()),
            places(e),
            providers,
            TOKEN.into(),
            PORT,
            open,
        )
    }

    fn call(app: &App, method: &str, path: &str, body: Value) -> (u16, Value) {
        let body = body.to_string();
        let resp = app.handle(&Request {
            method,
            path,
            host: Some("127.0.0.1:7429"),
            origin: Some("http://127.0.0.1:7429"),
            token: Some(TOKEN),
            body: &body,
        });
        (
            resp.status,
            serde_json::from_str(&resp.body).unwrap_or(Value::Null),
        )
    }

    #[test]
    fn only_a_local_request_carrying_the_token_is_served() {
        let e = env(&OFFLINE);
        let app = app(&e, &OFFLINE);
        let status = |host: Option<&str>, origin: Option<&str>, token: Option<&str>, path: &str| {
            app.handle(&Request {
                method: "GET",
                path,
                host,
                origin,
                token,
                body: "",
            })
            .status
        };
        let local = Some("localhost:7429");
        assert_eq!(status(local, None, None, "/"), 200);
        assert_eq!(status(local, None, None, "/api/state"), 401);
        assert_eq!(status(local, None, Some("nope"), "/api/state"), 401);
        assert_eq!(status(local, None, Some(TOKEN), "/api/state"), 200);
        assert_eq!(
            status(
                Some("rebound.example:7429"),
                None,
                Some(TOKEN),
                "/api/state"
            ),
            403
        );
        assert_eq!(
            status(Some("127.0.0.1:1"), None, Some(TOKEN), "/api/state"),
            403
        );
        assert_eq!(status(None, None, Some(TOKEN), "/api/state"), 403);
        assert_eq!(
            status(
                local,
                Some("https://evil.example"),
                Some(TOKEN),
                "/api/state"
            ),
            403
        );
        assert_eq!(status(local, None, Some(TOKEN), "/elsewhere"), 404);
    }

    #[test]
    fn the_page_is_self_contained() {
        let e = env(&OFFLINE);
        let app = app(&e, &OFFLINE);
        let get = |path| {
            app.handle(&Request {
                method: "GET",
                path,
                host: Some("127.0.0.1:7429"),
                origin: None,
                token: None,
                body: "",
            })
        };
        let resp = get("/");
        assert!(resp.content_type.starts_with("text/html"));
        assert!(resp.body.contains("X-Remuda-Token"));
        assert!(!resp.body.contains("<script src"));
        assert_eq!(resp.body.matches("<link").count(), 1);
        assert!(
            resp.body
                .contains(r#"<link rel="icon" href="/favicon.svg""#)
        );

        let icon = get("/favicon.svg");
        assert_eq!((icon.status, icon.content_type), (200, "image/svg+xml"));
        assert!(icon.body.starts_with("<svg"));
    }

    #[test]
    fn the_state_reports_the_timer_its_last_run_and_tokengauge_usage() {
        let e = env(&OFFLINE);
        let app = app(&e, &OFFLINE);
        let (_, state) = call(&app, "GET", "/api/state", Value::Null);
        assert_eq!(state["health"]["timer"], "absent");
        assert_eq!(state["health"]["lastRefresh"], Value::Null);
        assert_eq!(state["health"]["tokengauge"], false);
        assert_eq!(state["usage"], Value::Null);

        let units = e.tmp.path().join("units");
        let global = e.tmp.path().join("global");
        std::fs::create_dir_all(global.join("timers.target.wants")).unwrap();
        std::fs::create_dir_all(&units).unwrap();
        std::fs::write(units.join("remuda-refresh.timer"), "").unwrap();
        let (_, state) = call(&app, "GET", "/api/state", Value::Null);
        assert_eq!(state["health"]["timer"], "disabled");
        std::os::unix::fs::symlink(
            units.join("remuda-refresh.timer"),
            global.join("timers.target.wants/remuda-refresh.timer"),
        )
        .unwrap();
        runs::switched(&e.store, "claude", 9);
        runs::record(
            &e.store,
            &runs::Run {
                at: 7,
                refreshed: vec!["claude/work".into()],
                problems: vec![],
            },
        );
        std::fs::write(
            e.tmp.path().join("tokengauge-usage.json"),
            json!({"payloads": [{"provider": "claude", "usage": {
                "primary": {"usedPercent": 40, "windowMinutes": 300}
            }}]})
            .to_string(),
        )
        .unwrap();
        let (_, state) = call(&app, "GET", "/api/state", Value::Null);
        assert_eq!(state["health"]["timer"], "enabled");
        assert_eq!(state["health"]["lastRefresh"]["at"], 7);
        assert_eq!(state["health"]["tokengauge"], true);
        assert_eq!(state["health"]["switchedAt"]["claude"], 9);
        assert_eq!(
            state["usage"]["providers"]["claude"]["windows"][0]["usedPercent"],
            40
        );

        let timer = || call(&app, "GET", "/api/state", Value::Null).1["health"]["timer"].clone();
        std::fs::remove_file(units.join("remuda-refresh.timer")).unwrap();
        std::os::unix::fs::symlink("/dev/null", units.join("remuda-refresh.timer")).unwrap();
        assert_eq!(timer(), "masked");
        std::fs::remove_file(units.join("remuda-refresh.timer")).unwrap();
        assert_eq!(timer(), "absent");
        std::fs::write(global.join("remuda-refresh.timer"), "").unwrap();
        assert_eq!(timer(), "disabled");
    }

    #[test]
    fn credentials_are_listed_switched_labelled_renamed_and_removed() {
        let e = env(&OFFLINE);
        e.stored("work", "u-work", HOUR);
        e.stored("perso", "u-perso", HOUR);
        let work = e.store.get("claude", "work").unwrap().unwrap();
        e.sign_in(work.creds, account("u-work"));
        let app = app(&e, &OFFLINE);

        let (status, state) = call(&app, "GET", "/api/state", Value::Null);
        assert_eq!(status, 200);
        assert_eq!(state["providers"][0]["id"], "claude");
        assert_eq!(state["credentials"].as_array().unwrap().len(), 2);

        let (_, r) = call(&app, "POST", "/api/use", json!({"name": "perso"}));
        assert_eq!(
            r["message"],
            "switched Claude Code to perso (u-perso@example.com)"
        );
        assert_eq!(e.live_refresh_token().as_deref(), Some("r-perso"));

        let (_, r) = call(
            &app,
            "POST",
            "/api/label",
            json!({"name": "work", "text": "Job"}),
        );
        assert_eq!(r["message"], "labelled claude/work \"Job\"");
        let (_, r) = call(
            &app,
            "POST",
            "/api/rename",
            json!({"name": "work", "to": "job"}),
        );
        assert_eq!(r["message"], "renamed claude/work to claude/job");
        let (status, r) = call(&app, "POST", "/api/remove", json!({"name": "perso"}));
        assert_eq!(
            (status, r["error"].as_str().unwrap().contains("is active")),
            (400, true)
        );
        let (_, r) = call(&app, "POST", "/api/remove", json!({"name": "job"}));
        assert_eq!(r["message"], "removed claude/job");

        let (_, r) = call(&app, "POST", "/api/refresh", json!({}));
        assert_eq!(r["message"], "nothing was due");
        let (status, r) = call(&app, "POST", "/api/use", json!({}));
        assert_eq!(
            (status, r["error"].as_str()),
            (400, Some("name is required"))
        );
        let (status, _) = call(&app, "POST", "/api/nothing", json!({}));
        assert_eq!(status, 400);
    }

    #[test]
    fn a_live_login_nobody_stored_can_be_imported() {
        let e = env(&OFFLINE);
        e.sign_in(oauth("a1", "r1", HOUR), account("u-new"));
        let app = app(&e, &OFFLINE);
        let (_, state) = call(&app, "GET", "/api/state", Value::Null);
        assert_eq!(state["live"][0]["state"], "unstored");
        assert_eq!(state["live"][0]["email"], "u-new@example.com");
        let (_, r) = call(&app, "POST", "/api/import", json!({"name": "new"}));
        assert_eq!(
            r["message"],
            "stored claude/new (u-new@example.com, max 5x)"
        );
    }

    #[test]
    fn a_sign_in_runs_in_two_calls() {
        let mut server = mockito::Server::new();
        server
            .mock("POST", "/v1/oauth/token")
            .with_body(
                json!({"access_token": "a-login", "refresh_token": "r-login", "expires_in": 28800})
                    .to_string(),
            )
            .create();
        server
            .mock("GET", "/api/oauth/profile")
            .with_body(profile_body("u-new"))
            .create();
        let e = env(&server.url());
        let opened = Arc::new(Mutex::new(Vec::new()));
        let seen = opened.clone();
        let app = app_opening(
            &e,
            &server.url(),
            Box::new(move |url| {
                seen.lock().unwrap().push(url.to_owned());
                true
            }),
        );

        let (status, begun) = call(&app, "POST", "/api/login", json!({"name": "new"}));
        assert_eq!(status, 200);
        assert_eq!(begun["needsCode"], true);
        assert_eq!(begun["opened"], true);
        assert_eq!(*opened.lock().unwrap(), [begun["url"].as_str().unwrap()]);
        let url = reqwest::Url::parse(begun["url"].as_str().unwrap()).unwrap();
        let state = url.query_pairs().find(|(k, _)| k == "state").unwrap().1;
        let id = begun["id"].as_str().unwrap();

        let (_, r) = call(
            &app,
            "POST",
            "/api/login/finish",
            json!({"id": id, "code": format!("c#{state}")}),
        );
        assert_eq!(r["message"], "stored claude/new (u-new@example.com, pro)");
        let (status, r) = call(&app, "POST", "/api/login/finish", json!({"id": id}));
        assert_eq!(status, 400);
        assert!(r["error"].as_str().unwrap().contains("no longer open"));

        let (status, r) = call(&app, "POST", "/api/login", json!({"name": "new"}));
        assert_eq!(status, 400);
        assert!(r["error"].as_str().unwrap().contains("already exists"));
    }

    #[test]
    fn a_second_sign_in_for_the_same_cli_replaces_the_first() {
        let e = env(&OFFLINE);
        let app = app(&e, &OFFLINE);
        let (_, first) = call(&app, "POST", "/api/login", json!({"name": "one"}));
        let (_, second) = call(&app, "POST", "/api/login", json!({"name": "two"}));
        assert_eq!(app.pending.lock().unwrap().len(), 1);
        let (status, r) = call(
            &app,
            "POST",
            "/api/login/finish",
            json!({"id": first["id"], "code": "c#s"}),
        );
        assert_eq!(status, 400);
        assert!(r["error"].as_str().unwrap().contains("no longer open"));
        assert!(
            app.pending
                .lock()
                .unwrap()
                .contains_key(second["id"].as_str().unwrap())
        );
    }

    #[test]
    fn a_cancelled_sign_in_stops_waiting() {
        use crate::codex::{Api as CodexApi, Codex};
        let e = env(&OFFLINE);
        let providers: Vec<Box<dyn Provider>> = vec![Box::new(Codex::at(
            e.tmp.path(),
            CodexApi::local(&OFFLINE),
            0,
        ))];
        let app = Arc::new(App::new(
            Store::open(e.store.root()),
            places(&e),
            providers,
            TOKEN.into(),
            PORT,
            Box::new(|_| false),
        ));
        let (_, begun) = call(
            &app,
            "POST",
            "/api/login",
            json!({"provider": "codex", "name": "x"}),
        );
        assert_eq!(begun["opened"], false);
        let id = begun["id"].as_str().unwrap().to_owned();
        let waiting = {
            let app = Arc::clone(&app);
            let id = id.clone();
            std::thread::spawn(move || call(&app, "POST", "/api/login/finish", json!({"id": id})))
        };
        while app.pending.lock().unwrap()[&id].login.is_some() {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        call(&app, "POST", "/api/login/cancel", json!({"id": id}));
        let (status, r) = waiting.join().unwrap();
        assert_eq!(status, 400);
        assert_eq!(r["error"], "the sign-in was cancelled");
    }
}
