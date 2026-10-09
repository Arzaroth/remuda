use std::path::PathBuf;

use anyhow::{Context, Result};
use serde_json::{Map, Value, json};

use crate::device::DeviceFlow;
use crate::fsx::{Changed, lock_file, now_ms, read_json, rfc3339, update_json};
use crate::oauth::{self, claims};
use crate::paths;
use crate::provider::{Identity, Login, PendingLogin, Provider};
use crate::store::Entry;

// Mirrors xai-org/grok-build's `xai-grok-login`.
const ISSUER: &str = "https://auth.x.ai";
const CLIENT_ID: &str = "b1a00492-073a-47ea-816f-4c329264a828";
const OIDC_SCOPE: &str = "https://auth.x.ai::";
const LOGIN_SCOPE: &str = "openid profile email offline_access grok-cli:access api:access conversations:read conversations:write workspaces:read workspaces:write";

#[derive(Clone)]
pub struct Api {
    client: reqwest::blocking::Client,
    issuer: String,
}

impl Api {
    pub fn xai() -> Result<Self> {
        #[cfg(debug_assertions)]
        if let Some(base) = std::env::var_os("REMUDA_TEST_XAI_API") {
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
        format!("{}/oauth2/token", self.issuer)
    }

    fn device_url(&self) -> String {
        format!("{}/oauth2/device/code", self.issuer)
    }
}

fn text<'a>(entry: &'a Value, key: &str) -> Option<&'a str> {
    entry.get(key)?.as_str().filter(|t| !t.is_empty())
}

/// The scope key of the SuperGrok sign-in. `auth.json` can also hold an API
/// key or a session under other scopes, which are not a login to switch.
fn oidc_scope(file: &Value) -> Option<&str> {
    file.as_object()?
        .iter()
        .find(|(scope, entry)| scope.starts_with(OIDC_SCOPE) && text(entry, "key").is_some())
        .map(|(scope, _)| scope.as_str())
}

fn oidc(file: &Value) -> Option<&Value> {
    file.get(oidc_scope(file)?)
}

fn oidc_mut(file: &mut Value) -> Option<&mut Map<String, Value>> {
    let scope = oidc_scope(file)?.to_owned();
    file.get_mut(&scope)?.as_object_mut()
}

