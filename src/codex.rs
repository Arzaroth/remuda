use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde_json::{Map, Value, json};

use crate::callback::{Callback, page, respond};
use crate::fsx::{Changed, lock_file, now_ms, read_json, rfc3339, update_json};
use crate::oauth::{self, claims};
use crate::paths;
use crate::pkce;
use crate::provider::{Format, Identity, Login, PendingLogin, Provider, RunSpec, Upstream};
use crate::store::Entry;

const MIRRORS: &str = "0.167.0";
const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const ISSUER: &str = "https://auth.openai.com";
const CALLBACK_PORT: u16 = 1455;
const LOGIN_SCOPE: &str =
    "openid profile email offline_access api.connectors.read api.connectors.invoke";
const REFRESH_SCOPE: &str = "openid profile email";
const AUTH_CLAIMS: &str = "https://api.openai.com/auth";
const INFERENCE: &str = "https://chatgpt.com/backend-api/codex";
const DEVICE_WAIT: Duration = Duration::from_secs(15 * 60);

#[derive(Clone)]
pub struct Api {
    client: reqwest::blocking::Client,
    issuer: String,
    inference: String,
}

impl Api {
    pub fn openai() -> Result<Self> {
        #[cfg(debug_assertions)]
        if let Some(base) = std::env::var_os("REMUDA_TEST_OPENAI_API") {
            let base = base.to_string_lossy();
            return Self::at(&base, &base);
        }
        Self::at(ISSUER, INFERENCE)
    }

    fn at(issuer: &str, inference: &str) -> Result<Self> {
        Ok(Api {
            client: oauth::client()?,
            issuer: issuer.to_owned(),
            inference: inference.to_owned(),
        })
    }

    #[cfg(test)]
    pub fn local(base: &str) -> Self {
        Self::at(base, base).unwrap()
    }

    fn token_url(&self) -> String {
        format!("{}/oauth/token", self.issuer)
    }

    fn device_url(&self, step: &str) -> String {
        format!("{}/api/accounts/deviceauth/{step}", self.issuer)
    }

    fn post_json(&self, url: &str, body: &Value) -> Result<(reqwest::StatusCode, String)> {
        let resp = self
            .client
            .post(url)
            .json(body)
            .send()
            .context("device sign-in request failed")?;
        let status = resp.status();
        Ok((status, resp.text().unwrap_or_default()))
    }
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

    fn live_file(&self) -> Option<PathBuf> {
        Some(self.auth_path.clone())
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

    fn signs_out(&self) -> bool {
        true
    }

    fn sign_out(&self, outgoing: &Value) -> Result<()> {
        crate::fsx::remove_json_if(&self.auth_path, outgoing)
    }

    /// Codex never re-reads auth.json while it runs, and a refresh in flight
    /// writes the old account's tokens into the new file.
    fn sessions(&self) -> &'static [&'static str] {
        &["codex"]
    }

    /// TokenGauge refreshes the live auth.json in place under this lock.
    fn lock_live(&self) -> Result<Option<std::fs::File>> {
        lock_file(&self.auth_path.with_file_name("auth.json.lock")).map(Some)
    }

