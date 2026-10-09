use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use crate::device::DeviceFlow;
use crate::fsx::{Changed, read_json, update_json};
use crate::oauth::{self, claims};
use crate::paths;
use crate::provider::{Identity, Login, PendingLogin, Provider};
use crate::store::Entry;

// Mirrors MoonshotAI/kimi-code's `packages/oauth`.
const CLIENT_ID: &str = "17e5f671-d194-4dfb-9706-5516cb48c098";
const AUTH_HOST: &str = "https://auth.kimi.com";
const API_BASE: &str = "https://api.kimi.com/coding/v1";
const PLATFORM: &str = "kimi_code_cli";

#[derive(Clone)]
pub struct Api {
    client: reqwest::blocking::Client,
    auth: String,
    api: String,
}

impl Api {
    pub fn kimi() -> Result<Self> {
        #[cfg(debug_assertions)]
        if let Some(base) = std::env::var_os("REMUDA_TEST_KIMI_API") {
            let base = base.to_string_lossy();
            return Self::at(&base, &base);
        }
        Self::at(AUTH_HOST, API_BASE)
    }

    fn at(auth: &str, api: &str) -> Result<Self> {
        Ok(Api {
            client: oauth::client()?,
            auth: auth.to_owned(),
            api: api.to_owned(),
        })
    }

    #[cfg(test)]
    pub fn local(base: &str) -> Self {
        Self::at(base, base).unwrap()
    }

    fn token_url(&self) -> String {
        format!("{}/api/oauth/token", self.auth)
    }
}

fn text<'a>(creds: &'a Value, key: &str) -> Option<&'a str> {
    creds.get(key)?.as_str().filter(|t| !t.is_empty())
}

/// Seconds, possibly fractional (the Python CLI writes a float), or
/// milliseconds from a writer that used them.
fn expiry_ms(raw: &Value) -> Option<i64> {
    let n = raw.as_f64().filter(|n| n.is_finite() && *n > 0.0)?;
    Some(if n > 10_000_000_000.0 { n } else { n * 1000.0 } as i64)
}

/// Kimi: the whole of `credentials/kimi-code.json` belongs to the login.
pub struct Kimi {
    home: PathBuf,
    api: Api,
}

impl Kimi {
    pub fn from_env() -> Result<Self> {
        Ok(Kimi {
            home: paths::kimi_home(),
            api: Api::kimi()?,
        })
    }

    #[cfg(test)]
    pub fn at(dir: &std::path::Path, api: Api) -> Self {
        Kimi {
            home: dir.to_path_buf(),
            api,
        }
    }

    fn creds_path(&self) -> PathBuf {
        self.home.join("credentials").join("kimi-code.json")
    }

    /// The device headers the CLI sends with every OAuth call. The device id
    /// is the CLI's own and is never minted here.
    fn headers(&self) -> Vec<(&'static str, String)> {
        let version = env!("CARGO_PKG_VERSION").to_owned();
        let os = std::env::consts::OS;
        let mut headers = vec![
            ("X-Msh-Platform", PLATFORM.to_owned()),
            ("X-Msh-Version", version),
            ("X-Msh-Device-Name", "remuda".to_owned()),
            (
                "X-Msh-Device-Model",
                format!("{os} {}", std::env::consts::ARCH),
            ),
            ("X-Msh-Os-Version", os.to_owned()),
        ];
        if let Ok(id) = std::fs::read_to_string(self.home.join("device_id"))
            && !id.trim().is_empty()
        {
            headers.push(("X-Msh-Device-Id", id.trim().to_owned()));
        }
        headers
    }

    fn with_headers(
        &self,
        mut req: reqwest::blocking::RequestBuilder,
    ) -> reqwest::blocking::RequestBuilder {
        for (name, value) in self.headers() {
            req = req.header(name, value);
        }
        req
    }

    /// Who the token belongs to, according to kimi.com.
    fn me(&self, access: &str) -> Result<Identity> {
        let resp = self
            .with_headers(self.api.client.get(format!("{}/me", self.api.api)))
            .bearer_auth(access)
            .send()
            .context("the Kimi account request failed")?;
        let status = resp.status();
        if !status.is_success() {
            bail!("kimi.com did not say whose token this is ({status})");
        }
        let me: Value = resp.json().context("malformed Kimi account response")?;
        let account_id =
            oauth::text(&me, "user_id").context("kimi.com did not say whose token this is")?;
        Ok(Identity {
            account_id,
            email: oauth::text(&me, "email").unwrap_or_default(),
            oauth_account: None,
        })
    }
}

