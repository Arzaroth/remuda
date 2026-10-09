use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use crate::fsx::{Changed, read_json, update_json};
use crate::oauth::{self, claims};
use crate::paths;
use crate::pkce;
use crate::provider::{Identity, Login, PendingLogin, Provider};
use crate::store::Entry;

// cursor-agent is closed source; these are the endpoints and client id the
// open-source Cursor clients agree on (oh-my-pi, jcode, auth2api).
const CLIENT_ID: &str = "KbZUR41cY7W6zRSdpSUJ7I7mLYBKOCmB";
const API: &str = "https://api2.cursor.sh";
const WEB: &str = "https://cursor.com";
const POLL_FIRST: Duration = Duration::from_secs(1);
const POLL_MAX: Duration = Duration::from_secs(10);
const LOGIN_TIMEOUT: Duration = Duration::from_secs(10 * 60);

#[derive(Clone)]
pub struct Api {
    client: reqwest::blocking::Client,
    api: String,
    web: String,
    poll_first: Duration,
}

impl Api {
    pub fn cursor() -> Result<Self> {
        #[cfg(debug_assertions)]
        if let Some(base) = std::env::var_os("REMUDA_TEST_CURSOR_API") {
            let base = base.to_string_lossy();
            return Self::at(&base, &base, POLL_FIRST);
        }
        Self::at(API, WEB, POLL_FIRST)
    }

    fn at(api: &str, web: &str, poll_first: Duration) -> Result<Self> {
        Ok(Api {
            client: oauth::client()?,
            api: api.to_owned(),
            web: web.to_owned(),
            poll_first,
        })
    }

    #[cfg(test)]
    pub fn local(base: &str) -> Self {
        Self::at(base, base, Duration::from_millis(50)).unwrap()
    }
}

fn text<'a>(creds: &'a Value, key: &str) -> Option<&'a str> {
    creds.get(key)?.as_str().filter(|t| !t.is_empty())
}

/// The user id in the token's `sub`, which reads `<provider>|<user id>`,
/// read the way TokenGauge reads it to build cursor.com's session cookie.
fn user_of(access_token: &str) -> Option<String> {
    let sub = claims(access_token)?.get("sub")?.as_str()?.to_owned();
    let user = sub.split('|').nth(1)?.trim();
    (!user.is_empty()).then(|| user.to_owned())
}

fn identity_of(creds: &Value) -> Option<Identity> {
    let access = text(creds, "accessToken")?;
    Some(Identity {
        account_id: user_of(access)?,
        email: claims(access)
            .and_then(|c| c.get("email")?.as_str().map(str::to_owned))
            .unwrap_or_default(),
        oauth_account: None,
    })
}

/// A random version 4 UUID, which is what Cursor's sign-in expects.
fn uuid() -> Result<String> {
    let mut b = [0u8; 16];
    getrandom::fill(&mut b).map_err(|e| anyhow::anyhow!("no randomness: {e}"))?;
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let hex: String = b.iter().map(|x| format!("{x:02x}")).collect();
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    ))
}

/// Cursor: the whole of cursor-agent's `auth.json` belongs to the login. The
/// IDE keeps its own login in a database remuda leaves alone.
pub struct Cursor {
    auth_path: PathBuf,
    api: Api,
}

impl Cursor {
    pub fn from_env() -> Result<Self> {
        Ok(Cursor {
            auth_path: paths::cursor_auth(),
            api: Api::cursor()?,
        })
    }

    #[cfg(test)]
    pub fn at(dir: &std::path::Path, api: Api) -> Self {
        Cursor {
            auth_path: dir.join("auth.json"),
            api,
        }
    }
}

