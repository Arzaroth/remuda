use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde_json::{Map, Value, json};

use crate::fsx::{Changed, now_ms, read_json, rfc3339, update_json};
use crate::oauth;
use crate::paths;
use crate::pkce;
use crate::provider::{Identity, Login, PendingLogin, Provider};
use crate::store::Entry;

// Mirrors codex-cli 0.153.4's ChatGPT sign-in.
const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const ISSUER: &str = "https://auth.openai.com";
const CALLBACK_PORT: u16 = 1455;
const LOGIN_SCOPE: &str =
    "openid profile email offline_access api.connectors.read api.connectors.invoke";
const REFRESH_SCOPE: &str = "openid profile email";
const LOGIN_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const AUTH_CLAIMS: &str = "https://api.openai.com/auth";

#[derive(Clone)]
pub struct Api {
    client: reqwest::blocking::Client,
    issuer: String,
}

impl Api {
    pub fn openai() -> Result<Self> {
        #[cfg(debug_assertions)]
        if let Some(base) = std::env::var_os("REMUDA_TEST_OPENAI_API") {
            return Self::at(&base.to_string_lossy());
        }
        Self::at(ISSUER)
    }

    fn at(issuer: &str) -> Result<Self> {
        Ok(Api {
            client: oauth::client()?,
            issuer: issuer.to_owned(),
        })
    }

    #[cfg(test)]
    pub fn local(base: &str) -> Self {
        Self::at(base).unwrap()
    }

    fn token_url(&self) -> String {
        format!("{}/oauth/token", self.issuer)
    }
}

fn claims(jwt: &str) -> Option<Value> {
    let payload = jwt.split('.').nth(1)?;
    let bytes = URL_SAFE_NO_PAD.decode(payload.trim_end_matches('=')).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn tokens(creds: &Value) -> Option<&Map<String, Value>> {
    creds.get("tokens").and_then(Value::as_object)
}

fn token<'a>(creds: &'a Value, key: &str) -> Option<&'a str> {
    tokens(creds)?.get(key)?.as_str().filter(|t| !t.is_empty())
}

/// One person in one ChatGPT workspace. `tokens.account_id` names only the
/// workspace, which every seat of a Team plan shares.
fn seat_of(access_token: &str, id_token: Option<&str>) -> Option<String> {
    let from_access = claims(access_token).and_then(|c| {
        auth_claim(&c, "chatgpt_account_user_id")?
            .as_str()
            .map(str::to_owned)
    });
    from_access.or_else(|| {
        let c = claims(id_token?)?;
        let user = auth_claim(&c, "chatgpt_user_id").or_else(|| auth_claim(&c, "user_id"))?;
        let workspace = auth_claim(&c, "chatgpt_account_id")?;
        Some(format!("{}__{}", user.as_str()?, workspace.as_str()?))
    })
}

fn identity_of(creds: &Value) -> Option<Identity> {
    let id_token = token(creds, "id_token");
    let account_id = seat_of(token(creds, "access_token")?, id_token)?;
    let email = id_token
        .and_then(claims)
        .as_ref()
        .and_then(|c| c.get("email"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    Some(Identity {
        account_id,
        email,
        oauth_account: None,
    })
}

fn auth_claim<'a>(claims: &'a Value, key: &str) -> Option<&'a Value> {
    claims.get(AUTH_CLAIMS)?.get(key)
}

/// Codex: the whole of `auth.json` belongs to the login.
pub struct Codex {
    auth_path: PathBuf,
    api: Api,
    callback_port: u16,
}

impl Codex {
    pub fn from_env() -> Result<Self> {
        Ok(Codex {
            auth_path: paths::codex_auth(),
            api: Api::openai()?,
            callback_port: CALLBACK_PORT,
        })
    }

    #[cfg(test)]
    pub fn at(dir: &std::path::Path, api: Api, callback_port: u16) -> Self {
        Codex {
            auth_path: dir.join("auth.json"),
            api,
            callback_port,
        }
    }
}

