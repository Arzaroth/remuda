use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde_json::{Map, Value, json};

use crate::fsx::{now_ms, read_json, rfc3339, write_json};
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
        Self::at(ISSUER)
    }

    fn at(issuer: &str) -> Result<Self> {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(30))
            .user_agent(concat!("remuda/", env!("CARGO_PKG_VERSION")))
            .build()
            .context("failed to build the HTTP client")?;
        Ok(Api {
            client,
            issuer: issuer.to_owned(),
        })
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

fn identity_of(creds: &Value) -> Option<Identity> {
    let id_claims = token(creds, "id_token").and_then(claims);
    let account_id = token(creds, "account_id").map(str::to_owned).or_else(|| {
        auth_claim(id_claims.as_ref()?, "chatgpt_account_id")?
            .as_str()
            .map(str::to_owned)
    })?;
    let email = id_claims
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

    /// The account id sits in the same file as the tokens it names, so the
    /// file is its own confirmation.
    fn confirm(&self, creds: &Value) -> Result<String> {
        identity_of(creds)
            .map(|id| id.account_id)
            .context("auth.json does not name its account")
    }

    fn install(&self, entry: &Entry) -> Result<()> {
        write_json(&self.auth_path, &entry.creds)
    }

    fn refresh(&self, creds: &mut Value) -> Result<Option<String>> {
        let refresh_token = token(creds, "refresh_token")
            .context("credential has no refresh token")?
            .to_owned();
        let resp = self
            .api
            .client
            .post(self.api.token_url())
            .json(&json!({
                "client_id": CLIENT_ID,
                "grant_type": "refresh_token",
                "refresh_token": refresh_token,
                "scope": REFRESH_SCOPE,
            }))
            .send()
            .context("token request failed")?;
        let status = resp.status();
        let body = resp.text().unwrap_or_default();
        if !status.is_success() {
            bail!(
                "token endpoint answered {status}: {}",
                body.chars().take(200).collect::<String>()
            );
        }
        let answer: Value = serde_json::from_str(&body).context("malformed token response")?;
        let fresh = |k: &str| {
            answer
                .get(k)
                .and_then(Value::as_str)
                .filter(|t| !t.is_empty())
                .map(str::to_owned)
        };
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
        Ok(fresh("id_token").as_deref().and_then(claims).and_then(|c| {
            auth_claim(&c, "chatgpt_account_id")?
                .as_str()
                .map(str::to_owned)
        }))
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
        let resp = self
            .api
            .client
            .post(self.api.token_url())
            .form(&[
                ("grant_type", "authorization_code"),
                ("code", code),
                ("redirect_uri", &self.redirect_uri),
                ("client_id", CLIENT_ID),
                ("code_verifier", &self.verifier),
            ])
            .send()
            .context("token request failed")?;
        let status = resp.status();
        let body = resp.text().unwrap_or_default();
        if !status.is_success() {
            bail!(
                "token endpoint answered {status}: {}",
                body.chars().take(200).collect::<String>()
            );
        }
        let answer: Value = serde_json::from_str(&body).context("malformed token response")?;
        let get = |k: &str| {
            answer
                .get(k)
                .and_then(Value::as_str)
                .filter(|t| !t.is_empty())
                .with_context(|| format!("the token endpoint sent no {k}"))
        };
        let id_token = get("id_token")?;
        let account_id = claims(id_token)
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
