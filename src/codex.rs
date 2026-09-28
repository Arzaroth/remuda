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

use crate::fsx::{now_ms, read_json, rfc3339, write_json};
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

    fn live_identity(&self, creds: &Value) -> Result<Option<Identity>> {
        Ok(identity_of(creds))
    }

    /// The seat is read out of the tokens themselves, not a field beside them,
    /// so no file can disagree with it.
    fn identify(&self, creds: &Value) -> Result<Identity> {
        identity_of(creds).context("the tokens do not name their ChatGPT seat")
    }

    fn install(&self, entry: &Entry) -> Result<()> {
        write_json(&self.auth_path, &entry.creds)
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
        let listener = TcpListener::bind(("127.0.0.1", self.callback_port)).with_context(|| {
            format!(
                "port {} is taken; is `codex login` running?",
                self.callback_port
            )
        })?;
        let port = listener.local_addr()?.port();
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
            listener,
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
    listener: TcpListener,
    redirect_uri: String,
    verifier: String,
    state: String,
    url: String,
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
    /// The query of the first request to `/auth/callback`. Anything else the
    /// browser asks for (a favicon) gets a 404 and the wait goes on.
    fn wait_for_callback(&self) -> Result<(TcpStream, Map<String, Value>)> {
        self.listener.set_nonblocking(true)?;
        let deadline = Instant::now() + LOGIN_TIMEOUT;
        loop {
            let stream = match self.listener.accept() {
                Ok((stream, _)) => stream,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    if self.cancelled.load(Ordering::Relaxed) {
                        bail!("the sign-in was cancelled");
                    }
                    if Instant::now() > deadline {
                        bail!("gave up waiting for the browser to finish signing in");
                    }
                    std::thread::sleep(Duration::from_millis(100));
                    continue;
                }
                Err(e) => return Err(e).context("callback listener failed"),
            };
            stream.set_nonblocking(false)?;
            stream.set_read_timeout(Some(Duration::from_secs(10)))?;
            let mut line = String::new();
            BufReader::new(&stream).read_line(&mut line)?;
            let target = line.split_whitespace().nth(1).unwrap_or_default();
            let url = reqwest::Url::parse(&format!("http://localhost{target}"))?;
            if url.path() != "/auth/callback" {
                respond(&stream, "404 Not Found", &page("Not found."));
                continue;
            }
            let query = url
                .query_pairs()
                .map(|(k, v)| (k.into_owned(), json!(v.into_owned())))
                .collect();
            return Ok((stream, query));
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
            if param("state") != Some(self.state.as_str()) {
                bail!("the callback belongs to another login attempt");
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

    const OFFLINE: &str = "http://127.0.0.1:9";

    #[test]
    fn identity_plan_and_expiry_come_from_the_tokens() {
        let tmp = tempfile::tempdir().unwrap();
        let c = codex(tmp.path(), OFFLINE);
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
        let c = codex(tmp.path(), OFFLINE);
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
        let c = codex(tmp.path(), OFFLINE);
        assert!(c.live().unwrap().is_none());
        write_json(
            &tmp.path().join("auth.json"),
            &json!({"OPENAI_API_KEY": "sk-x", "tokens": null}),
        )
        .unwrap();
        assert!(c.live().unwrap().is_none());
    }

    #[test]
    fn a_rotated_live_login_is_carried_back_by_its_account_id_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::open(&tmp.path().join("store"));
        let c = codex(tmp.path(), OFFLINE);
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
        let c = codex(tmp.path(), OFFLINE);
        for (name, account) in [("work", "acct-1"), ("perso", "acct-2")] {
            let creds = auth(account, name, &format!("r-{name}"), 1_900_000_000);
            let identity = c.live_identity(&creds).unwrap().unwrap();
            store
                .save(&Entry::new("codex", name, creds, identity, 1))
                .unwrap();
        }
        let work = store.get("codex", "work").unwrap().unwrap();
        c.install(&work).unwrap();

        let said = ops::switch(&store, &c, "perso", false).unwrap();

        assert_eq!(said, "switched Codex to perso (acct-2@example.com)");
        let live = read_json(&tmp.path().join("auth.json")).unwrap().unwrap();
        assert_eq!(live, store.get("codex", "perso").unwrap().unwrap().creds);
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
        let exchange = server
            .mock("POST", "/oauth/token")
            .match_body(Matcher::AllOf(vec![
                Matcher::UrlEncoded("grant_type".into(), "authorization_code".into()),
                Matcher::UrlEncoded("code".into(), "the-code".into()),
                Matcher::UrlEncoded("client_id".into(), CLIENT_ID.into()),
            ]))
            .with_body(
                json!({
                    "id_token": id_token("acct-9", "pro"),
                    "access_token": access_token("acct-9", "login", 1_900_000_000),
                    "refresh_token": "r-new",
                })
                .to_string(),
            )
            .create();
        let tmp = tempfile::tempdir().unwrap();
        let c = codex(tmp.path(), &server.url());
        let pending = c.begin_login().unwrap();
        assert!(!pending.needs_code());
        let url = pending.url().to_owned();
        assert!(url.starts_with(&format!("{}/oauth/authorize?", server.url())));
        assert_eq!(param(&url, "code_challenge_method"), "S256");
        let redirect = param(&url, "redirect_uri");
        let state = param(&url, "state");

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
    fn a_callback_from_another_attempt_or_a_refusal_stores_nothing() {
        for (query, says) in [
            ("code=c&state=forged", "another login attempt"),
            (
                "error=access_denied&error_description=no%20thanks",
                "refused: no thanks",
            ),
        ] {
            let tmp = tempfile::tempdir().unwrap();
            let c = codex(tmp.path(), OFFLINE);
            let pending = c.begin_login().unwrap();
            let redirect = param(pending.url(), "redirect_uri");
            let query = query.to_owned();
            let browser = std::thread::spawn(move || {
                browse(&format!("{redirect}?{query}").replace("localhost", "127.0.0.1"))
            });
            let err = pending.finish(None).err().unwrap();
            let page = browser.join().unwrap();
            assert!(err.to_string().contains(says), "{err}");
            assert!(page.starts_with("HTTP/1.1 400"), "{page}");
        }
    }

    #[test]
    fn a_taken_callback_port_says_what_is_probably_holding_it() {
        let held = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = held.local_addr().unwrap().port();
        let tmp = tempfile::tempdir().unwrap();
        let c = Codex::at(tmp.path(), Api::local(OFFLINE), port);
        let err = c.begin_login().err().unwrap();
        assert!(err.to_string().contains("codex login"), "{err}");
    }
}
