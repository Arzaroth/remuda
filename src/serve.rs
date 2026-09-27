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
