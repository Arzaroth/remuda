use std::collections::{BTreeMap, HashMap};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow, bail};
use jiff::Zoned;
use serde_json::{Value, json};

use crate::auto;
use crate::commands::{self, RefreshScope};
use crate::config;
use crate::fsx;
use crate::gauge;
use crate::logbook::{self, By, Event, Kind};
use crate::ops;
use crate::paths;
use crate::pkce;
use crate::provider::{PendingLogin, Provider};
use crate::proxy;
use crate::runs;
use crate::shown;
use crate::store::Store;
use crate::warm;

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
    pub config: PathBuf,
}

impl Places {
    pub fn from_env() -> Self {
        Places {
            units: paths::systemd_user_units(),
            snapshot: paths::tokengauge_snapshot(),
            config: paths::config_file(),
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
    auto: Mutex<BTreeMap<String, Value>>,
    /// `provider/name` to what keep-warm last did for that login.
    warm: Mutex<BTreeMap<String, Value>>,
    proxy: proxy::Control,
    /// Provider to why the proxy last turned its active login down, until
    /// it next spends it.
    refused: Mutex<BTreeMap<String, String>>,
    me: OnceLock<Weak<App>>,
}

/// The latest events the page's Log shows; `remuda log` reaches further back.
const LOG_SHOWN: usize = 500;

/// Opens a sign-in page in a private window; false leaves it to the page.
pub type Opener = Box<dyn Fn(&str) -> bool + Send + Sync>;

/// What a round decided, without its reasons, whose figures move every round.
fn gist(seen: &Value) -> (&Value, &Value) {
    let action = &seen["decision"]["action"];
    (&action["kind"], &action["to"])
}

/// Runs auto mode beside the page: when the store, TokenGauge's snapshot or
/// the config changes, and every minute so a schedule takes effect on time.
pub fn watch(app: std::sync::Arc<App>) {
    std::thread::spawn(move || {
        let mut seen = None;
        let mut last: Option<std::time::Instant> = None;
        loop {
            let now = app.fingerprint();
            if seen.as_ref() != Some(&now)
                || last.is_none_or(|l| l.elapsed() >= std::time::Duration::from_secs(60))
            {
                app.apply_proxy();
                app.auto_round(&Zoned::now());
                app.warm_round(jiff::Timestamp::now());
                seen = Some(app.fingerprint());
                last = Some(std::time::Instant::now());
            }
            std::thread::sleep(std::time::Duration::from_secs(5));
        }
    });
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
    pub(crate) fn json(status: u16, body: Value) -> Self {
        Response {
            status,
            content_type: "application/json",
            body: body.to_string(),
        }
    }
}

fn sent_config(body: &Value) -> Result<config::Config> {
    serde_json::from_value(body.get("config").cloned().unwrap_or(json!({})))
        .context("config is not a valid config")
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
            auto: Mutex::new(BTreeMap::new()),
            warm: Mutex::new(BTreeMap::new()),
            proxy: proxy::Control::default(),
            refused: Mutex::new(BTreeMap::new()),
            me: OnceLock::new(),
        }
    }

    /// Starts the proxy when its settings say so. The listener holds the app
    /// weakly, so it stops answering once the app is gone.
    pub fn start(self: &Arc<Self>) {
        let _ = self.me.set(Arc::downgrade(self));
        self.apply_proxy();
    }

    fn apply_proxy(&self) {
        let Some(me) = self.me.get() else {
            return;
        };
        let backend: Weak<dyn proxy::Backend> = me.clone();
        match self.proxy_file().read() {
            Ok(settings) => self.proxy.apply(backend, &settings),
            Err(e) => eprintln!("warning: the proxy is not running: {e:#}"),
        }
    }

    /// Pings each login named in `[warm]` whose 5-hour window has closed,
    /// one at a time, never holding the store lock across a ping.
    pub fn warm_round(&self, now: jiff::Timestamp) {
        let Ok(loaded) = config::load(&self.places.config) else {
            return;
        };
        let warm = loaded.config.warm;
        let mut memory = self.warm.lock().unwrap().clone();
        memory.retain(|key, _| {
            key.split_once('/').is_some_and(|(p, n)| {
                warm.get(p)
                    .is_some_and(|names| names.iter().any(|x| x == n))
            })
        });
        if warm.is_empty() {
            *self.warm.lock().unwrap() = memory;
            return;
        }
        let usage = gauge::read(&self.places.snapshot);
        let mut actives = BTreeMap::new();
        let switches = {
            let Ok(_lock) = self.store.lock() else {
                return;
            };
            for id in warm.keys() {
                let active = match self.providers().into_iter().find(|p| p.id() == id) {
                    Some(p) => ops::sync_live(&self.store, p)
                        .map_err(|e| format!("{e:#}"))
                        .and_then(|live| warm::active(&live)),
                    None => Err(format!("remuda has no provider {id:?}")),
                };
                actives.insert(id.clone(), active);
            }
            runs::switches(&self.store)
        };
        let last = |p: &str, n: &str| {
            let m = memory.get(&format!("{p}/{n}"))?;
            Some(warm::Last {
                at: m["lastPing"].as_i64()?,
                ok: m["lastOk"].as_bool().unwrap_or(true),
            })
        };
        let active = |id: &str| actives.get(id).cloned().unwrap_or(Ok(None));
        let planned = warm::plan(&warm, usage.as_ref(), &switches, &active, &last, now);
        let mut pinged = false;
        for planned in planned {
            let key = format!("{}/{}", planned.provider, planned.name);
            let before = memory.get(&key).cloned().unwrap_or_else(|| json!({}));
            let mut seen = match planned.due {
                warm::Due::Open { resets_at } => {
                    json!({"state": "open", "resetsAt": resets_at.as_millisecond()})
                }
                warm::Due::Not(_) if before["state"] == "failed" => before.clone(),
                warm::Due::Not(why) => json!({"state": "waiting", "why": why}),
                warm::Due::Now => {
                    let Some(p) = self
                        .providers()
                        .into_iter()
                        .find(|p| p.id() == planned.provider)
                    else {
                        continue;
                    };
                    let creds = {
                        let Ok(_lock) = self.store.lock() else {
                            continue;
                        };
                        ops::sync_live(&self.store, p).and_then(|live| {
                            let active = warm::active(&live).map_err(anyhow::Error::msg)?;
                            warm::creds(&self.store, p, &planned.name, active.as_deref())
                        })
                    };
                    let done = creds.and_then(|c| proxy::ping(p, &c));
                    let at = fsx::now_ms();
                    match done {
                        Ok(model) => {
                            pinged = true;
                            eprintln!("keep warm: opened {key}'s 5-hour window on {model}");
                            logbook::note(
                                &self.store,
                                Event::new(
                                    Kind::Warm,
                                    true,
                                    format!("opened the 5-hour window on {model}"),
                                )
                                .about(&planned.provider, Some(&planned.name)),
                            );
                            json!({"state": "pinged", "at": at, "model": model,
                                   "lastPing": at, "lastOk": true})
                        }
                        Err(e) => {
                            let error = format!("{e:#}");
                            if before["error"].as_str() != Some(error.as_str()) {
                                eprintln!("keep warm: could not ping {key}: {error}");
                            }
                            logbook::note_once(
                                &self.store,
                                Event::new(Kind::Warm, false, format!("could not ping: {error}"))
                                    .about(&planned.provider, Some(&planned.name)),
                            );
                            json!({"state": "failed", "at": at, "error": error,
                                   "lastPing": at, "lastOk": false})
                        }
                    }
                }
            };
            for k in ["lastPing", "lastOk"] {
                if seen.get(k).is_none()
                    && let Some(v) = before.get(k)
                {
                    seen[k] = v.clone();
                }
            }
            memory.insert(key, seen);
        }
        *self.warm.lock().unwrap() = memory;
        if pinged {
            gauge::nudge(&self.places.snapshot);
        }
    }