impl Provider for Cursor {
    fn id(&self) -> &'static str {
        "cursor"
    }

    fn name(&self) -> &'static str {
        "Cursor"
    }

    fn home(&self) -> PathBuf {
        self.auth_path
            .parent()
            .map(PathBuf::from)
            .unwrap_or_default()
    }

    fn access_token<'a>(&self, creds: &'a Value) -> Option<&'a str> {
        text(creds, "accessToken")
    }

    fn refresh_token<'a>(&self, creds: &'a Value) -> Option<&'a str> {
        text(creds, "refreshToken")
    }

    fn expires_at(&self, creds: &Value) -> Option<i64> {
        let exp = claims(self.access_token(creds)?)?.get("exp")?.as_i64()?;
        Some(exp * 1000)
    }

    /// The membership is not in the file; TokenGauge asks cursor.com for it.
    fn plan(&self, _creds: &Value) -> String {
        "-".to_owned()
    }

    fn live(&self) -> Result<Option<Value>> {
        let Some(file) = read_json(&self.auth_path)? else {
            return Ok(None);
        };
        Ok(text(&file, "accessToken").is_some().then_some(file))
    }

    fn foreign_login(&self) -> Result<Option<String>> {
        let Some(file) = read_json(&self.auth_path)? else {
            return Ok(None);
        };
        let api_key = text(&file, "accessToken").is_none() && text(&file, "apiKey").is_some();
        Ok(api_key.then(|| "an API key".to_owned()))
    }

    fn live_identity(&self, creds: &Value) -> Result<Option<Identity>> {
        Ok(identity_of(creds))
    }

    fn identify(&self, creds: &Value) -> Result<Identity> {
        identity_of(creds).context("the token does not name its Cursor user")
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

    /// Cursor usually renews only the access token and keeps the refresh
    /// token; a session it will not renew answers 200 with `shouldLogout`.
    fn refresh(&self, creds: &mut Value) -> Result<Option<String>> {
        let refresh_token = text(creds, "refreshToken")
            .context("credential has no refresh token")?
            .to_owned();
        let answer = oauth::token_request(
            self.api
                .client
                .post(format!("{}/oauth/token", self.api.api))
                .json(&json!({
                    "grant_type": "refresh_token",
                    "client_id": CLIENT_ID,
                    "refresh_token": refresh_token,
                })),
        )?;
        let access = oauth::text(&answer, "access_token");
        let access = match access {
            Some(a) if answer.get("shouldLogout") != Some(&Value::Bool(true)) => a,
            _ => bail!("Cursor ended this session: sign this credential in again"),
        };
        creds["accessToken"] = json!(access);
        if let Some(rotated) = oauth::text(&answer, "refresh_token") {
            creds["refreshToken"] = json!(rotated);
        }
        Ok(user_of(&access))
    }

    fn begin_login(&self) -> Result<Box<dyn PendingLogin>> {
        let verifier = pkce::random()?;
        let uuid = uuid()?;
        let url = reqwest::Url::parse_with_params(
            &format!("{}/loginDeepControl", self.api.web),
            &[
                ("challenge", pkce::challenge(&verifier).as_str()),
                ("uuid", uuid.as_str()),
                ("mode", "login"),
                ("redirectTarget", "cli"),
            ],
        )?
        .to_string();
        Ok(Box::new(CursorPending {
            api: self.api.clone(),
            uuid,
            verifier,
            url,
            cancelled: Arc::new(AtomicBool::new(false)),
        }))
    }
}

/// A sign-in that cursor.com finishes on its side: the browser approves the
/// uuid, and this side polls until the tokens are there to collect.
struct CursorPending {
    api: Api,
    uuid: String,
    verifier: String,
    url: String,
    cancelled: Arc<AtomicBool>,
}

impl CursorPending {
    fn wait(&self, d: Duration) -> Result<()> {
        let until = Instant::now() + d;
        while Instant::now() < until {
            if self.cancelled.load(Ordering::Relaxed) {
                bail!("the sign-in was cancelled");
            }
            std::thread::sleep(Duration::from_millis(100).min(until - Instant::now()));
        }
        Ok(())
    }

    fn poll(&self) -> Result<Value> {
        let deadline = Instant::now() + LOGIN_TIMEOUT;
        let mut delay = self.api.poll_first;
        let mut errors = 0;
        while Instant::now() < deadline {
            self.wait(delay)?;
            let url = reqwest::Url::parse_with_params(
                &format!("{}/auth/poll", self.api.api),
                &[("uuid", &self.uuid), ("verifier", &self.verifier)],
            )?;
            let resp = self.api.client.get(url).send();
            match resp {
                Ok(r) if r.status() == reqwest::StatusCode::NOT_FOUND => {
                    errors = 0;
                    delay = delay.mul_f64(1.2).min(POLL_MAX);
                }
                Ok(r) if r.status().is_success() => {
                    return r
                        .json()
                        .map_err(reqwest::Error::without_url)
                        .context("malformed sign-in answer");
                }
                Ok(r) => bail!("cursor.com answered the sign-in with {}", r.status()),
                // The URL carries the verifier, which with the uuid collects
                // the tokens: no error may quote it.
                Err(e) => {
                    errors += 1;
                    if errors >= 3 {
                        return Err(e.without_url())
                            .context("cursor.com stopped answering the sign-in");
                    }
                }
            }
        }
        bail!("the sign-in was not approved in time")
    }
}

impl PendingLogin for CursorPending {
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
        let answer = self.poll()?;
        let access = text(&answer, "accessToken").context("cursor.com sent no access token")?;
        let creds = json!({
            "accessToken": access,
            "refreshToken": text(&answer, "refreshToken"),
            "apiKey": null,
        });
        let identity = identity_of(&creds).context("the new login names no account")?;
        Ok(Login { creds, identity })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fsx::write_json;
    use crate::oauth::fake_jwt as jwt;
    use crate::ops::testing::OFFLINE;
    use crate::ops::{self, LiveState};
    use crate::store::Store;
    use mockito::Matcher;

    fn token(user: &str, tag: &str) -> String {
        jwt(json!({"sub": format!("auth0|{user}"), "tag": tag, "exp": 1_900_000_000}))
    }

    fn auth(user: &str, tag: &str, refresh: &str) -> Value {
        json!({"accessToken": token(user, tag), "refreshToken": refresh, "apiKey": null})
    }