/// The person the access token was issued to: its own `sub`, which no file
/// beside it can contradict.
fn account_of(access_token: &str) -> Option<String> {
    claims(access_token)?
        .get("sub")?
        .as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

fn identity_of(creds: &Value) -> Option<Identity> {
    let entry = oidc(creds)?;
    Some(Identity {
        account_id: account_of(text(entry, "key")?)?,
        email: text(entry, "email").unwrap_or_default().to_owned(),
        oauth_account: None,
    })
}

/// The `auth.json` the grok CLI writes for a fresh sign-in, built from the
/// token endpoint's answer. Who it is comes from the id token, which the CLI
/// validates and remuda only reads.
fn auth_json(answer: &Value, now: &str) -> Result<Value> {
    let access = oauth::text(answer, "access_token").context("no access token was issued")?;
    let id = oauth::text(answer, "id_token")
        .as_deref()
        .and_then(claims)
        .unwrap_or(Value::Null);
    let claim = |k: &str| id.get(k).and_then(Value::as_str).unwrap_or_default();
    let user = account_of(&access).unwrap_or_else(|| claim("sub").to_owned());
    let mut entry = json!({
        "key": access,
        "auth_mode": "oidc",
        "create_time": now,
        "user_id": user,
        "email": claim("email"),
        "oidc_issuer": ISSUER,
        "oidc_client_id": CLIENT_ID,
    });
    if let Some(refresh) = oauth::text(answer, "refresh_token") {
        entry["refresh_token"] = json!(refresh);
    }
    if let Some(secs) = answer.get("expires_in").and_then(Value::as_i64) {
        entry["expires_at"] = json!(rfc3339(now_ms() + secs * 1000));
    }
    Ok(json!({ format!("{OIDC_SCOPE}{CLIENT_ID}"): entry }))
}

/// Grok: the whole of `auth.json` belongs to the login.
pub struct Grok {
    auth_path: PathBuf,
    api: Api,
}

impl Grok {
    pub fn from_env() -> Result<Self> {
        Ok(Grok {
            auth_path: paths::grok_auth(),
            api: Api::xai()?,
        })
    }

    #[cfg(test)]
    pub fn at(dir: &std::path::Path, api: Api) -> Self {
        Grok {
            auth_path: dir.join("auth.json"),
            api,
        }
    }
}

impl Provider for Grok {
    fn id(&self) -> &'static str {
        "grok"
    }

    fn name(&self) -> &'static str {
        "Grok"
    }

    fn home(&self) -> PathBuf {
        self.auth_path
            .parent()
            .map(PathBuf::from)
            .unwrap_or_default()
    }

    fn access_token<'a>(&self, creds: &'a Value) -> Option<&'a str> {
        text(oidc(creds)?, "key")
    }

    fn refresh_token<'a>(&self, creds: &'a Value) -> Option<&'a str> {
        text(oidc(creds)?, "refresh_token")
    }

    fn expires_at(&self, creds: &Value) -> Option<i64> {
        let exp = claims(self.access_token(creds)?)?.get("exp")?.as_i64()?;
        Some(exp * 1000)
    }

    /// The tier is not in the file; TokenGauge asks x.ai for it.
    fn plan(&self, creds: &Value) -> String {
        match oidc(creds).and_then(|e| text(e, "principal_type")) {
            Some(t) if t.eq_ignore_ascii_case("team") => "team".to_owned(),
            _ => "-".to_owned(),
        }
    }

    fn live(&self) -> Result<Option<Value>> {
        let Some(file) = read_json(&self.auth_path)? else {
            return Ok(None);
        };
        Ok(oidc_scope(&file).is_some().then_some(file))
    }

    fn foreign_login(&self) -> Result<Option<String>> {
        let Some(file) = read_json(&self.auth_path)? else {
            return Ok(None);
        };
        if oidc_scope(&file).is_some() {
            return Ok(None);
        }
        let keyed = file
            .as_object()
            .is_some_and(|m| m.values().any(|e| text(e, "key").is_some()));
        Ok(keyed.then(|| "an API key".to_owned()))
    }

    fn live_identity(&self, creds: &Value) -> Result<Option<Identity>> {
        Ok(identity_of(creds))
    }

    fn identify(&self, creds: &Value) -> Result<Identity> {
        identity_of(creds).context("the token does not name its x.ai account")
    }

    /// Replaces the x.ai sign-in and nothing else: an API key or session the
    /// file holds under another scope stays.
    fn install(&self, entry: &Entry, outgoing: Option<&Value>) -> Result<()> {
        let (scope, login) = oidc_scope(&entry.creds)
            .and_then(|s| Some((s.to_owned(), entry.creds.get(s)?.clone())))
            .context("stored credential has no x.ai sign-in")?;
        update_json(&self.auth_path, |file| {
            if outgoing.is_some_and(|o| oidc(file) != oidc(o)) {
                return Err(Changed.into());
            }
            let map = file.as_object_mut().context("auth.json is not an object")?;
            map.retain(|k, _| !k.starts_with(OIDC_SCOPE));
            map.insert(scope.clone(), login.clone());
            Ok(())
        })
    }

    fn refresh(&self, creds: &mut Value) -> Result<Option<String>> {
        let entry = oidc(creds).context("credential has no x.ai sign-in")?;
        let refresh_token = text(entry, "refresh_token")
            .context("credential has no refresh token")?
            .to_owned();
        let client_id = text(entry, "oidc_client_id")
            .unwrap_or(CLIENT_ID)
            .to_owned();
        let answer = oauth::token_request(self.api.client.post(self.api.token_url()).form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token.as_str()),
            ("client_id", client_id.as_str()),
        ]))?;
        let access = oauth::text(&answer, "access_token")
            .context("the token endpoint sent no access token")?;
        let entry = oidc_mut(creds).context("credential has no x.ai sign-in")?;
        entry.insert("key".into(), json!(access));
        if let Some(rotated) = oauth::text(&answer, "refresh_token") {
            entry.insert("refresh_token".into(), json!(rotated));
        }
        if let Some(secs) = answer.get("expires_in").and_then(Value::as_i64) {
            entry.insert("expires_at".into(), json!(rfc3339(now_ms() + secs * 1000)));
        }
        Ok(account_of(&access))
    }

    /// The grok CLI refreshes the live login under this lock.
    fn lock_live(&self) -> Result<Option<std::fs::File>> {
        lock_file(&self.auth_path.with_file_name("auth.json.lock")).map(Some)
    }

    fn begin_login(&self) -> Result<Box<dyn PendingLogin>> {
        DeviceFlow {
            client: self.api.client.clone(),
            device_url: self.api.device_url(),
            token_url: self.api.token_url(),
            client_id: CLIENT_ID.to_owned(),
            scope: Some(LOGIN_SCOPE.to_owned()),
            headers: Vec::new(),
            into_login: Box::new(|answer| {
                let creds = auth_json(&answer, &rfc3339(now_ms()))?;
                let identity = identity_of(&creds).context("the new login names no account")?;
                Ok(Login { creds, identity })
            }),
        }
        .begin()
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

    fn key(user: &str, tag: &str, exp_secs: i64) -> String {
        jwt(json!({"sub": user, "tag": tag, "exp": exp_secs}))
    }

    fn auth(user: &str, access: &str, refresh: &str, exp_secs: i64) -> Value {
        json!({
            format!("{OIDC_SCOPE}{user}"): {
                "key": key(user, access, exp_secs),
                "auth_mode": "oidc",
                "user_id": user,
                "email": format!("{user}@example.com"),
                "principal_type": "User",
                "refresh_token": refresh,
                "expires_at": "2026-09-01T00:00:00Z",
                "oidc_issuer": ISSUER,
                "oidc_client_id": CLIENT_ID,
            }
        })
    }

    fn grok(dir: &std::path::Path, base: &str) -> Grok {
        Grok::at(dir, Api::local(base))
    }

    #[test]
    fn the_account_is_the_tokens_own_subject() {
        let tmp = tempfile::tempdir().unwrap();
        let g = grok(tmp.path(), &OFFLINE);
        let mut creds = auth("u-1", "a", "r", 1_900_000_000);
        creds[format!("{OIDC_SCOPE}u-1")]["user_id"] = json!("someone-else");
        let id = g.identify(&creds).unwrap();
        assert_eq!(id.account_id, "u-1");
        assert_eq!(id.email, "u-1@example.com");
        assert_eq!(g.expires_at(&creds), Some(1_900_000_000_000));
        assert_eq!(g.refresh_token(&creds), Some("r"));
        assert_eq!(g.plan(&creds), "-");
    }

    #[test]
    fn an_api_key_is_not_a_login_to_switch() {
        let tmp = tempfile::tempdir().unwrap();
        let g = grok(tmp.path(), &OFFLINE);
        write_json(
            &tmp.path().join("auth.json"),
            &json!({"https://accounts.x.ai/sign-in": {"key": "xai-123"}}),
        )
        .unwrap();
        assert_eq!(g.live().unwrap(), None);
        assert_eq!(g.foreign_login().unwrap().as_deref(), Some("an API key"));
        let store = Store::open(&tmp.path().join("store"));
        assert_eq!(
            ops::sync_live(&store, &g).unwrap(),
            LiveState::Foreign {
                what: "an API key".into()
            }
        );
    }

    #[test]
    fn switching_replaces_auth_json_whole() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::open(&tmp.path().join("store"));
        let g = grok(tmp.path(), &OFFLINE);
        for (name, user) in [("work", "u-1"), ("perso", "u-2")] {
            let creds = auth(user, name, &format!("r-{name}"), 1_900_000_000);
            let identity = g.identify(&creds).unwrap();
            store
                .save(&Entry::new("grok", name, creds, identity, 1))
                .unwrap();
        }
        let work = store.get("grok", "work").unwrap().unwrap();
        g.install(&work, None).unwrap();

        let said = ops::switch(&store, &g, "perso", false).unwrap();

        assert_eq!(said, "switched Grok to perso (u-2@example.com)");
        let live = read_json(&tmp.path().join("auth.json")).unwrap().unwrap();
        assert_eq!(live, store.get("grok", "perso").unwrap().unwrap().creds);
    }

    #[test]
    fn a_switch_leaves_the_files_other_scopes_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::open(&tmp.path().join("store"));
        let g = grok(tmp.path(), &OFFLINE);
        let creds = auth("u-2", "b", "r-b", 1_900_000_000);
        let identity = g.identify(&creds).unwrap();
        store
            .save(&Entry::new("grok", "perso", creds, identity, 1))
            .unwrap();
        let mut live = auth("u-1", "a", "r-a", 1_900_000_000);
        live["https://accounts.x.ai/sign-in"] = json!({"key": "xai-key"});
        write_json(&tmp.path().join("auth.json"), &live).unwrap();

        let perso = store.get("grok", "perso").unwrap().unwrap();
        g.install(&perso, Some(&live)).unwrap();

        let file = read_json(&tmp.path().join("auth.json")).unwrap().unwrap();
        assert_eq!(file["https://accounts.x.ai/sign-in"]["key"], "xai-key");
        assert!(file.get(format!("{OIDC_SCOPE}u-1")).is_none());
        assert_eq!(g.identify(&file).unwrap().account_id, "u-2");
    }

    #[test]
    fn a_refresh_rotates_the_tokens_and_keeps_the_rest() {
        let mut server = mockito::Server::new();
        let token = server
            .mock("POST", "/oauth2/token")
            .match_body(Matcher::AllOf(vec![
                Matcher::UrlEncoded("grant_type".into(), "refresh_token".into()),
                Matcher::UrlEncoded("refresh_token".into(), "r1".into()),
                Matcher::UrlEncoded("client_id".into(), CLIENT_ID.into()),
            ]))
            .with_body(
                json!({
                    "access_token": key("u-1", "new", 1_900_000_000),
                    "refresh_token": "r2",
                    "expires_in": 21600,
                })
                .to_string(),
            )
            .create();
        let tmp = tempfile::tempdir().unwrap();
        let g = grok(tmp.path(), &server.url());
        let mut creds = auth("u-1", "old", "r1", 1);

        let account = g.refresh(&mut creds).unwrap();

        token.assert();
        assert_eq!(account.as_deref(), Some("u-1"));
        let entry = &creds[format!("{OIDC_SCOPE}u-1")];
        assert_eq!(entry["key"], key("u-1", "new", 1_900_000_000));
        assert_eq!(entry["refresh_token"], "r2");
        assert_ne!(entry["expires_at"], "2026-09-01T00:00:00Z");
        assert_eq!(entry["email"], "u-1@example.com");
    }

    #[test]
    fn a_device_sign_in_writes_the_file_the_cli_would() {
        let mut server = mockito::Server::new();
        server
            .mock("POST", "/oauth2/device/code")
            .match_body(Matcher::UrlEncoded("client_id".into(), CLIENT_ID.into()))
            .with_body(
                json!({"device_code": "dc", "verification_uri_complete": "https://accounts.x.ai/device?code=X", "interval": 1})
                    .to_string(),
            )
            .create();
        server
            .mock("POST", "/oauth2/token")
            .with_body(
                json!({
                    "access_token": key("u-9", "a", 1_900_000_000),
                    "refresh_token": "r9",
                    "id_token": jwt(json!({"sub": "u-9", "email": "nine@example.com"})),
                    "expires_in": 21600,
                })
                .to_string(),
            )
            .create();
        let tmp = tempfile::tempdir().unwrap();
        let g = grok(tmp.path(), &server.url());

        let pending = g.begin_login().unwrap();
        assert_eq!(pending.url(), "https://accounts.x.ai/device?code=X");
        let login = pending.finish(None).unwrap();

        assert_eq!(login.identity.account_id, "u-9");
        assert_eq!(login.identity.email, "nine@example.com");
        let entry = &login.creds[format!("{OIDC_SCOPE}{CLIENT_ID}")];
        assert_eq!(entry["auth_mode"], "oidc");
        assert_eq!(entry["refresh_token"], "r9");
        assert_eq!(g.refresh_token(&login.creds), Some("r9"));
    }

    #[test]
    fn a_switch_holds_the_lock_the_cli_refreshes_under() {
        let tmp = tempfile::tempdir().unwrap();
        let g = grok(tmp.path(), &OFFLINE);
        let held = g.lock_live().unwrap().unwrap();
        assert!(matches!(
            std::fs::File::open(tmp.path().join("auth.json.lock"))
                .unwrap()
                .try_lock(),
            Err(std::fs::TryLockError::WouldBlock)
        ));
        drop(held);
    }

    #[test]
    fn a_refused_refresh_leaves_the_tokens_alone() {
        let mut server = mockito::Server::new();
        server
            .mock("POST", "/oauth2/token")
            .with_status(400)
            .with_body(r#"{"error":"invalid_grant"}"#)
            .create();
        let tmp = tempfile::tempdir().unwrap();
        let g = grok(tmp.path(), &server.url());
        let mut creds = auth("u-1", "a1", "r1", 1);
        let before = creds.clone();
        let err = g.refresh(&mut creds).unwrap_err();
        assert!(
            err.to_string().contains("sign this credential in again"),
            "{err}"
        );
        assert_eq!(creds, before);
    }
}