    /// One round of auto mode: decide for every CLI config.toml sets up, and
    /// switch the ones it is on for. What it decided is kept for the page.
    pub fn auto_round(&self, now: &Zoned) {
        let config = match config::load(&self.places.config) {
            Ok(l) => l.config,
            Err(e) => {
                let mut seen = self.auto.lock().unwrap();
                if seen.get("error").and_then(Value::as_str) != Some(&format!("{e:#}")) {
                    eprintln!("auto mode: {e:#}");
                }
                *seen = BTreeMap::from([("error".to_owned(), json!(format!("{e:#}")))]);
                return;
            }
        };
        let usage = gauge::read(&self.places.snapshot);
        let Ok(_lock) = self.store.lock() else {
            return;
        };
        let switches = runs::switches(&self.store);
        let before = self.auto.lock().unwrap().clone();
        let mut after = BTreeMap::new();
        let mut switched = false;
        for (id, settings) in &config.auto {
            let Some(p) = self.providers().into_iter().find(|p| p.id() == id) else {
                continue;
            };
            let cx = auto::Context {
                schedules: &config.schedules,
                usage: usage.as_ref(),
                switched_at: switches.get(id).copied(),
                now,
            };
            if !settings.enabled {
                after.insert(id.clone(), json!({"enabled": false}));
                continue;
            }
            let decision = match ops::sync_live(&self.store, p) {
                Ok(live) => auto::assess(settings, p, &live, &cx),
                Err(e) => auto::Decision::hold(format!("{e:#}")),
            };
            let at = now.timestamp().as_millisecond();
            let mut seen = json!({"enabled": true, "at": at, "decision": decision});
            let last = before.get(id.as_str());
            if let Some(prev) = last.and_then(|l| l.get("switched")) {
                seen["switched"] = prev.clone();
            }
            let ready = || match auto::sessions_open(p) {
                Some(busy) => bail!("{busy}"),
                None => Ok(()),
            };
            if let auto::Action::Switch { to, why } = &decision.action {
                match ops::switch_when(&self.store, p, to, false, By::Auto, Some(why), &ready) {
                    Ok(message) => {
                        eprintln!("auto mode: {message}: {why}");
                        seen["switched"] = json!({"at": at, "to": to, "why": why});
                        switched = true;
                    }
                    Err(e) => {
                        seen["error"] = json!(format!("{e:#}"));
                        if last.map(|l| &l["error"]) != Some(&seen["error"]) {
                            eprintln!("auto mode: could not switch {} to {to}: {e:#}", p.name());
                        }
                        logbook::note_once(
                            &self.store,
                            Event::new(
                                Kind::Switch,
                                false,
                                format!("could not switch to {to}: {e:#}"),
                            )
                            .about(p.id(), None)
                            .by(By::Auto),
                        );
                    }
                }
            } else if last.map(gist) != Some(gist(&seen)) {
                let action = &seen["decision"]["action"];
                let kind = action["kind"].as_str().unwrap_or_default();
                let why = action["why"].as_str().unwrap_or_default();
                eprintln!("auto mode: {} {kind}: {why}", p.name());
                logbook::note(
                    &self.store,
                    Event::new(Kind::Auto, true, format!("{kind}: {why}")).about(p.id(), None),
                );
            }
            after.insert(id.clone(), seen);
        }
        drop(_lock);
        if switched {
            gauge::nudge(&self.places.snapshot);
        }
        *self.auto.lock().unwrap() = after;
    }

    fn providers(&self) -> Vec<&dyn Provider> {
        self.providers.iter().map(|p| p.as_ref()).collect()
    }

    fn proxy_file(&self) -> proxy::settings::File {
        proxy::settings::File::at(self.store.root().join(".proxy.json"))
    }

