use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::fsx::{now_ms, read_json, write_json};
use crate::oauth;
use crate::paths;
use crate::pkce;
use crate::provider::{Identity, Login, PendingLogin, Provider};
use crate::store::Entry;

// Mirrors Claude Code 2.1.282's OAuth client.
const CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
const AUTHORIZE_URL: &str = "https://claude.com/cai/oauth/authorize";
const TOKEN_URL: &str = "https://platform.claude.com/v1/oauth/token";
const REDIRECT_URI: &str = "https://platform.claude.com/oauth/code/callback";
const PROFILE_URL: &str = "https://api.anthropic.com/api/oauth/profile";
const LOGIN_SCOPES: &[&str] = &[
    "org:create_api_key",
    "user:profile",
    "user:inference",
    "user:sessions:claude_code",
    "user:mcp_servers",
    "user:file_upload",
    "user:plugins",
];

pub const OAUTH_KEY: &str = "claudeAiOauth";

#[derive(Clone)]
pub struct Api {
    client: reqwest::blocking::Client,
    token_url: String,
    profile_url: String,
}

impl Api {
    pub fn claude() -> Result<Self> {
        Self::at(TOKEN_URL, PROFILE_URL)
    }

    fn at(token_url: &str, profile_url: &str) -> Result<Self> {
        Ok(Api {
            client: oauth::client()?,
            token_url: token_url.to_owned(),
            profile_url: profile_url.to_owned(),
        })
    }

    #[cfg(test)]
    pub fn local(base: &str) -> Self {
        Self::at(
            &format!("{base}/v1/oauth/token"),
            &format!("{base}/api/oauth/profile"),
        )
        .unwrap()
    }
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: i64,
    refresh_token_expires_in: Option<i64>,
    scope: Option<String>,
    account: Option<TokenAccount>,
}

#[derive(Deserialize)]
struct TokenAccount {
    uuid: String,
}

fn post_token(api: &Api, body: &Value) -> Result<TokenResponse> {
    let answer = oauth::token_request(api.client.post(&api.token_url).json(body))?;
    serde_json::from_value(answer).context("malformed token response")
}

fn apply_tokens(oauth: &mut Map<String, Value>, t: &TokenResponse) {
    let now = now_ms();
    oauth.insert("accessToken".into(), json!(t.access_token));
    if let Some(refresh) = &t.refresh_token {
        oauth.insert("refreshToken".into(), json!(refresh));
    }
    oauth.insert("expiresAt".into(), json!(now + t.expires_in * 1000));
    if let Some(secs) = t.refresh_token_expires_in {
        oauth.insert("refreshTokenExpiresAt".into(), json!(now + secs * 1000));
    }
    if let Some(scope) = &t.scope {
        let scopes: Vec<&str> = scope.split_whitespace().collect();
        oauth.insert("scopes".into(), json!(scopes));
    }
}

/// Refreshes in place and returns the account uuid the token endpoint reports.
pub fn refresh(api: &Api, oauth: &mut Map<String, Value>) -> Result<Option<String>> {
    let refresh_token = oauth
        .get("refreshToken")
        .and_then(Value::as_str)
        .context("credential has no refresh token")?;
    let scopes = oauth
        .get("scopes")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_default();
    let mut body = json!({
        "grant_type": "refresh_token",
        "refresh_token": refresh_token,
        "client_id": CLIENT_ID,
    });
    if !scopes.is_empty() {
        body["scope"] = json!(scopes);
    }
    let t = post_token(api, &body)?;
    apply_tokens(oauth, &t);
    Ok(t.account.map(|a| a.uuid))
}

pub fn profile(api: &Api, access_token: &str) -> Result<Value> {
    let resp = api
        .client
        .get(&api.profile_url)
        .bearer_auth(access_token)
        .header("Cache-Control", "no-cache")
        .send()
        .context("profile request failed")?;
    let status = resp.status();
    if !status.is_success() {
        bail!("profile endpoint answered {status}");
    }
    resp.json().context("malformed profile response")
}

pub fn profile_account_uuid(profile: &Value) -> Option<&str> {
    profile.pointer("/account/uuid").and_then(Value::as_str)
}