impl Provider for Codex {
    fn id(&self) -> &'static str {
        "codex"
    }

    fn name(&self) -> &'static str {
        "Codex"
    }

    fn home(&self) -> PathBuf {
        self.auth_path
            .parent()
            .map(PathBuf::from)
            .unwrap_or_default()
    }

    fn access_token<'a>(&self, creds: &'a Value) -> Option<&'a str> {
        token(creds, "access_token")
    }

    fn refresh_token<'a>(&self, creds: &'a Value) -> Option<&'a str> {
        token(creds, "refresh_token")
    }

    fn expires_at(&self, creds: &Value) -> Option<i64> {
        let exp = claims(self.access_token(creds)?)?.get("exp")?.as_i64()?;
        Some(exp * 1000)
    }

    fn plan(&self, creds: &Value) -> String {
        token(creds, "id_token")
            .and_then(claims)
            .and_then(|c| {
                auth_claim(&c, "chatgpt_plan_type")?
                    .as_str()
                    .map(str::to_owned)
            })
            .unwrap_or_else(|| "-".to_owned())
    }

    /// A ChatGPT sign-in only: an API key has no account to switch between.
    fn live(&self) -> Result<Option<Value>> {
        let Some(file) = read_json(&self.auth_path)? else {
            return Ok(None);
        };
        Ok(token(&file, "access_token").is_some().then_some(file))
    }

    fn foreign_login(&self) -> Result<Option<String>> {
        let Some(file) = read_json(&self.auth_path)? else {
            return Ok(None);
        };
        if token(&file, "access_token").is_some() {
            return Ok(None);
        }
        let set = |k: &str| {
            file.get(k)
                .and_then(Value::as_str)
                .is_some_and(|v| !v.is_empty())
        };
        Ok(
            if set("personal_access_token") || set("personalAccessToken") {
                Some("a personal access token".to_owned())
            } else if set("OPENAI_API_KEY") {
                Some("an API key".to_owned())
            } else {
                None
            },
        )
    }

    fn live_identity(&self, creds: &Value) -> Result<Option<Identity>> {
        Ok(identity_of(creds))
    }

    /// The seat is read out of the tokens themselves, not a field beside them,
    /// so no file can disagree with it.
    fn identify(&self, creds: &Value) -> Result<Identity> {
        identity_of(creds).context("the tokens do not name their ChatGPT seat")
    }

    fn install(&self, entry: &Entry, outgoing: Option<&Value>) -> Result<()> {
        let replacement = entry.creds.clone();
        update_json(&self.auth_path, |file| {
            if outgoing.is_some_and(|o| file != o) {
                return Err(Changed.into());
            }
            *file = replacement.clone();
            Ok(())
        })
    }

    /// TokenGauge refreshes the live auth.json in place under this lock.
    fn lock_live(&self) -> Result<Option<std::fs::File>> {
        let path = self.auth_path.with_file_name("auth.json.lock");
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .with_context(|| format!("failed to create {}", dir.display()))?;
        }
        let file = std::fs::File::options()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .with_context(|| format!("failed to open {}", path.display()))?;
        file.lock()
            .with_context(|| format!("failed to lock {}", path.display()))?;
        Ok(Some(file))
    }

    fn refresh(&self, creds: &mut Value) -> Result<Option<String>> {
        let refresh_token = token(creds, "refresh_token")
            .context("credential has no refresh token")?
            .to_owned();
        let answer =
            oauth::token_request(self.api.client.post(self.api.token_url()).json(&json!({
                "client_id": CLIENT_ID,
                "grant_type": "refresh_token",
                "refresh_token": refresh_token,
                "scope": REFRESH_SCOPE,
            })))?;
        let fresh = |k: &str| oauth::text(&answer, k);
        let access = fresh("access_token").context("the token endpoint sent no access token")?;
        let tokens = creds
            .get_mut("tokens")
            .and_then(Value::as_object_mut)
            .context("credential has no tokens")?;
        tokens.insert("access_token".into(), json!(access));
        for key in ["refresh_token", "id_token"] {
            if let Some(v) = fresh(key) {
                tokens.insert(key.into(), json!(v));
            }
        }
        creds["last_refresh"] = json!(rfc3339(now_ms()));
        Ok(seat_of(&access, fresh("id_token").as_deref()))
    }

    fn begin_login(&self) -> Result<Box<dyn PendingLogin>> {
        let listeners = bind_callback(self.callback_port)?;
        let port = listeners[0].local_addr()?.port();
        let redirect_uri = format!("http://localhost:{port}/auth/callback");
        let verifier = pkce::random()?;
        let state = pkce::random()?;
        let url = reqwest::Url::parse_with_params(
            &format!("{}/oauth/authorize", self.api.issuer),
            &[
                ("response_type", "code"),
                ("client_id", CLIENT_ID),
                ("redirect_uri", &redirect_uri),
                ("scope", LOGIN_SCOPE),
                ("code_challenge", &pkce::challenge(&verifier)),
                ("code_challenge_method", "S256"),
                ("id_token_add_organizations", "true"),
                ("codex_cli_simplified_flow", "true"),
                ("state", &state),
                ("originator", "codex_cli_rs"),
            ],
        )?
        .to_string();
        Ok(Box::new(CodexPending {
            api: self.api.clone(),
            cancelled: Arc::new(AtomicBool::new(false)),
            listeners,
            redirect_uri,
            verifier,
            state,
            url,
        }))
    }
}