    /// A settings file that cannot be read shows as the proxy's error, not as
    /// a page that cannot load.
    fn proxy_state(&self) -> Value {
        let (s, unreadable) = match self.proxy_file().read() {
            Ok(s) => (s, None),
            Err(e) => (proxy::settings::Settings::default(), Some(format!("{e:#}"))),
        };
        let (running, error) = self.proxy.status();
        let error = unreadable.or(error);
        let origin = format!("http://127.0.0.1:{}", running.unwrap_or(s.port));
        let providers: Vec<Value> = self
            .providers
            .iter()
            .filter_map(|p| {
                let models = proxy::route::models(p.id());
                let mut formats = Vec::new();
                for m in models {
                    for f in p.formats(proxy::route::bare(m)) {
                        if !formats.contains(&f.name()) {
                            formats.push(f.name());
                        }
                    }
                }
                (!formats.is_empty())
                    .then(|| json!({"provider": p.id(), "models": models, "formats": formats}))
            })
            .collect();
        json!({
            "enabled": s.enabled,
            "port": s.port,
            "running": running,
            "error": error,
            "routes": providers,
            "baseUrls": {"openai": format!("{origin}/v1"), "claude": origin},
            "keys": s.keys,
        })
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

    /// Only the store's JSON files count: the temp files its writes go
    /// through and the redirect pages kept there without `$XDG_RUNTIME_DIR`
    /// would wake every page for nothing. Links are followed, since a write
    /// goes through them to the file they name.
    pub fn fingerprint(&self) -> Vec<(PathBuf, SystemTime, u64)> {
        let stamp = |path: PathBuf| {
            let meta = std::fs::metadata(&path).ok()?;
            Some((path, meta.modified().unwrap_or(UNIX_EPOCH), meta.len()))
        };
        let entries = |dir: &Path| {
            std::fs::read_dir(dir)
                .into_iter()
                .flatten()
                .flatten()
                .map(|e| e.path())
                .collect::<Vec<_>>()
        };
        let is_json = |p: &PathBuf| p.extension().is_some_and(|x| x == "json" || x == "jsonl");
        let mut files = Vec::new();
        for path in entries(self.store.root()) {
            if path.is_dir() {
                files.extend(entries(&path).into_iter().filter(is_json));
            } else if is_json(&path) {
                files.push(path);
            }
        }
        files.push(self.places.snapshot.clone());
        files.push(self.places.config.clone());
        let mut seen: Vec<_> = files.into_iter().filter_map(stamp).collect();
        seen.sort();
        seen
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

    /// A stream outlives one `Response`, so the listener serves it itself.
    pub fn streams(&self, req: &Request) -> bool {
        req.method == "GET" && req.path == "/api/events"
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

    fn logins(&self, provider: &str) -> Option<Vec<String>> {
        let p = self.providers().into_iter().find(|p| p.id() == provider)?;
        let entries = self.store.list(p.id()).ok()?;
        Some(entries.into_iter().map(|e| e.name).collect())
    }

    fn config(&self) -> Result<Value> {
        let loaded = config::load(&self.places.config)?;
        let problems = loaded.config.check(&|id| self.logins(id));
        Ok(json!({
            "config": loaded.config,
            "version": loaded.version,
            "problems": problems,
            "path": self.places.config,
        }))
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
                    .map(|p| {
                        let warm = crate::proxy::route::cheapest(p.id()).is_some();
                        json!({"id": p.id(), "name": p.name(), "warm": warm})
                    })
                    .collect();
                state["health"] = self.health();
                state["usage"] = gauge::read(&self.places.snapshot).unwrap_or(Value::Null);
                state["shown"] = shown::shown(&state);
                state["auto"] = json!(*self.auto.lock().unwrap());
                state["warm"] = json!(*self.warm.lock().unwrap());
                state["proxy"] = self.proxy_state();
                Ok(state)
            }
            ("GET", "/api/config") => self.config(),
            ("GET", "/api/log") => Ok(json!({ "events": logbook::recent(store, None, LOG_SHOWN) })),
            ("POST", "/api/config/check") => {
                let problems = match sent_config(&body) {
                    Ok(config) => config.check(&|id| self.logins(id)),
                    Err(e) => vec![config::Problem {
                        at: "config".into(),
                        message: format!("{e:#}"),
                    }],
                };
                Ok(json!({ "problems": problems }))
            }
            ("POST", "/api/config") => {
                let config = sent_config(&body)?;
                let version = text("version").context("version is required")?;
                let problems = config.check(&|id| self.logins(id));
                if !problems.is_empty() {
                    let list: Vec<String> = problems.iter().map(ToString::to_string).collect();
                    bail!("the config was not saved: {}", list.join("; "));
                }
                config::save(&self.places.config, &config, version)?;
                self.config()
            }
            ("POST", "/api/proxy/settings") => {
                let enabled = body.get("enabled").and_then(Value::as_bool);
                let port = match body.get("port") {
                    None | Some(Value::Null) => None,
                    Some(v) => Some(
                        v.as_u64()
                            .and_then(|p| u16::try_from(p).ok())
                            .filter(|p| *p > 0)
                            .context("port must be a number from 1 to 65535")?,
                    ),
                };
                self.proxy_file().edit(|s| {
                    if let Some(enabled) = enabled {
                        s.enabled = enabled;
                    }
                    if let Some(port) = port {
                        s.port = port;
                    }
                    Ok(())
                })?;
                self.apply_proxy();
                let (running, error) = self.proxy.status();
                let message = match (running, error) {
                    (Some(port), _) => format!("the proxy listens on 127.0.0.1:{port}"),
                    (None, Some(e)) => bail!("saved, but the proxy is not running: {e}"),
                    (None, None) => "the proxy is off".to_owned(),
                };
                Ok(json!({"message": message}))
            }
            ("POST", "/api/proxy/keys") => {
                let key = self.proxy_file().add_key(spec()?)?;
                Ok(json!({"message": format!("added the key {}", key.name), "id": key.id}))
            }
            ("POST", "/api/proxy/keys/remove") => {
                self.proxy_file()
                    .remove_key(text("id").context("id is required")?)?;
                Ok(json!({"message": "key removed"}))
            }
            ("POST", "/api/use") => {
                let (p, name) = commands::resolve(store, &providers, spec()?)?;
                let message = ops::switch_by(store, p, &name, flag("discard"), By::Page)?;
                gauge::nudge(&self.places.snapshot);
                Ok(json!({ "message": message }))
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
                capture(|out| commands::rename(store, &self.places.config, p, &name, to, out))
            }
            ("POST", "/api/remove") => {
                let (p, name) = commands::resolve(store, &providers, spec()?)?;
                let state = ops::sync_live(store, p)?;
                capture(|out| commands::remove(store, &self.places.config, p, &state, &name, out))
            }
            ("POST", "/api/reset") => {
                let (p, name) = commands::resolve(store, &providers, spec()?)?;
                ops::sync_live(store, p)?;
                let entry = store
                    .get(p.id(), &name)?
                    .with_context(|| format!("no credential named {}/{name}", p.id()))?;
                let message = p.redeem_reset(&entry).inspect_err(|e| {
                    logbook::note(
                        store,
                        Event::new(Kind::Reset, false, format!("{e:#}"))
                            .about(p.id(), Some(&name))
                            .by(By::Page),
                    );
                })?;
                logbook::note(
                    store,
                    Event::new(Kind::Reset, true, &message)
                        .about(p.id(), Some(&name))
                        .by(By::Page),
                );
                gauge::nudge(&self.places.snapshot);
                Ok(json!({ "message": message }))
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
                gauge::nudge(&self.places.snapshot);
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
                let asks_for = login.asks_for();
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
                Ok(
                    json!({"id": id, "url": url, "needsCode": needs_code, "asksFor": asks_for, "opened": opened}),
                )
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

impl proxy::Backend for App {
    fn settings(&self) -> Result<proxy::settings::Settings> {
        self.proxy_file().read()
    }

    fn providers(&self) -> Vec<&dyn Provider> {
        App::providers(self)
    }

    fn spent(&self, provider: &str, refused: Option<&str>) {
        let Ok(mut all) = self.refused.lock() else {
            return;
        };
        let event = match refused {
            Some(why) if all.get(provider).map(String::as_str) != Some(why) => {
                all.insert(provider.to_owned(), why.to_owned());
                Event::new(Kind::Proxy, false, why)
            }
            None if all.remove(provider).is_some() => {
                Event::new(Kind::Proxy, true, "spending the active login again")
            }
            _ => return,
        };
        drop(all);
        logbook::note(&self.store, event.about(provider, None));
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
            config: e.tmp.path().join("config.toml"),
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

    /// A TokenGauge daemon beside the test snapshot that acks every command,
    /// and the next command it read, waited for a few seconds at most.
    fn gauge_daemon(e: &Env) -> impl Fn() -> String {
        use std::io::{BufRead, BufReader};
        let daemon =
            std::os::unix::net::UnixListener::bind(e.tmp.path().join("tokengauge.sock")).unwrap();
        let (heard, commands) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for s in daemon.incoming() {
                let s = s.unwrap();
                let mut line = String::new();
                BufReader::new(&s).read_line(&mut line).unwrap();
                if heard.send(line).is_err() {
                    return;
                }
                (&s).write_all(b"{\"type\":\"ack\"}\n").unwrap();
            }
        });
        move || {
            commands
                .recv_timeout(std::time::Duration::from_secs(5))
                .expect("remuda never nudged TokenGauge")
        }
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
    fn the_page_manages_the_proxy() {
        let e = env(&OFFLINE);
        let app = app(&e, &OFFLINE);
        let proxy = || call(&app, "GET", "/api/state", Value::Null).1["proxy"].clone();
        assert_eq!(proxy()["enabled"], false);
        assert_eq!(proxy()["baseUrls"]["openai"], "http://127.0.0.1:8317/v1");
        assert_eq!(proxy()["baseUrls"]["claude"], "http://127.0.0.1:8317");
        assert_eq!(proxy()["routes"][0]["provider"], "claude");
        assert_eq!(
            proxy()["routes"][0]["formats"],
            json!(["Anthropic messages"])
        );

        let (status, _) = call(
            &app,
            "POST",
            "/api/proxy/settings",
            json!({"enabled": true, "port": 9000}),
        );
        assert_eq!(status, 200);
        assert_eq!(proxy()["enabled"], true);
        assert_eq!(proxy()["baseUrls"]["claude"], "http://127.0.0.1:9000");
        let (status, _) = call(&app, "POST", "/api/proxy/settings", json!({"port": 0}));
        assert_eq!(status, 400);
        let (status, _) = call(&app, "POST", "/api/proxy/settings", json!({"port": 70000}));
        assert_eq!(status, 400);

        let (status, r) = call(&app, "POST", "/api/proxy/keys", json!({"name": "laptop"}));
        assert_eq!(status, 200);
        let keys = proxy()["keys"].clone();
        assert_eq!(keys[0]["name"], "laptop");
        assert_eq!(keys[0]["id"], r["id"]);
        assert!(keys[0]["key"].as_str().unwrap().starts_with("rmd-"));

        let (status, _) = call(
            &app,
            "POST",
            "/api/proxy/keys/remove",
            json!({"id": r["id"]}),
        );
        assert_eq!(status, 200);
        assert_eq!(proxy()["keys"], json!([]));
    }

    #[test]
    fn a_broken_proxy_file_breaks_only_the_proxy() {
        let e = env(&OFFLINE);
        let app = app(&e, &OFFLINE);
        std::fs::create_dir_all(e.store.root()).unwrap();
        std::fs::write(
            e.store.root().join(".proxy.json"),
            r#"{"keys": "rmd-secret"}"#,
        )
        .unwrap();
        let (status, state) = call(&app, "GET", "/api/state", Value::Null);
        assert_eq!(status, 200);
        let error = state["proxy"]["error"].as_str().unwrap();
        assert!(error.contains("is not proxy settings"), "{error}");
        assert!(!error.contains("rmd-secret"), "{error}");
        assert!(state["providers"].is_array());
    }

    #[test]
    fn a_started_page_runs_the_proxy_its_settings_ask_for() {
        let e = env(&OFFLINE);
        let app = Arc::new(app(&e, &OFFLINE));
        std::fs::create_dir_all(e.store.root()).unwrap();
        std::fs::write(
            e.store.root().join(".proxy.json"),
            r#"{"enabled": true, "port": 0}"#,
        )
        .unwrap();
        app.start();
        let state = call(&app, "GET", "/api/state", Value::Null).1;
        let port = state["proxy"]["running"].as_u64().expect("not running") as u16;
        assert_eq!(
            state["proxy"]["baseUrls"]["claude"],
            format!("http://127.0.0.1:{port}")
        );
        assert!(std::net::TcpStream::connect(("127.0.0.1", port)).is_ok());

        let taken = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let busy = taken.local_addr().unwrap().port();
        let (status, r) = call(&app, "POST", "/api/proxy/settings", json!({"port": busy}));
        assert_eq!(status, 400);
        assert!(r["error"].as_str().unwrap().contains("not running"), "{r}");

        let (status, r) = call(
            &app,
            "POST",
            "/api/proxy/settings",
            json!({"enabled": false}),
        );
        assert_eq!(status, 200);
        assert_eq!(r["message"], "the proxy is off");
        assert_eq!(
            call(&app, "GET", "/api/state", Value::Null).1["proxy"]["running"],
            Value::Null
        );
    }

    #[test]
    fn a_proxy_settings_change_wakes_the_page() {
        let e = env(&OFFLINE);
        let app = app(&e, &OFFLINE);
        let before = app.fingerprint();
        call(&app, "POST", "/api/proxy/keys", json!({"name": "a"}));
        assert_ne!(app.fingerprint(), before);
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
        // With no stored login there is nothing to add up, but the CLI's own
        // figures still reach the page through `shown`.
        assert_eq!(
            state["shown"]["claude"]["usage"]["windows"][0]["usedPercent"],
            40
        );
        assert_eq!(state["shown"]["claude"]["weighted"], json!([]));

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
        let logged = logbook::read(&e.store);
        assert_eq!(
            (logged[0].kind, logged[0].subject().as_str(), logged[0].by),
            (Kind::Switch, "claude/perso", Some(By::Page))
        );

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
        let (_, log) = call(&app, "GET", "/api/log", Value::Null);
        let kinds: Vec<&str> = log["events"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["kind"].as_str().unwrap())
            .collect();
        assert_eq!(kinds, ["switch", "store", "store"]);
        assert_eq!(log["events"][0]["by"], "page");

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
    fn the_page_reads_and_saves_the_config() {
        let e = env(&OFFLINE);
        e.stored("work", "u-work", HOUR);
        e.stored("perso", "u-perso", HOUR);
        let app = app(&e, &OFFLINE);
        let path = places(&e).config;

        let (status, r) = call(&app, "GET", "/api/config", Value::Null);
        assert_eq!((status, &r["config"]), (200, &json!({})));
        assert_eq!(r["problems"], json!([]));
        let empty = r["version"].clone();

        let config = json!({
            "auto": {"claude": {"enabled": true, "logins": ["work", "perso"]}}
        });
        let (status, r) = call(
            &app,
            "POST",
            "/api/config",
            json!({"config": config, "version": empty}),
        );
        assert_eq!((status, &r["config"]), (200, &config));
        assert_ne!(r["version"], empty);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("logins = [\"work\", \"perso\"]"));

        let (status, r) = call(
            &app,
            "POST",
            "/api/config",
            json!({"config": {}, "version": empty}),
        );
        assert_eq!(status, 400);
        assert!(
            r["error"]
                .as_str()
                .unwrap()
                .contains("changed since it was read")
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
    }

    #[test]
    fn the_page_cannot_save_a_config_auto_mode_cannot_act_on() {
        let e = env(&OFFLINE);
        e.stored("work", "u-work", HOUR);
        let app = app(&e, &OFFLINE);
        let (_, r) = call(&app, "GET", "/api/config", Value::Null);

        let (status, r) = call(
            &app,
            "POST",
            "/api/config",
            json!({
                "config": {"auto": {"claude": {"logins": ["ghost"]}}},
                "version": r["version"],
            }),
        );

        assert_eq!(
            (status, r["error"].as_str()),
            (
                400,
                Some("the config was not saved: auto.claude.logins: no stored login claude/ghost")
            )
        );
        assert!(!places(&e).config.exists());
    }

    #[test]
    fn the_page_checks_a_config_without_saving_it() {
        let e = env(&OFFLINE);
        e.stored("work", "u-work", HOUR);
        let app = app(&e, &OFFLINE);

        let (_, r) = call(
            &app,
            "POST",
            "/api/config/check",
            json!({"config": {"auto": {"claude": {"logins": ["work", "ghost"]}}}}),
        );
        assert_eq!(
            r["problems"],
            json!([{"at": "auto.claude.logins", "message": "no stored login claude/ghost"}])
        );
        let (_, r) = call(
            &app,
            "POST",
            "/api/config/check",
            json!({"config": {"schedules": {"x": {"hours": "09:00-09:00"}}}}),
        );
        assert_eq!(r["problems"][0]["at"], "config");
        assert!(!places(&e).config.exists());
    }

    fn snapshot(e: &Env, updated: i64, perso: u64, work: u64) {
        let window = |used: u64| json!({"primary": {"usedPercent": used, "windowMinutes": 300}});
        std::fs::write(
            places(e).snapshot,
            json!({
                "meta": {"schemaVersion": 2, "updatedAtMs": updated},
                "payloads": [
                    {"provider": "claude", "credential": "perso", "usage": window(perso)},
                    {"provider": "claude", "credential": "work", "usage": window(work)},
                ],
            })
            .to_string(),
        )
        .unwrap();
    }

    fn on_perso(e: &Env, enabled: bool) -> App {
        e.stored("work", "u-work", HOUR);
        e.stored("perso", "u-perso", HOUR);
        let perso = e.store.get("claude", "perso").unwrap().unwrap();
        e.sign_in(perso.creds, account("u-perso"));
        std::fs::write(
            places(e).config,
            format!("[auto.claude]\nenabled = {enabled}\nlogins = [\"work\", \"perso\"]\n"),
        )
        .unwrap();
        app(e, &OFFLINE)
    }

    #[test]
    fn auto_mode_moves_a_cli_off_a_maxed_login_then_waits_for_new_figures() {
        let e = env(&OFFLINE);
        let app = on_perso(&e, true);
        snapshot(&e, crate::fsx::now_ms() - 1000, 100, 10);

        app.auto_round(&Zoned::now());

        assert_eq!(e.live_refresh_token().as_deref(), Some("r-work"));
        let (_, state) = call(&app, "GET", "/api/state", Value::Null);
        let claude = &state["auto"]["claude"];
        assert_eq!(claude["switched"]["to"], "work");
        assert_eq!(claude["switched"]["why"], "perso's 5 hours limit maxed");

        app.auto_round(&Zoned::now());
        app.auto_round(&Zoned::now());
        let logged = logbook::read(&e.store);
        assert_eq!(logged.len(), 2, "{logged:?}");
        assert_eq!(
            (logged[0].kind, logged[0].subject().as_str(), logged[0].by),
            (Kind::Switch, "claude/work", Some(By::Auto))
        );
        assert!(logged[0].message.ends_with(": perso's 5 hours limit maxed"));
        assert_eq!(
            (logged[1].kind, logged[1].message.as_str()),
            (
                Kind::Auto,
                "hold: waiting for TokenGauge to fetch after the last switch"
            )
        );

        let (_, state) = call(&app, "GET", "/api/state", Value::Null);
        let claude = &state["auto"]["claude"];
        assert_eq!(
            claude["decision"]["action"],
            json!({"kind": "hold", "why": "waiting for TokenGauge to fetch after the last switch"})
        );
        assert_eq!(claude["switched"]["to"], "work", "the last switch is kept");
        assert_eq!(e.live_refresh_token().as_deref(), Some("r-work"));
    }

    #[test]
    fn keep_warm_opens_a_closed_window_once_per_snapshot() {
        let mut server = mockito::Server::new();
        let work = server
            .mock("POST", "/v1/messages")
            .match_query(mockito::Matcher::Any)
            .match_header("authorization", "Bearer a-work")
            .match_body(mockito::Matcher::PartialJson(json!({"max_tokens": 1})))
            .with_body("event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"max_tokens\"}}\n\n")
            .expect(2)
            .create();
        let perso = server
            .mock("POST", "/v1/messages")
            .match_query(mockito::Matcher::Any)
            .match_header("authorization", "Bearer a-perso")
            .with_body("event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"max_tokens\"}}\n\n")
            .expect(1)
            .create();
        let e = env(&server.url());
        e.stored("work", "u-work", HOUR);
        e.stored("perso", "u-perso", HOUR);
        let live = e.store.get("claude", "perso").unwrap().unwrap();
        e.sign_in(live.creds, account("u-perso"));
        std::fs::write(
            places(&e).config,
            "[warm]\nclaude = [\"work\", \"perso\", \"ghost\"]\n",
        )
        .unwrap();
        let app = app(&e, &server.url());
        let heard = gauge_daemon(&e);
        snapshot(&e, crate::fsx::now_ms() - 1000, 3, 3);

        app.warm_round(jiff::Timestamp::now());
        assert!(heard().contains("refresh"));
        let warm = || call(&app, "GET", "/api/state", Value::Null).1["warm"].clone();
        assert_eq!(warm()["claude/work"]["state"], "pinged");
        assert_eq!(warm()["claude/work"]["model"], "claude-haiku-4-5");
        assert_eq!(warm()["claude/perso"]["state"], "pinged");
        assert_eq!(warm()["claude/ghost"]["state"], "waiting");
        assert_eq!(e.live_refresh_token().as_deref(), Some("r-perso"));

        app.warm_round(jiff::Timestamp::now());
        assert_eq!(warm()["claude/work"]["state"], "waiting");
        assert!(warm()["claude/work"]["lastPing"].is_i64());

        std::thread::sleep(std::time::Duration::from_millis(5));
        let asked = |ms: i64| jiff::Timestamp::from_millisecond(ms).unwrap().to_string();
        let usage = |at: String| json!({"updatedAt": at, "primary": {"usedPercent": 0, "windowMinutes": 300}});
        std::fs::write(
            places(&e).snapshot,
            json!({
                "meta": {"schemaVersion": 2, "updatedAtMs": crate::fsx::now_ms()},
                "payloads": [
                    {"provider": "claude", "credential": "perso", "usage": usage(asked(crate::fsx::now_ms() - HOUR))},
                    {"provider": "claude", "credential": "work", "usage": usage(asked(crate::fsx::now_ms()))},
                ],
            })
            .to_string(),
        )
        .unwrap();
        let later = jiff::Timestamp::now() + jiff::SignedDuration::from_mins(6);
        app.warm_round(later);
        assert_eq!(warm()["claude/work"]["state"], "pinged");
        assert_eq!(
            warm()["claude/perso"]["state"],
            "waiting",
            "TokenGauge carried perso's figures from before its ping"
        );
        assert!(heard().contains("refresh"));

        std::fs::write(places(&e).config, "[warm]\nclaude = [\"work\"]\n").unwrap();
        app.warm_round(later);
        assert_eq!(warm()["claude/work"]["state"], "waiting");
        assert!(
            warm().get("claude/perso").is_none(),
            "a login taken off the list is forgotten"
        );
        work.assert();
        perso.assert();
    }

    #[test]
    fn keep_warm_backs_off_after_a_failed_ping_and_keeps_its_error() {
        let mut server = mockito::Server::new();
        let refused = server
            .mock("POST", "/v1/messages")
            .match_query(mockito::Matcher::Any)
            .with_status(429)
            .with_body(
                r#"{"type":"error","error":{"type":"rate_limit_error","message":"slow down"}}"#,
            )
            .expect(2)
            .create();
        let e = env(&server.url());
        e.stored("work", "u-work", HOUR);
        std::fs::write(places(&e).config, "[warm]\nclaude = [\"work\"]\n").unwrap();
        let app = app(&e, &server.url());
        let daemon =
            std::os::unix::net::UnixListener::bind(e.tmp.path().join("tokengauge.sock")).unwrap();
        daemon.set_nonblocking(true).unwrap();
        let warm = || call(&app, "GET", "/api/state", Value::Null).1["warm"]["claude/work"].clone();

        snapshot(&e, crate::fsx::now_ms(), 3, 3);
        app.warm_round(jiff::Timestamp::now());
        assert_eq!(warm()["state"], "failed");
        assert!(warm()["error"].as_str().unwrap().contains("slow down"));
        assert!(daemon.accept().is_err(), "a failed ping nudges nothing");

        snapshot(&e, crate::fsx::now_ms() + 1, 3, 3);
        app.warm_round(jiff::Timestamp::now() + jiff::SignedDuration::from_mins(5));
        assert_eq!(
            warm()["state"],
            "failed",
            "the error stays shown until a retry"
        );
        app.warm_round(jiff::Timestamp::now() + jiff::SignedDuration::from_mins(11));
        assert_eq!(warm()["state"], "failed");
        refused.assert();
        let logged = logbook::read(&e.store);
        assert_eq!(logged.len(), 1, "the same failure twice is one line");
        assert_eq!(
            (logged[0].kind, logged[0].subject().as_str(), logged[0].ok),
            (Kind::Warm, "claude/work", false)
        );
    }

    #[test]
    fn keep_warm_leaves_an_open_window_alone() {
        let e = env(&OFFLINE);
        e.stored("work", "u-work", HOUR);
        std::fs::write(places(&e).config, "[warm]\nclaude = [\"work\"]\n").unwrap();
        let app = app(&e, &OFFLINE);
        let resets = jiff::Timestamp::now() + jiff::SignedDuration::from_hours(2);
        std::fs::write(
            places(&e).snapshot,
            json!({
                "meta": {"schemaVersion": 2, "updatedAtMs": crate::fsx::now_ms()},
                "payloads": [{"provider": "claude", "credential": "work", "usage":
                    {"primary": {"usedPercent": 5, "windowMinutes": 300, "resetsAt": resets.to_string()}}}],
            })
            .to_string(),
        )
        .unwrap();
        app.warm_round(jiff::Timestamp::now());
        let warm = call(&app, "GET", "/api/state", Value::Null).1["warm"].clone();
        assert_eq!(warm["claude/work"]["state"], "open");
    }

    #[test]
    fn auto_mode_off_leaves_the_cli_alone() {
        let e = env(&OFFLINE);
        let app = on_perso(&e, false);
        snapshot(&e, crate::fsx::now_ms(), 100, 10);

        app.auto_round(&Zoned::now());

        assert_eq!(e.live_refresh_token().as_deref(), Some("r-perso"));
        let (_, state) = call(&app, "GET", "/api/state", Value::Null);
        assert_eq!(state["auto"]["claude"], json!({"enabled": false}));
    }

    #[test]
    fn auto_mode_never_drops_a_login_nobody_stored() {
        let e = env(&OFFLINE);
        let app = on_perso(&e, true);
        e.sign_in(oauth("a-new", "r-new", HOUR), account("u-new"));
        snapshot(&e, crate::fsx::now_ms(), 100, 10);

        app.auto_round(&Zoned::now());

        assert_eq!(e.live_refresh_token().as_deref(), Some("r-new"));
        let (_, state) = call(&app, "GET", "/api/state", Value::Null);
        assert_eq!(
            state["auto"]["claude"]["decision"]["action"]["kind"],
            "hold"
        );
    }

    #[test]
    fn a_switch_that_fails_is_reported_and_the_cli_left_as_it_was() {
        let e = env(&OFFLINE);
        let app = on_perso(&e, true);
        e.stored("work", "u-work", 60_000);
        std::fs::write(
            places(&e).config,
            "[auto.nope]\n[auto.claude]\nenabled = true\nlogins = [\"work\", \"perso\"]\n",
        )
        .unwrap();
        snapshot(&e, crate::fsx::now_ms() - 1000, 100, 10);

        app.auto_round(&Zoned::now());
        app.auto_round(&Zoned::now());

        assert_eq!(e.live_refresh_token().as_deref(), Some("r-perso"));
        let logged = logbook::read(&e.store);
        assert_eq!(logged.len(), 1, "{logged:?}");
        assert_eq!(
            (logged[0].kind, logged[0].subject().as_str(), logged[0].ok),
            (Kind::Switch, "claude", false)
        );
        assert!(logged[0].message.starts_with("could not switch to work: "));
        let (_, state) = call(&app, "GET", "/api/state", Value::Null);
        let claude = &state["auto"]["claude"];
        assert_eq!(claude["decision"]["action"]["to"], "work");
        assert!(claude["error"].is_string(), "{claude}");
        assert!(claude.get("switched").is_none());
        assert!(state["auto"].get("nope").is_none());
    }

    #[test]
    fn the_proxy_logs_a_refused_login_once_and_when_it_is_spent_again() {
        let e = env(&OFFLINE);
        e.stored("work", "u-work", HOUR);
        let app = app(&e, &OFFLINE);
        let backend: &dyn proxy::Backend = &app;
        backend.spent("claude", None);
        backend.spent("claude", Some("Claude Code is signed out"));
        backend.spent("claude", Some("Claude Code is signed out"));
        backend.spent("claude", None);
        backend.spent("claude", None);
        let seen: Vec<(Kind, String, bool, String)> = logbook::read(&e.store)
            .into_iter()
            .map(|e| (e.kind, e.subject(), e.ok, e.message))
            .collect();
        assert_eq!(
            seen,
            [
                (
                    Kind::Proxy,
                    "claude".to_owned(),
                    false,
                    "Claude Code is signed out".to_owned()
                ),
                (
                    Kind::Proxy,
                    "claude".to_owned(),
                    true,
                    "spending the active login again".to_owned()
                ),
            ]
        );
    }

    #[test]
    fn a_broken_config_stops_auto_mode_and_says_why() {
        let e = env(&OFFLINE);
        let app = on_perso(&e, true);
        std::fs::write(places(&e).config, "[auto.claude\n").unwrap();

        app.auto_round(&Zoned::now());

        let (_, state) = call(&app, "GET", "/api/state", Value::Null);
        assert!(
            state["auto"]["error"]
                .as_str()
                .unwrap()
                .contains("is not valid")
        );
        assert_eq!(e.live_refresh_token().as_deref(), Some("r-perso"));
    }

    #[test]
    fn a_config_edited_by_hand_reports_what_it_names_wrongly() {
        let e = env(&OFFLINE);
        e.stored("work", "u-work", HOUR);
        let app = app(&e, &OFFLINE);
        std::fs::write(places(&e).config, "[auto.claude]\nlogins = [\"gone\"]\n").unwrap();

        let (status, r) = call(&app, "GET", "/api/config", Value::Null);

        assert_eq!(status, 200);
        assert_eq!(r["problems"][0]["at"], "auto.claude.logins");
        assert_eq!(r["problems"][0]["message"], "no stored login claude/gone");
    }

    #[test]
    fn renaming_and_removing_from_the_page_keep_the_config_in_step() {
        let e = env(&OFFLINE);
        e.stored("work", "u-work", HOUR);
        e.stored("perso", "u-perso", HOUR);
        let app = app(&e, &OFFLINE);
        let path = places(&e).config;
        std::fs::write(&path, "[auto.claude]\nlogins = [\"work\", \"perso\"]\n").unwrap();

        call(
            &app,
            "POST",
            "/api/rename",
            json!({"name": "work", "to": "job"}),
        );
        call(&app, "POST", "/api/remove", json!({"name": "perso"}));

        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "[auto.claude]\nlogins = [\"job\"]\n"
        );
    }

    #[test]
    fn a_config_edit_wakes_the_page() {
        let e = env(&OFFLINE);
        let app = app(&e, &OFFLINE);
        let before = app.fingerprint();
        std::fs::write(places(&e).config, "[schedules.x]\n").unwrap();
        assert_ne!(app.fingerprint(), before);
    }

    #[test]
    fn a_refresh_and_a_switch_ask_tokengauge_to_fetch_now() {
        let e = env(&OFFLINE);
        e.stored("work", "u-work", 3 * HOUR);
        e.stored("perso", "u-perso", 3 * HOUR);
        let work = e.store.get("claude", "work").unwrap().unwrap();
        e.sign_in(work.creds, account("u-work"));
        let app = app(&e, &OFFLINE);
        let heard = gauge_daemon(&e);

        let (status, _) = call(&app, "POST", "/api/refresh", json!({}));
        assert_eq!(status, 200);
        assert_eq!(heard(), "{\"cmd\":\"refresh\"}\n");
        let (status, _) = call(&app, "POST", "/api/use", json!({"name": "perso"}));
        assert_eq!(status, 200);
        assert_eq!(heard(), "{\"cmd\":\"refresh\"}\n");
    }

    #[test]
    fn a_refresh_that_failed_still_asks_tokengauge_to_fetch() {
        let e = env(&OFFLINE);
        e.stored("work", "u-work", 3 * HOUR);
        e.stored("perso", "u-perso", HOUR);
        let work = e.store.get("claude", "work").unwrap().unwrap();
        e.sign_in(work.creds, account("u-work"));
        let app = app(&e, &OFFLINE);
        let heard = gauge_daemon(&e);

        let (status, _) = call(&app, "POST", "/api/refresh", json!({}));
        assert_eq!(status, 400);
        assert_eq!(heard(), "{\"cmd\":\"refresh\"}\n");
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

    fn stored_in_org(e: &Env, name: &str) {
        let mut identity = identity(&format!("u-{name}"));
        identity.oauth_account.as_mut().unwrap()["organizationUuid"] = json!(format!("o-{name}"));
        let creds = oauth(&format!("a-{name}"), &format!("r-{name}"), HOUR);
        e.store
            .save(&crate::store::Entry::new(
                "claude", name, creds, identity, 1,
            ))
            .unwrap();
    }

    const CLI: &str = r"^claude-cli/\S+ \(external, cli\)$";

    fn usage_offering(server: &mut mockito::Server, grant: Value) -> mockito::Mock {
        usage_offering_to(server, "a-work", grant)
    }

    fn usage_offering_to(server: &mut mockito::Server, token: &str, grant: Value) -> mockito::Mock {
        server
            .mock("GET", "/api/oauth/usage")
            .match_query(mockito::Matcher::UrlEncoded(
                "cedar_ember".into(),
                "1".into(),
            ))
            .match_header("authorization", format!("Bearer {token}").as_str())
            .match_header("anthropic-beta", "oauth-2025-04-20")
            .match_header("user-agent", mockito::Matcher::Regex(CLI.into()))
            .with_body(
                json!({"cedar_ember": {"eligible": true, "at_limit": false, "grants": [grant],
                    "next_grant_id": "launch"}})
                .to_string(),
            )
            .create()
    }

    #[test]
    fn a_banked_reset_is_spent_with_its_logins_own_token() {
        let mut server = mockito::Server::new();
        let e = env(&server.url());
        stored_in_org(&e, "work");
        let asked = usage_offering(
            &mut server,
            json!({"id": "launch", "resets_left": 1, "usable_now": true}),
        );
        let spent = server
            .mock("POST", "/api/organizations/o-work/reset_rate_limits")
            .match_header("authorization", "Bearer a-work")
            .match_header("anthropic-beta", "oauth-2025-04-20")
            .match_header("user-agent", mockito::Matcher::Regex(CLI.into()))
            .match_body(mockito::Matcher::AllOf(vec![
                mockito::Matcher::PartialJson(
                    json!({"program": "cedar_ember", "grant_id": "launch"}),
                ),
                mockito::Matcher::Regex(
                    r#""request_id":"[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}""#
                        .into(),
                ),
            ]))
            .with_body(json!({"result": "applied"}).to_string())
            .expect(1)
            .create();
        let nudged = gauge_daemon(&e);
        let app = app(&e, &server.url());

        let (status, r) = call(&app, "POST", "/api/reset", json!({"name": "work"}));
        assert_eq!(status, 200, "{r}");
        assert_eq!(r["message"], "claude/work: banked reset applied");
        asked.assert();
        spent.assert();
        assert_eq!(nudged(), "{\"cmd\":\"refresh\"}\n");
    }

    #[test]
    fn a_reset_the_endpoint_would_refuse_is_never_asked_for() {
        let mut server = mockito::Server::new();
        let e = env(&server.url());
        stored_in_org(&e, "work");
        usage_offering(
            &mut server,
            json!({"id": "launch", "resets_left": 0, "usable_now": false}),
        );
        let spent = server
            .mock("POST", mockito::Matcher::Any)
            .expect(0)
            .create();
        let app = app(&e, &server.url());

        let (status, r) = call(&app, "POST", "/api/reset", json!({"name": "work"}));
        assert_ne!(status, 200);
        assert!(r.to_string().contains("no banked reset"), "{r}");
        spent.assert();
    }

    #[test]
    fn the_login_in_use_is_spent_with_the_token_its_cli_rotated_to() {
        let mut server = mockito::Server::new();
        server
            .mock("GET", "/api/oauth/profile")
            .with_body(profile_body("u-work"))
            .create();
        let e = env(&server.url());
        stored_in_org(&e, "work");
        let mut live_account = account("u-work");
        live_account["organizationUuid"] = json!("o-work");
        e.sign_in(oauth("a-new", "r-new", HOUR), live_account);
        usage_offering_to(
            &mut server,
            "a-new",
            json!({"id": "launch", "resets_left": 1, "usable_now": true}),
        );
        let spent = server
            .mock("POST", "/api/organizations/o-work/reset_rate_limits")
            .match_header("authorization", "Bearer a-new")
            .with_body(json!({"result": "applied"}).to_string())
            .expect(1)
            .create();
        let app = app(&e, &server.url());

        let (status, r) = call(&app, "POST", "/api/reset", json!({"name": "work"}));
        assert_eq!(status, 200, "{r}");
        spent.assert();
        assert_eq!(e.stored_refresh_token("work").as_deref(), Some("r-new"));
    }

    #[test]
    fn a_refused_reset_says_what_the_endpoint_answered_and_nudges_nothing() {
        let mut server = mockito::Server::new();
        let e = env(&server.url());
        stored_in_org(&e, "work");
        usage_offering(
            &mut server,
            json!({"id": "launch", "resets_left": 1, "usable_now": true}),
        );
        server
            .mock("POST", "/api/organizations/o-work/reset_rate_limits")
            .with_status(409)
            .with_body(format!("already used{}", "!".repeat(500)))
            .create();
        let nudges =
            std::os::unix::net::UnixListener::bind(e.tmp.path().join("tokengauge.sock")).unwrap();
        nudges.set_nonblocking(true).unwrap();
        let app = app(&e, &server.url());

        let (status, r) = call(&app, "POST", "/api/reset", json!({"name": "work"}));
        assert_ne!(status, 200);
        let error = r["error"].as_str().unwrap();
        assert!(
            error.contains("409") && error.contains("already used"),
            "{error}"
        );
        assert!(error.len() < 400, "{error}");
        assert!(nudges.accept().is_err(), "a failed reset nudged TokenGauge");
    }

    #[test]
    fn a_login_without_an_organization_spends_nothing() {
        let mut server = mockito::Server::new();
        let e = env(&server.url());
        e.stored("work", "u-work", HOUR);
        let asked = server.mock("GET", mockito::Matcher::Any).expect(0).create();
        let spent = server
            .mock("POST", mockito::Matcher::Any)
            .expect(0)
            .create();
        let app = app(&e, &server.url());

        let (status, r) = call(&app, "POST", "/api/reset", json!({"name": "work"}));
        assert_ne!(status, 200);
        assert!(r.to_string().contains("names no organization"), "{r}");
        asked.assert();
        spent.assert();
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

    #[test]
    fn the_fingerprint_moves_with_the_store_and_the_snapshot_not_with_a_read() {
        let e = env(&OFFLINE);
        e.stored("work", "u-work", HOUR);
        let work = e.store.get("claude", "work").unwrap().unwrap();
        e.sign_in(work.creds, account("u-work"));
        let app = app(&e, &OFFLINE);
        call(&app, "GET", "/api/state", Value::Null);

        let before = app.fingerprint();
        call(&app, "GET", "/api/state", Value::Null);
        assert_eq!(app.fingerprint(), before);

        call(
            &app,
            "POST",
            "/api/label",
            json!({"name": "work", "text": "Job"}),
        );
        let labelled = app.fingerprint();
        assert_ne!(labelled, before);

        std::fs::write(&places(&e).snapshot, "{}").unwrap();
        let snapshot = app.fingerprint();
        assert_ne!(snapshot, labelled);

        let backend: &dyn proxy::Backend = &app;
        backend.spent("claude", Some("signed out"));
        let logged = app.fingerprint();
        assert_ne!(logged, snapshot, "a line in the log wakes the page");
        let snapshot = logged;

        let elsewhere = e.tmp.path().join("kept-elsewhere.json");
        std::fs::write(&elsewhere, "{}").unwrap();
        std::os::unix::fs::symlink(
            &elsewhere,
            e.store.root().join("claude").join("linked.json"),
        )
        .unwrap();
        let linked = app.fingerprint();
        assert_ne!(linked, snapshot);
        std::fs::write(&elsewhere, "{\"changed\": true}").unwrap();
        assert_ne!(app.fingerprint(), linked);
    }
}