/// The `oauthAccount` block Claude Code writes into `.claude.json` from a profile.
pub fn oauth_account_from_profile(p: &Value) -> Result<Value> {
    let account = p.get("account").context("profile has no account")?;
    let org = p
        .get("organization")
        .context("profile has no organization")?;
    let s = |v: &Value, k: &str| v.get(k).cloned().unwrap_or(Value::Null);
    let mut out = Map::new();
    out.insert("accountUuid".into(), s(account, "uuid"));
    out.insert("emailAddress".into(), s(account, "email"));
    out.insert("organizationUuid".into(), s(org, "uuid"));
    for (key, src) in [("displayName", "display_name"), ("fullName", "full_name")] {
        if let Some(v) = account
            .get(src)
            .filter(|v| v.as_str().is_some_and(|s| !s.is_empty()))
        {
            out.insert(key.into(), v.clone());
        }
    }
    out.insert(
        "hasExtraUsageEnabled".into(),
        json!(
            org.get("has_extra_usage_enabled")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        ),
    );
    if let Some(v) = org.get("billing_type").filter(|v| !v.is_null()) {
        out.insert("billingType".into(), v.clone());
    }
    out.insert("accountCreatedAt".into(), s(account, "created_at"));
    if let Some(v) = org.get("subscription_created_at").filter(|v| !v.is_null()) {
        out.insert("subscriptionCreatedAt".into(), v.clone());
    }
    out.insert(
        "ccOnboardingFlags".into(),
        org.get("cc_onboarding_flags")
            .cloned()
            .unwrap_or_else(|| json!({})),
    );
    out.insert(
        "claudeCodeTrialEndsAt".into(),
        s(org, "claude_code_trial_ends_at"),
    );
    out.insert(
        "claudeCodeTrialDurationDays".into(),
        s(org, "claude_code_trial_duration_days"),
    );
    out.insert("seatTier".into(), s(org, "seat_tier"));
    out.insert("profileFetchedAt".into(), json!(now_ms()));
    Ok(Value::Object(out))
}

fn subscription_type(org_type: Option<&str>) -> Value {
    match org_type {
        Some("claude_max") => json!("max"),
        Some("claude_pro") => json!("pro"),
        Some("claude_enterprise") => json!("enterprise"),
        Some("claude_team") => json!("team"),
        _ => Value::Null,
    }
}

pub struct Pkce {
    pub url: String,
    verifier: String,
    state: String,
}

pub fn start_login() -> Result<Pkce> {
    let verifier = pkce::random()?;
    let state = pkce::random()?;
    let challenge = pkce::challenge(&verifier);
    let url = reqwest::Url::parse_with_params(
        AUTHORIZE_URL,
        &[
            ("code", "true"),
            ("client_id", CLIENT_ID),
            ("response_type", "code"),
            ("redirect_uri", REDIRECT_URI),
            ("scope", &LOGIN_SCOPES.join(" ")),
            ("code_challenge", &challenge),
            ("code_challenge_method", "S256"),
            ("state", &state),
        ],
    )?
    .to_string();
    Ok(Pkce {
        url,
        verifier,
        state,
    })
}

pub struct ClaudeLogin {
    pub oauth: Value,
    pub oauth_account: Value,
}

/// Exchanges the `code#state` string the callback page shows.
pub fn finish_login(api: &Api, pkce: &Pkce, pasted: &str) -> Result<ClaudeLogin> {
    let pasted = pasted.trim();
    let (code, state) = pasted
        .split_once('#')
        .unwrap_or((pasted, pkce.state.as_str()));
    if state != pkce.state {
        bail!("the pasted code belongs to another login attempt");
    }
    let t = post_token(
        api,
        &json!({
            "grant_type": "authorization_code",
            "code": code,
            "redirect_uri": REDIRECT_URI,
            "client_id": CLIENT_ID,
            "code_verifier": pkce.verifier,
            "state": state,
        }),
    )?;
    let p = profile(api, &t.access_token)?;
    let mut oauth = Map::new();
    apply_tokens(&mut oauth, &t);
    let org = p.get("organization");
    oauth.insert(
        "subscriptionType".into(),
        subscription_type(
            org.and_then(|o| o.get("organization_type"))
                .and_then(Value::as_str),
        ),
    );
    oauth.insert(
        "rateLimitTier".into(),
        org.and_then(|o| o.get("rate_limit_tier"))
            .cloned()
            .unwrap_or(Value::Null),
    );
    Ok(ClaudeLogin {
        oauth: Value::Object(oauth),
        oauth_account: oauth_account_from_profile(&p)?,
    })
}