struct CodexPending {
    api: Api,
    cancelled: Arc<AtomicBool>,
    listeners: Vec<TcpListener>,
    redirect_uri: String,
    verifier: String,
    state: String,
    url: String,
}

/// 127.0.0.1 and, when the machine has it, [::1]: the redirect names
/// `localhost`, which a browser may resolve to either, and a port left free on
/// one of them is a port another local user could answer on. A port still held
/// by an attempt that is shutting down gets a moment to be released.
fn bind_callback(port: u16) -> Result<Vec<TcpListener>> {
    let deadline = Instant::now() + Duration::from_secs(2);
    let v4 = loop {
        match TcpListener::bind(("127.0.0.1", port)) {
            Ok(listener) => break listener,
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse && Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(e) => {
                return Err(e)
                    .with_context(|| format!("port {port} is taken; is `codex login` running?"));
            }
        }
    };
    let port = v4.local_addr()?.port();
    let mut listeners = vec![v4];
    if let Ok(v6) = TcpListener::bind(("::1", port)) {
        listeners.push(v6);
    }
    for listener in &listeners {
        listener.set_nonblocking(true)?;
    }
    Ok(listeners)
}

fn read_request(stream: &TcpStream) -> Option<reqwest::Url> {
    stream.set_nonblocking(false).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(2))).ok()?;
    let mut line = String::new();
    BufReader::new(std::io::Read::take(stream, 8192))
        .read_line(&mut line)
        .ok()?;
    let target = line.split_whitespace().nth(1)?;
    reqwest::Url::parse(&format!("http://localhost{target}")).ok()
}

fn respond(mut stream: &TcpStream, status: &str, body: &str) {
    let _ = write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}

fn page(message: &str) -> String {
    let message = message
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    format!(
        "<!doctype html><meta charset=utf-8><title>remuda</title><body style=\"font:16px system-ui;margin:3rem\"><p>{message}</p>"
    )
}