impl Provider for Kimi {
    fn id(&self) -> &'static str {
        "kimi"
    }

    fn name(&self) -> &'static str {
        "Kimi"
    }

    fn home(&self) -> PathBuf {
        self.home.clone()
    }

    fn access_token<'a>(&self, creds: &'a Value) -> Option<&'a str> {
        text(creds, "access_token")
    }

    fn refresh_token<'a>(&self, creds: &'a Value) -> Option<&'a str> {
        text(creds, "refresh_token")
    }

    fn expires_at(&self, creds: &Value) -> Option<i64> {
        creds.get("expires_at").and_then(expiry_ms).or_else(|| {
            let exp = claims(self.access_token(creds)?)?.get("exp")?.as_i64()?;
            Some(exp * 1000)
        })
    }

    /// The membership is not in the file; TokenGauge asks kimi.com for it.
    fn plan(&self, _creds: &Value) -> String {
        "-".to_owned()
    }

    /// A signed-out CLI keeps the file with its tokens emptied.
    fn live(&self) -> Result<Option<Value>> {
        let Some(file) = read_json(&self.creds_path())? else {
            return Ok(None);
        };
        Ok(text(&file, "access_token").is_some().then_some(file))
    }

    /// The file names nobody; only kimi.com can.
    fn live_identity(&self, _creds: &Value) -> Result<Option<Identity>> {
        Ok(None)
    }

    fn identify(&self, creds: &Value) -> Result<Identity> {
        let access = self
            .access_token(creds)
            .context("credential has no access token")?;
        self.me(access)
    }

    fn install(&self, entry: &Entry, outgoing: Option<&Value>) -> Result<()> {
        let replacement = entry.creds.clone();
        update_json(&self.creds_path(), |file| {
            if outgoing.is_some_and(|o| file != o) {
                return Err(Changed.into());
            }
            *file = replacement.clone();
            Ok(())
        })
    }

    fn refresh(&self, creds: &mut Value) -> Result<Option<String>> {
        let refresh_token = text(creds, "refresh_token")
            .context("credential has no refresh token")?
            .to_owned();
        let answer = oauth::token_request(self.with_headers(
            self.api.client.post(self.api.token_url()).form(&[
                ("client_id", CLIENT_ID),
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh_token.as_str()),
            ]),
        ))?;
        apply_tokens(creds, &answer)?;
        let access = text(creds, "access_token").unwrap_or_default();
        Ok(self.me(access).ok().map(|id| id.account_id))
    }

    fn begin_login(&self) -> Result<Box<dyn PendingLogin>> {
        let api = self.api.clone();
        let headers = self.headers();
        let me = Kimi {
            home: self.home.clone(),
            api: api.clone(),
        };
        DeviceFlow {
            client: api.client.clone(),
            device_url: format!("{}/api/oauth/device_authorization", api.auth),
            token_url: api.token_url(),
            client_id: CLIENT_ID.to_owned(),
            scope: None,
            headers,
            into_login: Box::new(move |answer| {
                let mut creds = json!({});
                apply_tokens(&mut creds, &answer)?;
                let identity = me.me(text(&creds, "access_token").unwrap_or_default())?;
                Ok(Login { creds, identity })
            }),
        }
        .begin()
    }
}