    fn mirrors(&self) -> Option<&'static str> {
        Some(MIRRORS)
    }

    fn run_spec(&self) -> Option<RunSpec> {
        Some(RunSpec {
            command: "codex",
            var: "CODEX_HOME",
            copied: Vec::new(),
            private: &["auth.json", "auth.json.lock"],
        })
    }

    fn relocated(&self, dir: &std::path::Path) -> Option<Box<dyn Provider>> {
        Some(Box::new(Codex::at(
            dir,
            self.api.clone(),
            self.callback_port,
        )))
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

    fn begin_login(&self, paste: bool) -> Result<Box<dyn PendingLogin>> {
        if paste {
            return CodexDevice::begin(&self.api);
        }
        let state = pkce::random()?;
        let callback = Callback::bind(
            self.callback_port,
            "/auth/callback",
            state.clone(),
            "is `codex login` running?",
        )?;
        let redirect_uri = format!("http://localhost:{}/auth/callback", callback.port());
        let verifier = pkce::random()?;
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
            callback,
            redirect_uri,
            verifier,
            url,
        }))
    }

    fn formats(&self, _model: &str) -> Vec<Format> {
        vec![Format::Responses]
    }

    fn upstream(&self, creds: &Value, format: Format, _model: &str) -> Option<Upstream> {
        if format != Format::Responses {
            return None;
        }
        let mut headers = vec![
            (
                "authorization",
                format!("Bearer {}", self.access_token(creds)?),
            ),
            ("originator", "codex_cli_rs".to_owned()),
            ("accept", "text/event-stream".to_owned()),
        ];
        if let Some(workspace) = token(creds, "account_id") {
            headers.push(("chatgpt-account-id", workspace.to_owned()));
        }
        Some(Upstream {
            url: format!("{}/responses", self.api.inference),
            headers,
        })
    }

    /// The ChatGPT backend takes only streamed, unstored responses, and
    /// refuses the API's sampling and storage fields. A client that did not
    /// ask for a stream gets the whole answer collected from it.
    fn shape(&self, _format: Format, body: &mut Value, _user_agent: Option<&str>) -> Result<()> {
        let Some(fields) = body.as_object_mut() else {
            bail!("the request body is not an object");
        };
        fields.insert("stream".to_owned(), json!(true));
        fields.insert("store".to_owned(), json!(false));
        fields.entry("instructions").or_insert_with(|| json!(""));
        for drop in [
            "previous_response_id",
            "max_output_tokens",
            "max_completion_tokens",
            "temperature",
            "top_p",
            "stream_options",
            "safety_identifier",
            "prompt_cache_retention",
        ] {
            fields.remove(drop);
        }
        Ok(())
    }
}

struct CodexPending {
    api: Api,
    callback: Callback,
    redirect_uri: String,
    verifier: String,
    url: String,
}