impl CodexPending {
    /// The query of the first request to `/auth/callback` carrying this
    /// attempt's state. Anything else on the port, whether another path, a
    /// stale or forged state, or a connection that never sends a request, is
    /// answered or dropped and the wait goes on.
    fn wait_for_callback(&self) -> Result<(TcpStream, Map<String, Value>)> {
        let deadline = Instant::now() + LOGIN_TIMEOUT;
        loop {
            let mut accepted = false;
            for listener in &self.listeners {
                let stream = match listener.accept() {
                    Ok((stream, _)) => stream,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => continue,
                    Err(e) => return Err(e).context("callback listener failed"),
                };
                accepted = true;
                let Some(url) = read_request(&stream) else {
                    continue;
                };
                if url.path() != "/auth/callback" {
                    respond(&stream, "404 Not Found", &page("Not found."));
                    continue;
                }
                let query: Map<String, Value> = url
                    .query_pairs()
                    .map(|(k, v)| (k.into_owned(), json!(v.into_owned())))
                    .collect();
                if query.get("state").and_then(Value::as_str) != Some(self.state.as_str()) {
                    respond(
                        &stream,
                        "400 Bad Request",
                        &page("This is not the sign-in remuda is waiting for."),
                    );
                    continue;
                }
                return Ok((stream, query));
            }
            if accepted {
                continue;
            }
            if self.cancelled.load(Ordering::Relaxed) {
                bail!("the sign-in was cancelled");
            }
            if Instant::now() > deadline {
                bail!("gave up waiting for the browser to finish signing in");
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn exchange(&self, code: &str) -> Result<Value> {
        let answer = oauth::token_request(self.api.client.post(self.api.token_url()).form(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", &self.redirect_uri),
            ("client_id", CLIENT_ID),
            ("code_verifier", &self.verifier),
        ]))?;
        let get = |k: &str| {
            oauth::text(&answer, k).with_context(|| format!("the token endpoint sent no {k}"))
        };
        let id_token = get("id_token")?;
        let account_id = claims(&id_token)
            .and_then(|c| {
                auth_claim(&c, "chatgpt_account_id")?
                    .as_str()
                    .map(str::to_owned)
            })
            .context("the id token names no ChatGPT account")?;
        Ok(json!({
            "OPENAI_API_KEY": null,
            "auth_mode": "chatgpt",
            "tokens": {
                "id_token": id_token,
                "access_token": get("access_token")?,
                "refresh_token": get("refresh_token")?,
                "account_id": account_id,
            },
            "last_refresh": rfc3339(now_ms()),
        }))
    }
}

impl PendingLogin for CodexPending {
    fn url(&self) -> &str {
        &self.url
    }

    fn needs_code(&self) -> bool {
        false
    }

    fn canceller(&self) -> Box<dyn Fn() + Send + Sync> {
        let cancelled = Arc::clone(&self.cancelled);
        Box::new(move || cancelled.store(true, Ordering::Relaxed))
    }

    fn finish(self: Box<Self>, _code: Option<&str>) -> Result<Login> {
        let (stream, query) = self.wait_for_callback()?;
        let param = |k: &str| query.get(k).and_then(Value::as_str);
        let outcome = (|| {
            if let Some(error) = param("error") {
                bail!(
                    "the sign-in was refused: {}",
                    param("error_description").unwrap_or(error)
                );
            }
            let code = param("code").context("the callback carried no code")?;
            let creds = self.exchange(code)?;
            let identity = identity_of(&creds).context("the new login names no account")?;
            Ok(Login { creds, identity })
        })();
        match &outcome {
            Ok(_) => respond(
                &stream,
                "200 OK",
                &page("Signed in. remuda has stored this Codex login; you can close this tab."),
            ),
            Err(e) => respond(
                &stream,
                "400 Bad Request",
                &page(&format!("Sign-in failed: {e}")),
            ),
        }
        outcome
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fsx::write_json;
    use crate::ops::testing::OFFLINE;
    use crate::ops::{self, LiveState};
    use crate::store::Store;
    use mockito::Matcher;

    fn jwt(claims: Value) -> String {
        let enc = |v: &Value| URL_SAFE_NO_PAD.encode(v.to_string());
        format!("{}.{}.sig", enc(&json!({"alg": "none"})), enc(&claims))
    }

    const WORKSPACE: &str = "ws-1";

    fn id_token(seat: &str, plan: &str) -> String {
        jwt(json!({
            "email": format!("{seat}@example.com"),
            AUTH_CLAIMS: {"chatgpt_account_id": WORKSPACE, "chatgpt_user_id": format!("user-{seat}"), "chatgpt_plan_type": plan},
        }))
    }

    fn access_token(seat: &str, tag: &str, exp_secs: i64) -> String {
        jwt(json!({
            "exp": exp_secs,
            "tag": tag,
            AUTH_CLAIMS: {"chatgpt_account_user_id": seat, "chatgpt_account_id": WORKSPACE},
        }))
    }

    fn auth(seat: &str, access: &str, refresh: &str, exp_secs: i64) -> Value {
        json!({
            "OPENAI_API_KEY": null,
            "auth_mode": "chatgpt",
            "tokens": {
                "id_token": id_token(seat, "plus"),
                "access_token": access_token(seat, access, exp_secs),
                "refresh_token": refresh,
                "account_id": WORKSPACE,
            },
            "last_refresh": "2026-09-01T00:00:00Z",
        })
    }

    fn codex(dir: &std::path::Path, base: &str) -> Codex {
        Codex::at(dir, Api::local(base), 0)
    }

    #[test]
    fn identity_plan_and_expiry_come_from_the_tokens() {
        let tmp = tempfile::tempdir().unwrap();
        let c = codex(tmp.path(), &OFFLINE);
        let creds = auth("acct-1", "a", "r", 1_900_000_000);
        let id = c.live_identity(&creds).unwrap().unwrap();
        assert_eq!(id.account_id, "acct-1");
        assert_eq!(id.email, "acct-1@example.com");
        assert!(id.oauth_account.is_none());
        assert_eq!(c.plan(&creds), "plus");
        assert_eq!(c.expires_at(&creds), Some(1_900_000_000_000));
        assert_eq!(c.refresh_token(&creds), Some("r"));
        assert_eq!(c.identify(&creds).unwrap().account_id, "acct-1");

        let mut older = creds.clone();
        older["tokens"]["access_token"] = json!(jwt(json!({"exp": 1_900_000_000})));
        assert_eq!(c.identify(&older).unwrap().account_id, "user-acct-1__ws-1");
        older["tokens"]["id_token"] = json!(jwt(json!({})));
        assert!(c.identify(&older).is_err());
        assert_eq!(c.plan(&json!({})), "-");
    }

    #[test]
    fn two_seats_of_one_workspace_are_two_accounts() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::open(&tmp.path().join("store"));
        let c = codex(tmp.path(), &OFFLINE);
        let alice = auth("alice", "a", "r-alice", 1_900_000_000);
        let identity = c.live_identity(&alice).unwrap().unwrap();
        store
            .save(&Entry::new("codex", "alice", alice, identity, 1))
            .unwrap();
        write_json(
            &tmp.path().join("auth.json"),
            &auth("bob", "b", "r-bob", 1_900_000_000),
        )
        .unwrap();

        let state = ops::sync_live(&store, &c).unwrap();

        assert!(matches!(state, LiveState::Unstored { .. }), "{state:?}");
        let alice = store.get("codex", "alice").unwrap().unwrap();
        assert_eq!(c.refresh_token(&alice.creds), Some("r-alice"));
    }

    #[test]
    fn an_api_key_login_is_not_a_login_to_switch() {
        let tmp = tempfile::tempdir().unwrap();
        let c = codex(tmp.path(), &OFFLINE);
        assert!(c.live().unwrap().is_none());
        assert!(c.foreign_login().unwrap().is_none());
        let key = json!({"OPENAI_API_KEY": "sk-x", "tokens": null});
        write_json(&tmp.path().join("auth.json"), &key).unwrap();
        assert!(c.live().unwrap().is_none());
        assert_eq!(c.foreign_login().unwrap().as_deref(), Some("an API key"));

        let store = Store::open(&tmp.path().join("store"));
        let creds = auth("acct-1", "a", "r", 1_900_000_000);
        let identity = c.live_identity(&creds).unwrap().unwrap();
        store
            .save(&Entry::new("codex", "work", creds, identity, 1))
            .unwrap();
        let err = ops::switch(&store, &c, "work", false).unwrap_err();
        assert!(err.to_string().contains("an API key"), "{err}");
        assert_eq!(
            read_json(&tmp.path().join("auth.json")).unwrap().unwrap(),
            key
        );
        ops::switch(&store, &c, "work", true).unwrap();
        assert!(c.live().unwrap().is_some());

        write_json(
            &tmp.path().join("auth.json"),
            &json!({"personal_access_token": "pat-x"}),
        )
        .unwrap();
        assert_eq!(
            c.foreign_login().unwrap().as_deref(),
            Some("a personal access token")
        );
    }

    #[test]
    fn a_rotated_live_login_is_carried_back_by_its_account_id_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::open(&tmp.path().join("store"));
        let c = codex(tmp.path(), &OFFLINE);
        let stored = auth("acct-1", "a1", "r1", 1_900_000_000);
        let identity = c.live_identity(&stored).unwrap().unwrap();
        store
            .save(&Entry::new("codex", "work", stored, identity, 1))
            .unwrap();
        write_json(
            &tmp.path().join("auth.json"),
            &auth("acct-1", "a2", "r2", 1_900_000_000),
        )
        .unwrap();

        let state = ops::sync_live(&store, &c).unwrap();

        assert_eq!(
            state,
            LiveState::Stored {
                name: "work".into(),
                synced: true
            }
        );
        let work = store.get("codex", "work").unwrap().unwrap();
        assert_eq!(c.refresh_token(&work.creds), Some("r2"));
    }

    #[test]
    fn switching_replaces_auth_json_whole() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::open(&tmp.path().join("store"));
        let c = codex(tmp.path(), &OFFLINE);
        for (name, account) in [("work", "acct-1"), ("perso", "acct-2")] {
            let creds = auth(account, name, &format!("r-{name}"), 1_900_000_000);
            let identity = c.live_identity(&creds).unwrap().unwrap();
            store
                .save(&Entry::new("codex", name, creds, identity, 1))
                .unwrap();
        }
        let work = store.get("codex", "work").unwrap().unwrap();
        c.install(&work, None).unwrap();

        let said = ops::switch(&store, &c, "perso", false).unwrap();

        assert_eq!(said, "switched Codex to perso (acct-2@example.com)");
        assert!(tmp.path().join("auth.json.lock").exists());
        let held = c.lock_live().unwrap().unwrap();
        assert!(matches!(
            std::fs::File::open(tmp.path().join("auth.json.lock"))
                .unwrap()
                .try_lock(),
            Err(std::fs::TryLockError::WouldBlock)
        ));
        drop(held);
        let live = read_json(&tmp.path().join("auth.json")).unwrap().unwrap();
        assert_eq!(live, store.get("codex", "perso").unwrap().unwrap().creds);
    }