/// Writes a token answer into the file's fields, the way the CLI does:
/// `expires_at` in whole seconds, and the old refresh token kept only if the
/// answer leaves it out.
fn apply_tokens(creds: &mut Value, answer: &Value) -> Result<()> {
    let access =
        oauth::text(answer, "access_token").context("the token endpoint sent no access token")?;
    creds["access_token"] = json!(access);
    for key in ["refresh_token", "scope", "token_type"] {
        if let Some(v) = oauth::text(answer, key) {
            creds[key] = json!(v);
        }
    }
    if let Some(secs) = answer.get("expires_in").and_then(Value::as_i64) {
        creds["expires_in"] = json!(secs);
        creds["expires_at"] = json!(crate::fsx::now_ms() / 1000 + secs);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fsx::write_json;
    use crate::ops::testing::OFFLINE;
    use crate::ops::{self, LiveState};
    use crate::store::Store;
    use mockito::Matcher;

    fn file(access: &str, refresh: &str) -> Value {
        json!({
            "access_token": access,
            "refresh_token": refresh,
            "expires_at": 1_900_000_000.5,
            "scope": "kimi-code",
            "token_type": "Bearer",
            "expires_in": 900,
        })
    }

    fn me_mock(server: &mut mockito::Server, access: &str, user: &str) -> mockito::Mock {
        server
            .mock("GET", "/me")
            .match_header("authorization", format!("Bearer {access}").as_str())
            .match_header("x-msh-platform", PLATFORM)
            .with_body(json!({"user_id": user, "email": format!("{user}@example.com")}).to_string())
            .create()
    }

    #[test]
    fn the_account_is_whoever_kimi_says_the_token_is() {
        let mut server = mockito::Server::new();
        let me = me_mock(&mut server, "a1", "u-1");
        let tmp = tempfile::tempdir().unwrap();
        let k = Kimi::at(tmp.path(), Api::local(&server.url()));
        let creds = file("a1", "r1");
        let id = k.identify(&creds).unwrap();
        me.assert();
        assert_eq!(id.account_id, "u-1");
        assert_eq!(id.email, "u-1@example.com");
        assert_eq!(k.live_identity(&creds).unwrap(), None);
        assert_eq!(k.expires_at(&creds), Some(1_900_000_000_500));
    }

    /// With no identity offline and kimi.com unreachable, the live login may
    /// be any stored one: which is active is unknown, so nothing may treat a
    /// stored one as inactive.
    #[test]
    fn an_unconfirmed_live_login_leaves_which_is_active_unknown() {
        let mut server = mockito::Server::new();
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::open(&tmp.path().join("store"));
        let k = Kimi::at(tmp.path(), Api::local(&server.url()));
        me_mock(&mut server, "a-work", "u-1");
        let stored = file("a-work", "r-work");
        let identity = k.identify(&stored).unwrap();
        store
            .save(&Entry::new("kimi", "work", stored, identity, 1))
            .unwrap();
        server.mock("GET", "/me").with_status(503).create();
        write_json(
            &tmp.path().join("credentials/kimi-code.json"),
            &file("a-rotated", "r-rotated"),
        )
        .unwrap();
        let err = ops::sync_live(&store, &k).unwrap_err();
        assert!(
            err.to_string().contains("which stored Kimi login is live"),
            "{err}"
        );
    }

    /// A refresh names the account through /me, and one that cannot be named
    /// is not filed under the account its sidecar only claims.
    #[test]
    fn a_healed_login_that_cannot_be_named_is_set_aside() {
        let mut server = mockito::Server::new();
        server
            .mock("POST", "/api/oauth/token")
            .with_body(
                json!({"access_token": "a2", "refresh_token": "r2", "expires_in": 900}).to_string(),
            )
            .create();
        server.mock("GET", "/me").with_status(503).create();
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::open(&tmp.path().join("store"));
        let k = Kimi::at(tmp.path(), Api::local(&server.url()));
        let id = Identity {
            account_id: "u-1".into(),
            email: String::new(),
            oauth_account: None,
        };
        store
            .save(&Entry::new("kimi", "work", file("a1", "r1"), id, 1))
            .unwrap();
        let path = tmp.path().join("store/kimi/work.json");
        let mut tampered = read_json(&path).unwrap().unwrap();
        tampered["access_token"] = json!("a-other");
        write_json(&path, &tampered).unwrap();
        assert!(!store.get("kimi", "work").unwrap().unwrap().verified);

        ops::heal(&store, &k, None).unwrap();

        assert!(!store.get("kimi", "work").unwrap().unwrap().verified);
        let kept: Vec<_> = std::fs::read_dir(tmp.path().join("store/kimi"))
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().starts_with(".set-aside-"))
            .collect();
        assert_eq!(kept.len(), 1);
    }

    #[test]
    fn a_signed_out_cli_is_signed_out() {
        let tmp = tempfile::tempdir().unwrap();
        let k = Kimi::at(tmp.path(), Api::local(&OFFLINE));
        assert_eq!(k.live().unwrap(), None);
        write_json(
            &tmp.path().join("credentials/kimi-code.json"),
            &json!({"access_token": "", "refresh_token": "", "scope": "kimi-code"}),
        )
        .unwrap();
        assert_eq!(k.live().unwrap(), None);
        let store = Store::open(&tmp.path().join("store"));
        assert_eq!(ops::sync_live(&store, &k).unwrap(), LiveState::SignedOut);
    }

    #[test]
    fn switching_replaces_the_file_whole() {
        let mut server = mockito::Server::new();
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::open(&tmp.path().join("store"));
        let k = Kimi::at(tmp.path(), Api::local(&server.url()));
        for (name, user) in [("work", "u-1"), ("perso", "u-2")] {
            let creds = file(&format!("a-{name}"), &format!("r-{name}"));
            me_mock(&mut server, &format!("a-{name}"), user);
            let identity = k.identify(&creds).unwrap();
            store
                .save(&Entry::new("kimi", name, creds, identity, 1))
                .unwrap();
        }
        let work = store.get("kimi", "work").unwrap().unwrap();
        k.install(&work, None).unwrap();

        let said = ops::switch(&store, &k, "perso", false).unwrap();

        assert_eq!(said, "switched Kimi to perso (u-2@example.com)");
        let live = read_json(&tmp.path().join("credentials/kimi-code.json"))
            .unwrap()
            .unwrap();
        assert_eq!(live, store.get("kimi", "perso").unwrap().unwrap().creds);
    }

    #[test]
    fn a_refresh_rotates_the_tokens_with_the_device_headers() {
        let mut server = mockito::Server::new();
        let mock = server
            .mock("POST", "/api/oauth/token")
            .match_header("x-msh-device-id", "dev-1")
            .match_body(Matcher::AllOf(vec![
                Matcher::UrlEncoded("grant_type".into(), "refresh_token".into()),
                Matcher::UrlEncoded("refresh_token".into(), "r1".into()),
                Matcher::UrlEncoded("client_id".into(), CLIENT_ID.into()),
            ]))
            .with_body(
                json!({"access_token": "a2", "refresh_token": "r2", "expires_in": 900, "token_type": "Bearer"})
                    .to_string(),
            )
            .create();
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("device_id"), "dev-1\n").unwrap();
        let k = Kimi::at(tmp.path(), Api::local(&server.url()));
        let mut creds = file("a1", "r1");

        k.refresh(&mut creds).unwrap();

        mock.assert();
        assert_eq!(creds["access_token"], "a2");
        assert_eq!(creds["refresh_token"], "r2");
        assert_eq!(creds["scope"], "kimi-code");
        assert!(creds["expires_at"].as_i64().unwrap() > 1_700_000_000);
    }

    #[test]
    fn a_refused_refresh_leaves_the_tokens_alone() {
        let mut server = mockito::Server::new();
        server
            .mock("POST", "/api/oauth/token")
            .with_status(400)
            .with_body(r#"{"error":"invalid_grant"}"#)
            .create();
        let tmp = tempfile::tempdir().unwrap();
        let k = Kimi::at(tmp.path(), Api::local(&server.url()));
        let mut creds = file("a1", "r1");
        let before = creds.clone();
        let err = k.refresh(&mut creds).unwrap_err();
        assert!(
            err.to_string().contains("sign this credential in again"),
            "{err}"
        );
        assert_eq!(creds, before);
    }

    #[test]
    fn a_device_sign_in_stores_the_file_the_cli_would() {
        let mut server = mockito::Server::new();
        server
            .mock("POST", "/api/oauth/device_authorization")
            .match_body(Matcher::UrlEncoded("client_id".into(), CLIENT_ID.into()))
            .with_body(
                json!({"device_code": "dc", "user_code": "K1", "verification_uri_complete": "https://www.kimi.com/code/authorize_device?user_code=K1", "interval": 1})
                    .to_string(),
            )
            .create();
        server
            .mock("POST", "/api/oauth/token")
            .with_body(
                json!({"access_token": "a9", "refresh_token": "r9", "expires_in": 900}).to_string(),
            )
            .create();
        me_mock(&mut server, "a9", "u-9");
        let tmp = tempfile::tempdir().unwrap();
        let k = Kimi::at(tmp.path(), Api::local(&server.url()));

        let pending = k.begin_login().unwrap();
        assert!(pending.url().ends_with("user_code=K1"));
        let login = pending.finish(None).unwrap();

        assert_eq!(login.identity.account_id, "u-9");
        assert_eq!(login.creds["access_token"], "a9");
        assert_eq!(k.refresh_token(&login.creds), Some("r9"));
    }
}