fn identity_of(oauth_account: Value) -> Result<Identity> {
    let field = |k: &str| {
        oauth_account
            .get(k)
            .and_then(Value::as_str)
            .map(str::to_owned)
    };
    Ok(Identity {
        account_id: field("accountUuid").context("oauthAccount has no accountUuid")?,
        email: field("emailAddress").unwrap_or_default(),
        oauth_account: Some(oauth_account),
    })
}

/// Claude Code: the login is `claudeAiOauth` in `.credentials.json`, and the
/// account its UI shows is `oauthAccount` in `.claude.json`.
pub struct Claude {
    creds_path: PathBuf,
    config_path: PathBuf,
    api: Api,
}

impl Claude {
    pub fn from_env() -> Result<Self> {
        Ok(Claude {
            creds_path: paths::claude_credentials(),
            config_path: paths::claude_config(),
            api: Api::claude()?,
        })
    }

    #[cfg(test)]
    pub fn at(dir: &std::path::Path, api: Api) -> Self {
        Claude {
            creds_path: dir.join(".credentials.json"),
            config_path: dir.join(".claude.json"),
            api,
        }
    }

    fn oauth(creds: &Value) -> Option<&Map<String, Value>> {
        creds.get(OAUTH_KEY).and_then(Value::as_object)
    }
}

impl Provider for Claude {
    fn id(&self) -> &'static str {
        "claude"
    }

    fn name(&self) -> &'static str {
        "Claude Code"
    }

    fn access_token<'a>(&self, creds: &'a Value) -> Option<&'a str> {
        Self::oauth(creds)?.get("accessToken")?.as_str()
    }

    fn refresh_token<'a>(&self, creds: &'a Value) -> Option<&'a str> {
        Self::oauth(creds)?.get("refreshToken")?.as_str()
    }

    fn expires_at(&self, creds: &Value) -> Option<i64> {
        Self::oauth(creds)?.get("expiresAt")?.as_i64()
    }

    fn refresh_expires_at(&self, creds: &Value) -> Option<i64> {
        Self::oauth(creds)?.get("refreshTokenExpiresAt")?.as_i64()
    }

    fn plan(&self, creds: &Value) -> String {
        let o = Self::oauth(creds);
        let get = |k| o.and_then(|o| o.get(k)).and_then(Value::as_str);
        match (get("subscriptionType"), get("rateLimitTier")) {
            (Some(sub), Some(tier)) if tier.ends_with("_20x") => format!("{sub} 20x"),
            (Some(sub), Some(tier)) if tier.ends_with("_5x") => format!("{sub} 5x"),
            (Some(sub), _) => sub.to_owned(),
            _ => "-".to_owned(),
        }
    }

    fn live(&self) -> Result<Option<Value>> {
        let Some(file) = read_json(&self.creds_path)? else {
            return Ok(None);
        };
        Ok(Self::oauth(&file)
            .filter(|o| {
                o.get("accessToken")
                    .and_then(Value::as_str)
                    .is_some_and(|t| !t.is_empty())
            })
            .map(|o| json!({ OAUTH_KEY: o })))
    }

    fn live_identity(&self, _creds: &Value) -> Result<Option<Identity>> {
        read_json(&self.config_path)?
            .and_then(|c| c.get("oauthAccount").cloned())
            .map(identity_of)
            .transpose()
    }

    fn confirm(&self, creds: &Value) -> Result<String> {
        let access = self
            .access_token(creds)
            .context("credential has no access token")?;
        let profile = profile(&self.api, access)?;
        profile_account_uuid(&profile)
            .map(str::to_owned)
            .context("profile has no account uuid")
    }

    /// Writes the account first and the login second: Claude Code adopts a
    /// login when the credentials file changes, and by then the account the
    /// UI shows is already the new one. Every other key in both files is kept.
    fn install(&self, entry: &Entry) -> Result<()> {
        let oauth = Self::oauth(&entry.creds)
            .context("stored credential has no login")?
            .clone();
        let account = entry
            .meta
            .oauth_account
            .clone()
            .context("stored credential has no oauthAccount")?;
        let mut config = read_json(&self.config_path)?.unwrap_or_else(|| json!({}));
        config
            .as_object_mut()
            .with_context(|| format!("{} is not a JSON object", self.config_path.display()))?
            .insert("oauthAccount".into(), account);
        write_json(&self.config_path, &config)?;

        let mut file = read_json(&self.creds_path)?.unwrap_or_else(|| json!({}));
        file.as_object_mut()
            .with_context(|| format!("{} is not a JSON object", self.creds_path.display()))?
            .insert(OAUTH_KEY.into(), Value::Object(oauth));
        write_json(&self.creds_path, &file)
    }

    fn refresh(&self, creds: &mut Value) -> Result<Option<String>> {
        let oauth = creds
            .get_mut(OAUTH_KEY)
            .and_then(Value::as_object_mut)
            .context("stored credential has no login")?;
        refresh(&self.api, oauth)
    }

    fn begin_login(&self) -> Result<Box<dyn PendingLogin>> {
        Ok(Box::new(ClaudePending {
            api: self.api.clone(),
            pkce: start_login()?,
        }))
    }
}