    #[test]
    fn a_codex_install_refuses_an_auth_json_that_moved() {
        let tmp = tempfile::tempdir().unwrap();
        let c = codex(tmp.path(), &OFFLINE);
        let seen = auth("acct-1", "a", "r1", 1_900_000_000);
        let rotated = auth("acct-1", "a", "r2", 1_900_000_000);
        write_json(&tmp.path().join("auth.json"), &rotated).unwrap();
        let other = auth("acct-2", "b", "s", 1_900_000_000);
        let identity = c.live_identity(&other).unwrap().unwrap();
        let entry = Entry::new("codex", "other", other, identity, 1);

        let err = c.install(&entry, Some(&seen)).unwrap_err();

        assert!(err.is::<Changed>(), "{err}");
        assert_eq!(
            read_json(&tmp.path().join("auth.json")).unwrap().unwrap(),
            rotated
        );
        c.install(&entry, Some(&rotated)).unwrap();
        assert_eq!(c.refresh_token(&c.live().unwrap().unwrap()), Some("s"));
    }

    #[test]
    fn a_refresh_keeps_what_the_endpoint_leaves_out() {
        let mut server = mockito::Server::new();
        let mock = server
            .mock("POST", "/oauth/token")
            .match_body(Matcher::PartialJson(json!({
                "grant_type": "refresh_token",
                "refresh_token": "r1",
                "client_id": CLIENT_ID,
            })))
            .with_body(
                json!({"access_token": access_token("acct-1", "new", 1_900_000_000), "id_token": id_token("acct-1", "pro")})
                    .to_string(),
            )
            .create();
        let tmp = tempfile::tempdir().unwrap();
        let c = codex(tmp.path(), &server.url());
        let mut creds = auth("acct-1", "a1", "r1", 1);

        let account = c.refresh(&mut creds).unwrap();

        mock.assert();
        assert_eq!(account.as_deref(), Some("acct-1"));
        assert_eq!(
            creds["tokens"]["access_token"],
            access_token("acct-1", "new", 1_900_000_000)
        );
        assert_eq!(creds["tokens"]["refresh_token"], "r1");
        assert_eq!(c.plan(&creds), "pro");
        assert_ne!(creds["last_refresh"], "2026-09-01T00:00:00Z");
    }

