use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Value, json};

use crate::commands::{self, RefreshScope};
use crate::ops;
use crate::pkce;
use crate::provider::{PendingLogin, Provider};
use crate::store::{Store, validate_name};

const PAGE: &str = include_str!("serve.html");
const MAX_BODY: u64 = 64 * 1024;

/// `login` is taken out while its `finish` runs; `cancel` stays behind so a
/// cancel can still reach it.
struct Pending {
    provider: &'static str,
    name: String,
    force: bool,
    login: Option<Box<dyn PendingLogin>>,
    cancel: Box<dyn Fn() + Send + Sync>,
}

pub struct App {
    store: Store,
    providers: Vec<Box<dyn Provider>>,
    token: String,
    port: u16,
    pending: Mutex<HashMap<String, Pending>>,
}

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
    fn json(status: u16, body: Value) -> Self {
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
    pub fn new(store: Store, providers: Vec<Box<dyn Provider>>, token: String, port: u16) -> Self {
        App {
            store,
            providers,
            token,
            port,
            pending: Mutex::new(HashMap::new()),
        }
    }

    fn providers(&self) -> Vec<&dyn Provider> {
        self.providers.iter().map(|p| p.as_ref()).collect()
    }

    /// A page on another site can reach 127.0.0.1 through the user's browser,
    /// and a rebound DNS name can reach it with its own Host. Only a request
    /// that names this listener, and carries the token from the URL the user
    /// was given, is served.
    fn is_local(&self, host: &str) -> bool {
        host == format!("127.0.0.1:{}", self.port) || host == format!("localhost:{}", self.port)
    }

    pub fn handle(&self, req: &Request) -> Response {
        if !req.host.is_some_and(|h| self.is_local(h)) {
            return Response::json(403, json!({"error": "not a local request"}));
        }
        if let Some(origin) = req.origin
            && !origin
                .strip_prefix("http://")
                .is_some_and(|h| self.is_local(h))
        {
            return Response::json(403, json!({"error": "cross-origin request refused"}));
        }
        match (req.method, req.path) {
            ("GET", "/") => Response {
                status: 200,
                content_type: "text/html; charset=utf-8",
                body: PAGE.to_owned(),
            },
            (_, path) if path.starts_with("/api/") => {
                if req.token != Some(self.token.as_str()) {
                    return Response::json(401, json!({"error": "missing or wrong token"}));
                }
                match self.api(req.method, path, req.body) {
                    Ok(v) => Response::json(200, v),
                    Err(e) => Response::json(400, json!({"error": format!("{e:#}")})),
                }
            }
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

        match (method, path) {
            ("GET", "/api/state") => {
                let _lock = store.lock()?;
                let lives = commands::sync_all(store, &providers)?;
                let mut state = commands::list_json(store, &lives)?;
                state["providers"] = providers
                    .iter()
                    .map(|p| json!({"id": p.id(), "name": p.name()}))
                    .collect();
                Ok(state)
            }
            ("POST", "/api/use") => {
                let _lock = store.lock()?;
                let (p, name) = commands::resolve(store, &providers, spec()?)?;
                Ok(json!({"message": ops::switch(store, p, &name, flag("discard"))?}))
            }
            ("POST", "/api/import") => {
                let _lock = store.lock()?;
                let p = commands::find(&providers, text("provider").unwrap_or("claude"))?;
                let state = ops::sync_live(store, p)?;
                capture(|out| commands::import(store, p, &state, spec()?, flag("force"), out))
            }
            ("POST", "/api/label") => {
                let _lock = store.lock()?;
                let (p, name) = commands::resolve(store, &providers, spec()?)?;
                capture(|out| commands::label(store, p, &name, text("text"), out))
            }
            ("POST", "/api/rename") => {
                let _lock = store.lock()?;
                let (p, name) = commands::resolve(store, &providers, spec()?)?;
                let to = text("to").context("to is required")?;
                capture(|out| commands::rename(store, p, &name, to, out))
            }
            ("POST", "/api/remove") => {
                let _lock = store.lock()?;
                let (p, name) = commands::resolve(store, &providers, spec()?)?;
                let state = ops::sync_live(store, p)?;
                capture(|out| commands::remove(store, p, &state, &name, out))
            }
            ("POST", "/api/refresh") => {
                let _lock = store.lock()?;
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
                let result = commands::refresh(store, &lives, scope, &mut out, &mut err);
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
                let p = commands::find(&providers, text("provider").unwrap_or("claude"))?;
                let name = spec()?;
                validate_name(name)?;
                let force = flag("force");
                if !force && store.get(p.id(), name)?.is_some() {
                    bail!("{}/{name} already exists", p.id());
                }
                let mut pending = self.pending.lock().map_err(|_| anyhow!("poisoned"))?;
                pending.retain(|_, l| {
                    let keep = l.provider != p.id();
                    if !keep {
                        (l.cancel)();
                    }
                    keep
                });
                let login = p.begin_login()?;
                let id = pkce::random()?;
                let answer = json!({"id": id, "url": login.url(), "needsCode": login.needs_code()});
                pending.insert(
                    id,
                    Pending {
                        provider: p.id(),
                        name: name.to_owned(),
                        force,
                        cancel: login.canceller(),
                        login: Some(login),
                    },
                );
                Ok(answer)
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
                Ok(
                    json!({"message": format!("stored {} ({})", entry.qualified(), entry.meta.email)}),
                )
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

fn header(rq: &tiny_http::Request, name: &'static str) -> Option<String> {
    rq.headers()
        .iter()
        .find(|h| h.field.equiv(name))
        .map(|h| h.value.as_str().to_owned())
}

/// One thread per request: a Codex sign-in holds its request open until the
/// browser calls back.
pub fn serve(app: Arc<App>, server: tiny_http::Server) {
    for mut rq in server.incoming_requests() {
        let app = Arc::clone(&app);
        std::thread::spawn(move || {
            let mut body = String::new();
            let _ = rq.as_reader().take(MAX_BODY).read_to_string(&mut body);
            let (host, origin, token) = (
                header(&rq, "Host"),
                header(&rq, "Origin"),
                header(&rq, "X-Remuda-Token"),
            );
            let method = rq.method().as_str().to_owned();
            let path = rq.url().split('?').next().unwrap_or_default().to_owned();
            let resp = app.handle(&Request {
                method: &method,
                path: &path,
                host: host.as_deref(),
                origin: origin.as_deref(),
                token: token.as_deref(),
                body: &body,
            });
            let mut out = tiny_http::Response::from_string(resp.body).with_status_code(resp.status);
            for (k, v) in [
                ("Content-Type", resp.content_type),
                ("Cache-Control", "no-store"),
                ("X-Content-Type-Options", "nosniff"),
                ("Referrer-Policy", "no-referrer"),
            ] {
                if let Ok(h) = tiny_http::Header::from_bytes(k, v) {
                    out.add_header(h);
                }
            }
            let _ = rq.respond(out);
        });
    }
}

pub fn bind(port: u16) -> Result<(tiny_http::Server, u16)> {
    let server = tiny_http::Server::http(("127.0.0.1", port))
        .map_err(|e| anyhow!("cannot listen on 127.0.0.1:{port} ({e}); pick another --port"))?;
    let port = server
        .server_addr()
        .to_ip()
        .context("not an IP listener")?
        .port();
    Ok((server, port))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::claude::{Api, Claude};
    use crate::ops::testing::*;

    const PORT: u16 = 7429;
    const TOKEN: &str = "t0ken";

    fn app(e: &Env, api_base: &str) -> App {
        let providers: Vec<Box<dyn Provider>> =
            vec![Box::new(Claude::at(e.tmp.path(), Api::local(api_base)))];
        App::new(Store::open(e.store.root()), providers, TOKEN.into(), PORT)
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
        let e = env(OFFLINE);
        let app = app(&e, OFFLINE);
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
        let e = env(OFFLINE);
        let resp = app(&e, OFFLINE).handle(&Request {
            method: "GET",
            path: "/",
            host: Some("127.0.0.1:7429"),
            origin: None,
            token: None,
            body: "",
        });
        assert!(resp.content_type.starts_with("text/html"));
        assert!(resp.body.contains("X-Remuda-Token"));
        assert!(!resp.body.contains("<script src"));
        assert!(!resp.body.contains("<link"));
    }

    #[test]
    fn credentials_are_listed_switched_labelled_renamed_and_removed() {
        let e = env(OFFLINE);
        e.stored("work", "u-work", HOUR);
        e.stored("perso", "u-perso", HOUR);
        let work = e.store.get("claude", "work").unwrap().unwrap();
        e.sign_in(work.creds, account("u-work"));
        let app = app(&e, OFFLINE);

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
        let e = env(OFFLINE);
        e.sign_in(oauth("a1", "r1", HOUR), account("u-new"));
        let app = app(&e, OFFLINE);
        let (_, state) = call(&app, "GET", "/api/state", Value::Null);
        assert_eq!(state["live"][0]["state"], "unstored");
        assert_eq!(state["live"][0]["email"], "u-new@example.com");
        let (_, r) = call(&app, "POST", "/api/import", json!({"name": "new"}));
        assert_eq!(r["message"], "stored claude/new (u-new@example.com)");
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
        let app = app(&e, &server.url());

        let (status, begun) = call(&app, "POST", "/api/login", json!({"name": "new"}));
        assert_eq!(status, 200);
        assert_eq!(begun["needsCode"], true);
        let url = reqwest::Url::parse(begun["url"].as_str().unwrap()).unwrap();
        let state = url.query_pairs().find(|(k, _)| k == "state").unwrap().1;
        let id = begun["id"].as_str().unwrap();

        let (_, r) = call(
            &app,
            "POST",
            "/api/login/finish",
            json!({"id": id, "code": format!("c#{state}")}),
        );
        assert_eq!(r["message"], "stored claude/new (u-new@example.com)");
        let (status, r) = call(&app, "POST", "/api/login/finish", json!({"id": id}));
        assert_eq!(status, 400);
        assert!(r["error"].as_str().unwrap().contains("no longer open"));

        let (status, r) = call(&app, "POST", "/api/login", json!({"name": "new"}));
        assert_eq!(status, 400);
        assert!(r["error"].as_str().unwrap().contains("already exists"));
    }

    #[test]
    fn a_cancelled_sign_in_stops_waiting() {
        use crate::codex::{Api as CodexApi, Codex};
        let e = env(OFFLINE);
        let providers: Vec<Box<dyn Provider>> = vec![Box::new(Codex::at(
            e.tmp.path(),
            CodexApi::local(OFFLINE),
            0,
        ))];
        let app = Arc::new(App::new(
            Store::open(e.store.root()),
            providers,
            TOKEN.into(),
            PORT,
        ));
        let (_, begun) = call(
            &app,
            "POST",
            "/api/login",
            json!({"provider": "codex", "name": "x"}),
        );
        let id = begun["id"].as_str().unwrap().to_owned();
        let waiting = {
            let app = Arc::clone(&app);
            let id = id.clone();
            std::thread::spawn(move || call(&app, "POST", "/api/login/finish", json!({"id": id})))
        };
        std::thread::sleep(std::time::Duration::from_millis(300));
        call(&app, "POST", "/api/login/cancel", json!({"id": id}));
        let (status, r) = waiting.join().unwrap();
        assert_eq!(status, 400);
        assert_eq!(r["error"], "the sign-in was cancelled");
    }

    #[test]
    fn the_listener_answers_over_a_real_socket() {
        let e = env(OFFLINE);
        let (server, port) = bind(0).unwrap();
        let providers: Vec<Box<dyn Provider>> =
            vec![Box::new(Claude::at(e.tmp.path(), Api::local(OFFLINE)))];
        let app = Arc::new(App::new(
            Store::open(e.store.root()),
            providers,
            TOKEN.into(),
            port,
        ));
        std::thread::spawn(move || serve(app, server));

        let get = |path: &str, token: &str| {
            let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
            write!(
                s,
                "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nX-Remuda-Token: {token}\r\nConnection: close\r\n\r\n"
            )
            .unwrap();
            let mut answer = String::new();
            s.read_to_string(&mut answer).unwrap();
            answer
        };
        let page = get("/", "");
        assert!(page.starts_with("HTTP/1.1 200"), "{page}");
        assert!(page.contains("Cache-Control: no-store"), "{page}");
        assert!(get("/api/state", "wrong").starts_with("HTTP/1.1 401"));
        let state = get("/api/state", TOKEN);
        assert!(state.starts_with("HTTP/1.1 200"), "{state}");
        assert!(state.contains("\"providers\""), "{state}");
    }
}
