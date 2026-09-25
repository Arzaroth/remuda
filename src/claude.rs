use std::time::Duration;

use anyhow::{Context, Result, bail};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::Deserialize;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::fsx::now_ms;

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
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(30))
            .user_agent(concat!("remuda/", env!("CARGO_PKG_VERSION")))
            .build()
            .context("failed to build the HTTP client")?;
        Ok(Api {
            client,
            token_url: token_url.to_owned(),
            profile_url: profile_url.to_owned(),
        })
    }
}

impl crate::ops::WhoAmI for Api {
    fn account_uuid(&self, access_token: &str) -> Result<String> {
        let profile = profile(self, access_token)?;
        profile_account_uuid(&profile)
            .map(str::to_owned)
            .context("profile has no account uuid")
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
    let resp = api
        .client
        .post(&api.token_url)
        .json(body)
        .send()
        .context("token request failed")?;
    let status = resp.status();
    let text = resp.text().unwrap_or_default();
    if !status.is_success() {
        if text.contains("invalid_grant") {
            bail!("the refresh token was rejected (invalid_grant): log this credential in again");
        }
        bail!(
            "token endpoint answered {status}: {}",
            text.chars().take(200).collect::<String>()
        );
    }
    serde_json::from_str(&text).context("malformed token response")
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

fn random_b64() -> Result<String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|e| anyhow::anyhow!("no randomness: {e}"))?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

pub struct Pkce {
    pub url: String,
    verifier: String,
    state: String,
}

pub fn start_login() -> Result<Pkce> {
    let verifier = random_b64()?;
    let state = random_b64()?;
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
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

pub struct Login {
    pub oauth: Value,
    pub oauth_account: Value,
}

/// Exchanges the `code#state` string the callback page shows.
pub fn finish_login(api: &Api, pkce: &Pkce, pasted: &str) -> Result<Login> {
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
    Ok(Login {
        oauth: Value::Object(oauth),
        oauth_account: oauth_account_from_profile(&p)?,
    })
}