    #[test]
    fn a_refused_refresh_leaves_the_tokens_alone() {
        let mut server = mockito::Server::new();
        server
            .mock("POST", "/oauth/token")
            .with_status(401)
            .with_body("refresh_token_reused")
            .create();
        let tmp = tempfile::tempdir().unwrap();
        let c = codex(tmp.path(), &server.url());
        let mut creds = auth("acct-1", "a1", "r1", 1);
        let before = creds.clone();
        let err = c.refresh(&mut creds).unwrap_err();
        assert!(
            err.to_string().contains("sign this credential in again"),
            "{err}"
        );
        assert_eq!(creds, before);
    }

    fn browse(url: &str) -> String {
        let url = reqwest::Url::parse(url).unwrap();
        let mut stream = TcpStream::connect(("127.0.0.1", url.port().unwrap())).unwrap();
        let target = match url.query() {
            Some(q) => format!("{}?{q}", url.path()),
            None => url.path().to_owned(),
        };
        write!(stream, "GET {target} HTTP/1.1\r\nHost: localhost\r\n\r\n").unwrap();
        let mut answer = String::new();
        std::io::Read::read_to_string(&mut stream, &mut answer).unwrap();
        answer
    }

    fn param(url: &str, key: &str) -> String {
        reqwest::Url::parse(url)
            .unwrap()
            .query_pairs()
            .find(|(k, _)| k == key)
            .unwrap()
            .1
            .into_owned()
    }