    #[test]
    fn the_account_is_the_user_in_the_tokens_subject() {
        let tmp = tempfile::tempdir().unwrap();
        let c = Cursor::at(tmp.path(), Api::local(&OFFLINE));
        let creds = auth("user_01", "a", "r");
        assert_eq!(c.identify(&creds).unwrap().account_id, "user_01");
        assert_eq!(c.expires_at(&creds), Some(1_900_000_000_000));
        assert!(c.identify(&json!({"accessToken": "opaque"})).is_err());
    }

    #[test]
    fn an_api_key_is_not_a_login_to_switch() {
        let tmp = tempfile::tempdir().unwrap();
        let c = Cursor::at(tmp.path(), Api::local(&OFFLINE));
        write_json(
            &tmp.path().join("auth.json"),
            &json!({"accessToken": null, "apiKey": "key_123"}),
        )
        .unwrap();
        let store = Store::open(&tmp.path().join("store"));
        assert_eq!(
            ops::sync_live(&store, &c).unwrap(),
            LiveState::Foreign {
                what: "an API key".into()
            }
        );
    }

    #[test]
    fn switching_replaces_auth_json_whole() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::open(&tmp.path().join("store"));
        let c = Cursor::at(tmp.path(), Api::local(&OFFLINE));
        for (name, user) in [("work", "u1"), ("perso", "u2")] {
            let creds = auth(user, name, &format!("r-{name}"));
            let identity = c.identify(&creds).unwrap();
            store
                .save(&Entry::new("cursor", name, creds, identity, 1))
                .unwrap();
        }
        let work = store.get("cursor", "work").unwrap().unwrap();
        c.install(&work, None).unwrap();

        ops::switch(&store, &c, "perso", false).unwrap();

        let live = read_json(&tmp.path().join("auth.json")).unwrap().unwrap();
        assert_eq!(live, store.get("cursor", "perso").unwrap().unwrap().creds);
    }

    #[test]
    fn a_refresh_renews_the_access_token_and_keeps_the_refresh_token() {
        let mut server = mockito::Server::new();
        let mock = server
            .mock("POST", "/oauth/token")
            .match_body(Matcher::PartialJson(json!({
                "grant_type": "refresh_token",
                "client_id": CLIENT_ID,
                "refresh_token": "r1",
            })))
            .with_body(json!({"access_token": token("u1", "new")}).to_string())
            .create();
        let tmp = tempfile::tempdir().unwrap();
        let c = Cursor::at(tmp.path(), Api::local(&server.url()));
        let mut creds = auth("u1", "old", "r1");

        let account = c.refresh(&mut creds).unwrap();

        mock.assert();
        assert_eq!(account.as_deref(), Some("u1"));
        assert_eq!(creds["accessToken"], token("u1", "new"));
        assert_eq!(creds["refreshToken"], "r1");
    }

    #[test]
    fn a_session_cursor_ended_is_not_saved_as_renewed() {
        let mut server = mockito::Server::new();
        server
            .mock("POST", "/oauth/token")
            .with_body(r#"{"access_token":"","shouldLogout":true}"#)
            .create();
        let tmp = tempfile::tempdir().unwrap();
        let c = Cursor::at(tmp.path(), Api::local(&server.url()));
        let mut creds = auth("u1", "old", "r1");
        let before = creds.clone();
        let err = c.refresh(&mut creds).unwrap_err();
        assert!(
            err.to_string().contains("sign this credential in again"),
            "{err}"
        );
        assert_eq!(creds, before);
    }

    #[test]
    fn a_sign_in_polls_until_cursor_has_the_tokens() {
        let mut server = mockito::Server::new();
        let tmp = tempfile::tempdir().unwrap();
        let c = Cursor::at(tmp.path(), Api::local(&server.url()));
        let pending = c.begin_login().unwrap();
        let url = reqwest::Url::parse(pending.url()).unwrap();
        assert_eq!(url.path(), "/loginDeepControl");
        let param = |k: &str| {
            url.query_pairs()
                .find(|(key, _)| key == k)
                .map(|(_, v)| v.into_owned())
                .unwrap()
        };
        assert_eq!(param("redirectTarget"), "cli");
        let uuid = param("uuid");
        assert_eq!(uuid.len(), 36);

        let waiting = server
            .mock("GET", "/auth/poll")
            .match_query(Matcher::UrlEncoded("uuid".into(), uuid.clone()))
            .with_status(404)
            .expect_at_least(1)
            .create();
        let finished = std::thread::spawn(move || pending.finish(None));
        std::thread::sleep(Duration::from_millis(300));
        waiting.assert();
        waiting.remove();
        server
            .mock("GET", "/auth/poll")
            .match_query(Matcher::UrlEncoded("uuid".into(), uuid))
            .with_body(json!({"accessToken": token("u7", "a"), "refreshToken": "r7"}).to_string())
            .create();
        let login = finished.join().unwrap().unwrap();
        assert_eq!(login.identity.account_id, "u7");
        assert_eq!(login.creds["refreshToken"], "r7");
    }
}