struct ClaudePending {
    api: Api,
    pkce: Pkce,
}

impl PendingLogin for ClaudePending {
    fn url(&self) -> &str {
        &self.pkce.url
    }

    fn needs_code(&self) -> bool {
        true
    }

    fn finish(self: Box<Self>, code: Option<&str>) -> Result<Login> {
        let code = code.context("Claude needs the code the sign-in page shows")?;
        let done = finish_login(&self.api, &self.pkce, code)?;
        Ok(Login {
            creds: json!({ OAUTH_KEY: done.oauth }),
            identity: identity_of(done.oauth_account)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_profile_maps_onto_the_block_claude_code_writes() {
        let p = json!({
            "account": {"uuid": "a-1", "email": "me@example.com", "display_name": "", "full_name": "Me", "created_at": "2025-01-01"},
            "organization": {"uuid": "o-1", "organization_type": "claude_max", "rate_limit_tier": "default_claude_max_20x", "billing_type": "stripe_subscription"}
        });
        let a = oauth_account_from_profile(&p).unwrap();
        assert_eq!(a["accountUuid"], "a-1");
        assert_eq!(a["emailAddress"], "me@example.com");
        assert_eq!(a["organizationUuid"], "o-1");
        assert!(a.get("displayName").is_none());
        assert_eq!(a["fullName"], "Me");
        assert_eq!(a["hasExtraUsageEnabled"], false);
        assert_eq!(a["billingType"], "stripe_subscription");
    }

    #[test]
    fn a_refresh_keeps_the_old_refresh_token_when_none_comes_back() {
        let mut oauth = Map::new();
        oauth.insert("refreshToken".into(), json!("old"));
        oauth.insert("subscriptionType".into(), json!("max"));
        let t: TokenResponse = serde_json::from_value(json!({
            "access_token": "new-access", "expires_in": 3600, "scope": "user:inference user:profile"
        }))
        .unwrap();
        apply_tokens(&mut oauth, &t);
        assert_eq!(oauth["refreshToken"], "old");
        assert_eq!(oauth["accessToken"], "new-access");
        assert_eq!(oauth["subscriptionType"], "max");
        assert_eq!(oauth["scopes"], json!(["user:inference", "user:profile"]));
    }

    #[test]
    fn a_failing_token_endpoint_says_what_it_answered() {
        let mut server = mockito::Server::new();
        server
            .mock("POST", "/v1/oauth/token")
            .with_status(503)
            .with_body("upstream down")
            .create();
        let mut oauth = Map::new();
        oauth.insert("refreshToken".into(), json!("r"));
        let err = refresh(&Api::local(&server.url()), &mut oauth).unwrap_err();
        assert!(err.to_string().contains("503"), "{err}");
        assert!(err.to_string().contains("upstream down"), "{err}");
        assert_eq!(oauth["refreshToken"], "r");
    }

    #[test]
    fn every_organization_type_maps_onto_a_plan() {
        for (org, plan) in [
            ("claude_max", json!("max")),
            ("claude_pro", json!("pro")),
            ("claude_team", json!("team")),
            ("claude_enterprise", json!("enterprise")),
            ("api", Value::Null),
        ] {
            assert_eq!(subscription_type(Some(org)), plan, "{org}");
        }
    }

    #[test]
    fn the_authorize_url_carries_pkce() {
        let pkce = start_login().unwrap();
        let url = reqwest::Url::parse(&pkce.url).unwrap();
        let q: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(q["code_challenge_method"], "S256");
        assert_eq!(q["state"], pkce.state);
        assert_eq!(q["code_challenge"], crate::pkce::challenge(&pkce.verifier));
    }
}