    #[test]
    fn a_login_completes_when_the_browser_calls_back() {
        let mut server = mockito::Server::new();
        let tmp = tempfile::tempdir().unwrap();
        let c = codex(tmp.path(), &server.url());
        let pending = c.begin_login().unwrap();
        assert!(!pending.needs_code());
        let url = pending.url().to_owned();
        assert!(url.starts_with(&format!("{}/oauth/authorize?", server.url())));
        assert_eq!(param(&url, "code_challenge_method"), "S256");
        let redirect = param(&url, "redirect_uri");
        let state = param(&url, "state");
        let challenge = param(&url, "code_challenge");
        let exchange = server
            .mock("POST", "/oauth/token")
            .match_body(Matcher::AllOf(vec![
                Matcher::UrlEncoded("grant_type".into(), "authorization_code".into()),
                Matcher::UrlEncoded("code".into(), "the-code".into()),
                Matcher::UrlEncoded("client_id".into(), CLIENT_ID.into()),
                Matcher::UrlEncoded("redirect_uri".into(), redirect.clone()),
            ]))
            .with_body_from_request(move |req| {
                let body = req.utf8_lossy_body().unwrap().into_owned();
                let verifier = reqwest::Url::parse(&format!("http://x/?{body}"))
                    .unwrap()
                    .query_pairs()
                    .find(|(k, _)| k == "code_verifier")
                    .map(|(_, v)| v.into_owned())
                    .unwrap_or_default();
                if pkce::challenge(&verifier) != challenge {
                    return b"{}".to_vec();
                }
                json!({
                    "id_token": id_token("acct-9", "pro"),
                    "access_token": access_token("acct-9", "login", 1_900_000_000),
                    "refresh_token": "r-new",
                })
                .to_string()
                .into_bytes()
            })
            .create();

        let browser = std::thread::spawn(move || {
            let port = reqwest::Url::parse(&redirect).unwrap().port().unwrap();
            let base = format!("http://127.0.0.1:{port}");
            let missing = browse(&format!("{base}/favicon.ico"));
            let done = browse(&format!("{base}/auth/callback?code=the-code&state={state}"));
            (missing, done)
        });
        let login = pending.finish(None).unwrap();
        let (missing, done) = browser.join().unwrap();

        exchange.assert();
        assert!(missing.starts_with("HTTP/1.1 404"), "{missing}");
        assert!(done.starts_with("HTTP/1.1 200"), "{done}");
        assert!(done.contains("Signed in"), "{done}");
        assert_eq!(login.identity.account_id, "acct-9");
        assert_eq!(login.creds["tokens"]["refresh_token"], "r-new");
        assert_eq!(login.creds["tokens"]["account_id"], WORKSPACE);
        assert_eq!(login.creds["auth_mode"], "chatgpt");
        assert_eq!(c.plan(&login.creds), "pro");
    }