fn exchange(api: &Api, redirect_uri: &str, verifier: &str, code: &str) -> Result<Value> {
    let answer = oauth::token_request(api.client.post(api.token_url()).form(&[
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", redirect_uri),
        ("client_id", CLIENT_ID),
        ("code_verifier", verifier),
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

impl PendingLogin for CodexPending {
    fn url(&self) -> &str {
        &self.url
    }

    fn needs_code(&self) -> bool {
        false
    }

    fn canceller(&self) -> Box<dyn Fn() + Send + Sync> {
        self.callback.canceller()
    }

    fn local_only(&self) -> bool {
        true
    }

    fn finish(self: Box<Self>, _code: Option<&str>) -> Result<Login> {
        let (stream, query) = self.callback.wait()?;
        let param = |k: &str| query.get(k).and_then(Value::as_str);
        let outcome = (|| {
            if let Some(error) = param("error") {
                bail!(
                    "the sign-in was refused: {}",
                    param("error_description").unwrap_or(error)
                );
            }
            let code = param("code").context("the callback carried no code")?;
            let creds = exchange(&self.api, &self.redirect_uri, &self.verifier, code)?;
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

struct CodexDevice {
    api: Api,
    device_auth_id: String,
    user_code: String,
    url: String,
    interval: Duration,
    cancelled: Arc<AtomicBool>,
}

impl CodexDevice {
    fn begin(api: &Api) -> Result<Box<dyn PendingLogin>> {
        let (status, body) = api.post_json(
            &api.device_url("usercode"),
            &json!({"client_id": CLIENT_ID}),
        )?;
        if status == reqwest::StatusCode::NOT_FOUND {
            bail!(
                "device code login is not enabled for this Codex server ({status}): sign in by callback instead, without --paste or `paste = true` under [login], or with --no-paste"
            );
        }
        if !status.is_success() {
            bail!(
                "the device code request answered {status}: {}",
                oauth::excerpt(&body)
            );
        }
        let answer: Value =
            serde_json::from_str(&body).context("malformed device code response")?;
        let device_auth_id =
            oauth::text(&answer, "device_auth_id").context("no device code was issued")?;
        let user_code = ["user_code", "usercode"]
            .iter()
            .find_map(|k| oauth::text(&answer, k))
            .context("no user code was issued")?;
        let interval = answer
            .get("interval")
            .and_then(|v| v.as_u64().or_else(|| v.as_str()?.trim().parse().ok()))
            .unwrap_or(5)
            .max(1);
        Ok(Box::new(CodexDevice {
            api: api.clone(),
            device_auth_id,
            user_code,
            url: format!("{}/codex/device", api.issuer),
            interval: Duration::from_secs(interval),
            cancelled: Arc::new(AtomicBool::new(false)),
        }))
    }

    fn poll(&self) -> Result<Value> {
        let deadline = Instant::now() + DEVICE_WAIT;
        let body = json!({"device_auth_id": self.device_auth_id, "user_code": self.user_code});
        let mut errors = 0;
        loop {
            crate::device::pause(&self.cancelled, self.interval)?;
            if Instant::now() >= deadline {
                bail!("the sign-in was not approved in time");
            }
            let (status, answer) = match self.api.post_json(&self.api.device_url("token"), &body) {
                Ok(sent) => sent,
                Err(e) => {
                    errors += 1;
                    if errors >= 3 {
                        return Err(e);
                    }
                    continue;
                }
            };
            if crate::device::transient(status) {
                errors += 1;
                if errors < 3 {
                    continue;
                }
            } else {
                errors = 0;
            }
            if status == reqwest::StatusCode::FORBIDDEN || status == reqwest::StatusCode::NOT_FOUND
            {
                continue;
            }
            if !status.is_success() {
                bail!(
                    "the device sign-in answered {status}: {}",
                    oauth::excerpt(&answer)
                );
            }
            return serde_json::from_str(&answer).context("malformed device sign-in response");
        }
    }
}

impl PendingLogin for CodexDevice {
    fn url(&self) -> &str {
        &self.url
    }

    fn needs_code(&self) -> bool {
        false
    }

    fn user_code(&self) -> Option<&str> {
        Some(&self.user_code)
    }

    fn canceller(&self) -> Box<dyn Fn() + Send + Sync> {
        let cancelled = self.cancelled.clone();
        Box::new(move || cancelled.store(true, Ordering::Relaxed))
    }

    fn finish(self: Box<Self>, _code: Option<&str>) -> Result<Login> {
        let answer = self.poll()?;
        let get = |k: &str| {
            oauth::text(&answer, k).with_context(|| format!("the device sign-in sent no {k}"))
        };
        let creds = exchange(
            &self.api,
            &format!("{}/deviceauth/callback", self.api.issuer),
            &get("code_verifier")?,
            &get("authorization_code")?,
        )?;
        let identity = identity_of(&creds).context("the new login names no account")?;
        Ok(Login { creds, identity })
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
    use std::io::Write;
    use std::net::{TcpListener, TcpStream};

    use crate::oauth::fake_jwt as jwt;

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
    fn signing_out_removes_auth_json_once_its_login_is_stored() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::open(&tmp.path().join("store"));
        let c = codex(tmp.path(), &OFFLINE);
        let creds = auth("acct-1", "work", "r-work", 1_900_000_000);
        let identity = c.live_identity(&creds).unwrap().unwrap();
        store
            .save(&Entry::new("codex", "work", creds, identity, 1))
            .unwrap();
        let work = store.get("codex", "work").unwrap().unwrap();
        c.install(&work, None).unwrap();

        let said = ops::sign_out(&store, &c, false, crate::logbook::By::Cli).unwrap();

        assert_eq!(said, "signed Codex out of work");
        assert!(!tmp.path().join("auth.json").exists());
        assert!(store.get("codex", "work").unwrap().is_some());
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
        let pending = c.begin_login(false).unwrap();
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
    fn a_device_sign_in_exchanges_the_approval_with_the_verifier_it_carries() {
        let mut server = mockito::Server::new();
        let tmp = tempfile::tempdir().unwrap();
        let c = codex(tmp.path(), &server.url());
        let usercode = server
            .mock("POST", "/api/accounts/deviceauth/usercode")
            .match_body(Matcher::Json(json!({"client_id": CLIENT_ID})))
            .with_body(
                json!({"device_auth_id": "da-1", "usercode": "ABCD-1234", "interval": "1"})
                    .to_string(),
            )
            .create();
        let not_yet: Vec<_> = [403, 404]
            .into_iter()
            .map(|status| {
                server
                    .mock("POST", "/api/accounts/deviceauth/token")
                    .with_status(status)
                    .expect(1)
                    .create()
            })
            .collect();
        let approved = server
            .mock("POST", "/api/accounts/deviceauth/token")
            .match_body(Matcher::Json(json!({"device_auth_id": "da-1", "user_code": "ABCD-1234"})))
            .with_body(
                json!({"authorization_code": "auth-1", "code_challenge": "ch", "code_verifier": "ver-1"})
                    .to_string(),
            )
            .create();
        let exchange = server
            .mock("POST", "/oauth/token")
            .match_body(Matcher::AllOf(vec![
                Matcher::UrlEncoded("grant_type".into(), "authorization_code".into()),
                Matcher::UrlEncoded("code".into(), "auth-1".into()),
                Matcher::UrlEncoded("code_verifier".into(), "ver-1".into()),
                Matcher::UrlEncoded(
                    "redirect_uri".into(),
                    format!("{}/deviceauth/callback", server.url()),
                ),
            ]))
            .with_body(
                json!({
                    "id_token": id_token("acct-9", "plus"),
                    "access_token": access_token("acct-9", "device", 1_900_000_000),
                    "refresh_token": "r-device",
                })
                .to_string(),
            )
            .create();

        let pending = c.begin_login(true).unwrap();
        assert!(!pending.needs_code());
        assert!(!pending.local_only());
        assert_eq!(pending.url(), format!("{}/codex/device", server.url()));
        assert_eq!(pending.user_code(), Some("ABCD-1234"));
        let login = pending.finish(None).unwrap();

        usercode.assert();
        not_yet.iter().for_each(|m| m.assert());
        approved.assert();
        exchange.assert();
        assert_eq!(login.identity.account_id, "acct-9");
        assert_eq!(login.creds["tokens"]["refresh_token"], "r-device");
        assert_eq!(c.plan(&login.creds), "plus");
    }

    #[test]
    fn a_device_sign_in_stops_on_an_error_and_on_a_cancel() {
        let mut server = mockito::Server::new();
        let tmp = tempfile::tempdir().unwrap();
        let c = codex(tmp.path(), &server.url());
        server
            .mock("POST", "/api/accounts/deviceauth/usercode")
            .with_body(
                json!({"device_auth_id": "da-1", "user_code": "WXYZ", "interval": 1}).to_string(),
            )
            .create();
        server
            .mock("POST", "/api/accounts/deviceauth/token")
            .with_status(400)
            .with_body("denied")
            .create();
        let err = c.begin_login(true).unwrap().finish(None).err().unwrap();
        assert!(err.to_string().contains("400"), "{err}");

        let pending = c.begin_login(true).unwrap();
        pending.canceller()();
        let err = pending.finish(None).err().unwrap();
        assert!(err.to_string().contains("cancelled"), "{err}");
    }

    #[test]
    fn a_device_sign_in_rides_out_two_busy_answers_but_not_a_third() {
        let mut server = mockito::Server::new();
        let tmp = tempfile::tempdir().unwrap();
        let c = codex(tmp.path(), &server.url());
        server
            .mock("POST", "/api/accounts/deviceauth/usercode")
            .with_body(
                json!({"device_auth_id": "da-1", "user_code": "WXYZ", "interval": 1}).to_string(),
            )
            .create();
        let busy: Vec<_> = [503, 429]
            .into_iter()
            .map(|status| {
                server
                    .mock("POST", "/api/accounts/deviceauth/token")
                    .with_status(status)
                    .expect(1)
                    .create()
            })
            .collect();
        let approved = server
            .mock("POST", "/api/accounts/deviceauth/token")
            .with_body(
                json!({"authorization_code": "auth-1", "code_verifier": "ver-1"}).to_string(),
            )
            .create();
        server
            .mock("POST", "/oauth/token")
            .with_body(
                json!({
                    "id_token": id_token("acct-9", "plus"),
                    "access_token": access_token("acct-9", "device", 1_900_000_000),
                    "refresh_token": "r-device",
                })
                .to_string(),
            )
            .create();
        let login = c.begin_login(true).unwrap().finish(None).unwrap();
        busy.iter().for_each(|m| m.assert());
        approved.assert();
        assert_eq!(login.creds["tokens"]["refresh_token"], "r-device");
        approved.remove();

        let down = server
            .mock("POST", "/api/accounts/deviceauth/token")
            .with_status(502)
            .with_body("bad gateway")
            .expect(3)
            .create();
        let err = c.begin_login(true).unwrap().finish(None).err().unwrap();
        down.assert();
        assert!(err.to_string().contains("502"), "{err}");
    }

    #[test]
    fn a_device_code_the_server_refuses_to_issue_is_an_error() {
        let mut server = mockito::Server::new();
        let tmp = tempfile::tempdir().unwrap();
        server
            .mock("POST", "/api/accounts/deviceauth/usercode")
            .with_status(404)
            .create();
        let err = codex(tmp.path(), &server.url())
            .begin_login(true)
            .err()
            .unwrap();
        assert_eq!(
            err.to_string(),
            "device code login is not enabled for this Codex server (404 Not Found): sign in by callback instead, without --paste or `paste = true` under [login], or with --no-paste"
        );
    }

    #[test]
    fn stray_connections_do_not_end_the_wait_and_a_refusal_does() {
        let tmp = tempfile::tempdir().unwrap();
        let c = codex(tmp.path(), &OFFLINE);
        let pending = c.begin_login(false).unwrap();
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
        let open = |url: &str, _: &std::path::Path| {
            let redirect = param(url, "redirect_uri").replace("localhost", "127.0.0.1");
            let state = param(url, "state");
            std::thread::spawn(move || browse(&format!("{redirect}?code=c&state={state}")));
            true
        };
        let (tx, lines) = std::sync::mpsc::channel();
        tx.send(String::new()).unwrap();
        let mut out = Vec::new();
        crate::commands::login(
            &store,
            &c,
            "nine",
            false,
            crate::commands::Prompt {
                open: &open,
                opens_in: Some("a test browser".into()),
                lines,
                paste: false,
            },
            &mut out,
        )
        .unwrap();
        let out = String::from_utf8(out).unwrap();
        assert!(out.starts_with("Sign in to Codex"), "{out}");
        assert!(
            out.contains(
                "Waiting for the browser to finish, or press Enter to open it in a test browser"
            ),
            "{out}"
        );
        assert!(out.contains("Opened it in a test browser."), "{out}");
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
        let pending = c.begin_login(false).unwrap();
        let port = reqwest::Url::parse(&param(pending.url(), "redirect_uri"))
            .unwrap()
            .port()
            .unwrap();
        assert!(TcpStream::connect(("127.0.0.1", port)).is_ok());
        assert!(TcpStream::connect(("::1", port)).is_ok());
    }

    #[test]
    fn a_callback_port_held_on_the_ipv6_loopback_is_taken() {
        let Ok(held) = TcpListener::bind(("::1", 0)) else {
            return;
        };
        let port = held.local_addr().unwrap().port();
        let tmp = tempfile::tempdir().unwrap();
        let c = Codex::at(tmp.path(), Api::local(&OFFLINE), port);
        let err = c.begin_login(false).err().unwrap();
        assert!(err.to_string().contains("codex login"), "{err}");
    }

    #[test]
    fn a_taken_callback_port_says_what_is_probably_holding_it() {
        let held = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = held.local_addr().unwrap().port();
        let tmp = tempfile::tempdir().unwrap();
        let c = Codex::at(tmp.path(), Api::local(&OFFLINE), port);
        let err = c.begin_login(false).err().unwrap();
        assert!(err.to_string().contains("codex login"), "{err}");
    }
}