    #[test]
    fn stray_connections_do_not_end_the_wait_and_a_refusal_does() {
        let tmp = tempfile::tempdir().unwrap();
        let c = codex(tmp.path(), &OFFLINE);
        let pending = c.begin_login().unwrap();
        let redirect = param(pending.url(), "redirect_uri").replace("localhost", "127.0.0.1");
        let state = param(pending.url(), "state");
        let browser = std::thread::spawn(move || {
            let port = reqwest::Url::parse(&redirect).unwrap().port().unwrap();
            drop(TcpStream::connect(("127.0.0.1", port)).unwrap());
            let mut garbage = TcpStream::connect(("127.0.0.1", port)).unwrap();
            garbage.write_all(b"\x00\x01 nonsense\r\n").unwrap();
            drop(garbage);
            let forged = browse(&format!("{redirect}?code=c&state=forged"));
            let refused = browse(&format!(
                "{redirect}?error=access_denied&error_description=no%20thanks&state={state}"
            ));
            (forged, refused)
        });
        let err = pending.finish(None).err().unwrap();
        let (forged, refused) = browser.join().unwrap();
        assert!(forged.starts_with("HTTP/1.1 400"), "{forged}");
        assert!(forged.contains("not the sign-in"), "{forged}");
        assert!(err.to_string().contains("refused: no thanks"), "{err}");
        assert!(refused.starts_with("HTTP/1.1 400"), "{refused}");
    }

    #[test]
    fn the_cli_login_waits_for_the_callback_without_asking_for_a_code() {
        let mut server = mockito::Server::new();
        server
            .mock("POST", "/oauth/token")
            .with_body(
                json!({
                    "id_token": id_token("seat-9", "pro"),
                    "access_token": access_token("seat-9", "login", 1_900_000_000),
                    "refresh_token": "r-new",
                })
                .to_string(),
            )
            .create();
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::open(&tmp.path().join("store"));
        let c = codex(tmp.path(), &server.url());
        let open = |url: &str| {
            let redirect = param(url, "redirect_uri").replace("localhost", "127.0.0.1");
            let state = param(url, "state");
            std::thread::spawn(move || browse(&format!("{redirect}?code=c&state={state}")));
            true
        };
        let mut out = Vec::new();
        crate::commands::login(
            &store,
            &c,
            "nine",
            false,
            crate::commands::Prompt {
                open: &open,
                read_code: &mut || unreachable!(),
            },
            &mut out,
        )
        .unwrap();
        let out = String::from_utf8(out).unwrap();
        assert!(
            out.starts_with("Opened a private window to sign in to"),
            "{out}"
        );
        assert!(out.contains("Waiting for the browser"), "{out}");
        assert!(
            out.ends_with("stored codex/nine (seat-9@example.com, pro)\n"),
            "{out}"
        );
    }

    #[test]
    fn an_unstored_codex_login_is_listed_with_its_own_import_hint() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::open(&tmp.path().join("store"));
        let c = codex(tmp.path(), &OFFLINE);
        write_json(
            &tmp.path().join("auth.json"),
            &auth("seat-1", "a", "r", 1_900_000_000),
        )
        .unwrap();
        let providers: [&dyn Provider; 1] = [&c];
        let lives = crate::commands::sync_all(&store, &providers).unwrap();
        let mut out = Vec::new();
        crate::commands::list(&store, &lives, false, &mut out).unwrap();
        let out = String::from_utf8(out).unwrap();
        assert!(out.contains("`remuda import -p codex <name>`"), "{out}");
    }

    #[test]
    fn the_callback_answers_on_both_loopbacks() {
        if TcpListener::bind(("::1", 0)).is_err() {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let c = codex(tmp.path(), &OFFLINE);
        let pending = c.begin_login().unwrap();
        let port = reqwest::Url::parse(&param(pending.url(), "redirect_uri"))
            .unwrap()
            .port()
            .unwrap();
        assert!(TcpStream::connect(("127.0.0.1", port)).is_ok());
        assert!(TcpStream::connect(("::1", port)).is_ok());
    }

    #[test]
    fn a_taken_callback_port_says_what_is_probably_holding_it() {
        let held = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = held.local_addr().unwrap().port();
        let tmp = tempfile::tempdir().unwrap();
        let c = Codex::at(tmp.path(), Api::local(&OFFLINE), port);
        let err = c.begin_login().err().unwrap();
        assert!(err.to_string().contains("codex login"), "{err}");
    }
}
