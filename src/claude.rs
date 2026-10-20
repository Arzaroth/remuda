use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use selvedge::plans::{Seat, team_seat, tier_multiplier};
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::fsx::{Changed, now_ms, read_json, update_json};
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
const API_URL: &str = "https://api.anthropic.com";
const CLI_USER_AGENT: &str = "claude-cli/2.1.304 (external, cli)";
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
    api_url: String,
}

impl Api {
    pub fn claude() -> Result<Self> {
        #[cfg(debug_assertions)]
        if let Some(base) = std::env::var_os("REMUDA_TEST_CLAUDE_API") {
            let base = base.to_string_lossy();
            return Self::at(&format!("{base}/v1/oauth/token"), &base);
        }
        Self::at(TOKEN_URL, API_URL)
    }

    fn at(token_url: &str, api_url: &str) -> Result<Self> {
        Ok(Api {
            client: oauth::client()?,
            token_url: token_url.to_owned(),
            api_url: api_url.to_owned(),
        })
    }

    #[cfg(test)]
    pub fn local(base: &str) -> Self {
        Self::at(&format!("{base}/v1/oauth/token"), base).unwrap()
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
fn refresh(api: &Api, oauth: &mut Map<String, Value>) -> Result<Option<String>> {
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

fn profile(api: &Api, access_token: &str) -> Result<Value> {
    let resp = api
        .client
        .get(format!("{}/api/oauth/profile", api.api_url))
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

/// The `oauthAccount` block Claude Code writes into `.claude.json` from a profile.
fn oauth_account_from_profile(p: &Value) -> Result<Value> {
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

/// The banked reset to spend: the one the usage endpoint names next, else
/// any it would accept now.
fn usable_grant(cedar_ember: &Value) -> Option<&str> {
    if cedar_ember.get("eligible").and_then(Value::as_bool) != Some(true) {
        return None;
    }
    let at_limit = cedar_ember.get("at_limit").and_then(Value::as_bool) == Some(true);
    let flag = |g: &Value, k: &str| g.get(k).and_then(Value::as_bool) == Some(true);
    let usable: Vec<&str> = cedar_ember
        .get("grants")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|g| {
            g.get("resets_left").and_then(Value::as_f64).unwrap_or(0.0) >= 1.0
                && !flag(g, "paused")
                && flag(g, "usable_now")
                && (at_limit || !flag(g, "use_requires_limit"))
        })
        .filter_map(|g| g.get("id")?.as_str())
        .collect();
    let next = cedar_ember.get("next_grant_id").and_then(Value::as_str);
    next.filter(|n| usable.contains(n))
        .or_else(|| usable.first().copied())
}

fn request_id() -> Result<String> {
    let mut b = [0u8; 16];
    getrandom::fill(&mut b).map_err(|e| anyhow::anyhow!("no randomness: {e}"))?;
    b[6] = b[6] & 0x0f | 0x40;
    b[8] = b[8] & 0x3f | 0x80;
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

/// Spends one of the login's banked usage-limit resets and returns what the
/// endpoint says came of it.
fn redeem_reset(api: &Api, access_token: &str, organization: &str) -> Result<String> {
    let usage = api
        .client
        .get(format!("{}/api/oauth/usage?cedar_ember=1", api.api_url))
        .bearer_auth(access_token)
        .header("anthropic-beta", "oauth-2025-04-20")
        .header("User-Agent", CLI_USER_AGENT)
        .send()
        .context("usage request failed")?;
    let status = usage.status();
    if !status.is_success() {
        bail!("usage endpoint answered {status}");
    }
    let usage: Value = usage.json().context("malformed usage response")?;
    let grant = usage
        .get("cedar_ember")
        .and_then(usable_grant)
        .context("no banked reset can be used right now")?;
    let resp = api
        .client
        .post(format!(
            "{}/api/organizations/{organization}/reset_rate_limits",
            api.api_url
        ))
        .bearer_auth(access_token)
        .header("anthropic-beta", "oauth-2025-04-20")
        .header("User-Agent", CLI_USER_AGENT)
        .json(&json!({"program": "cedar_ember", "grant_id": grant, "request_id": request_id()?}))
        .send()
        .context("reset request failed, and it may have gone through: check the card once TokenGauge updates before trying again")?;
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().unwrap_or_default();
        bail!(
            "reset endpoint answered {status}: {}",
            body.chars().take(200).collect::<String>()
        );
    }
    let answer: Value = resp.json().unwrap_or(Value::Null);
    Ok(answer
        .get("result")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_owned())
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

struct Pkce {
    pub url: String,
    verifier: String,
    state: String,
}

fn start_login() -> Result<Pkce> {
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

struct ClaudeLogin {
    pub oauth: Value,
    pub oauth_account: Value,
}

/// Exchanges the `code#state` string the callback page shows.
fn finish_login(api: &Api, pkce: &Pkce, pasted: &str) -> Result<ClaudeLogin> {
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

    fn home(&self) -> PathBuf {
        self.creds_path
            .parent()
            .map(PathBuf::from)
            .unwrap_or_default()
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
        let Some(sub) = get("subscriptionType") else {
            return "-".to_owned();
        };
        match get("rateLimitTier").and_then(tier_multiplier) {
            Some(n) if !is_team(sub) => format!("{sub} {n}x"),
            _ => sub.to_owned(),
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

    fn identify(&self, creds: &Value) -> Result<Identity> {
        let access = self
            .access_token(creds)
            .context("credential has no access token")?;
        identity_of(oauth_account_from_profile(&profile(&self.api, access)?)?)
    }

    /// Writes the account first and the login second: Claude Code adopts a
    /// login when the credentials file changes, and by then the account the
    /// UI shows is already the new one. Every other key in both files is kept.
    fn install(&self, entry: &Entry, outgoing: Option<&Value>) -> Result<()> {
        let oauth = Self::oauth(&entry.creds)
            .context("stored credential has no login")?
            .clone();
        let account = entry
            .meta
            .oauth_account
            .clone()
            .context("stored credential has no oauthAccount")?;
        for path in [&self.config_path, &self.creds_path] {
            if read_json(path)?.is_some_and(|v| !v.is_object()) {
                bail!("{} is not a JSON object", path.display());
            }
        }
        let expected = outgoing.and_then(|o| o.get(OAUTH_KEY));
        let previous = read_json(&self.config_path)?.and_then(|c| c.get("oauthAccount").cloned());
        update_json(&self.config_path, |config| {
            config["oauthAccount"] = account.clone();
            Ok(())
        })?;
        let written = update_json(&self.creds_path, |file| {
            if expected.is_some_and(|e| file.get(OAUTH_KEY) != Some(e)) {
                return Err(Changed.into());
            }
            file[OAUTH_KEY] = Value::Object(oauth.clone());
            Ok(())
        });
        if written.is_err() {
            let _ = update_json(&self.config_path, |config| {
                if config.get("oauthAccount") == Some(&account) {
                    match &previous {
                        Some(p) => config["oauthAccount"] = p.clone(),
                        None => {
                            config.as_object_mut().map(|c| c.remove("oauthAccount"));
                        }
                    }
                }
                Ok(())
            });
        }
        written
    }

    fn refresh(&self, creds: &mut Value) -> Result<Option<String>> {
        let oauth = creds
            .get_mut(OAUTH_KEY)
            .and_then(Value::as_object_mut)
            .context("stored credential has no login")?;
        refresh(&self.api, oauth)
    }

    fn redeem_reset(&self, entry: &Entry) -> Result<String> {
        if self
            .expires_at(&entry.creds)
            .is_some_and(|at| at <= now_ms())
        {
            bail!(
                "{}'s access token has expired: refresh it, or run claude if it is the login in use, then try again",
                entry.qualified()
            );
        }
        let access = self
            .access_token(&entry.creds)
            .context("stored credential has no access token")?;
        let organization = account_field(entry, "organizationUuid")
            .filter(|o| !o.is_empty() && o.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
            .context("stored credential names no organization")?;
        let result = redeem_reset(&self.api, access, organization)?;
        Ok(format!("{}: banked reset {result}", entry.qualified()))
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

fn is_team(subscription: &str) -> bool {
    subscription.to_ascii_lowercase().contains("team")
}

/// Whether a stored login holds a Team seat: its tokens say `team`, or say
/// nothing and its profile reported a Team seat tier. A token that names
/// another plan has left Team, whatever seat its profile last reported.
pub fn on_team(e: &Entry) -> bool {
    match token_field(e, "subscriptionType").map(str::trim) {
        Some(sub) if !sub.is_empty() => is_team(sub),
        _ => reported_seat(e).is_some_and(|t| t.starts_with("team_")),
    }
}

fn token_field<'a>(e: &'a Entry, key: &str) -> Option<&'a str> {
    Claude::oauth(&e.creds)?.get(key)?.as_str()
}

fn account_field<'a>(e: &'a Entry, key: &str) -> Option<&'a str> {
    e.meta.oauth_account.as_ref()?.get(key)?.as_str()
}

/// The seat a Team login holds, read off its seat tier and every rate-limit
/// tier it carries (`selvedge::plans::team_seat`). `None` off a Team plan, and
/// for a seat tier nothing says the weight of.
pub fn seat(e: &Entry) -> Option<Seat> {
    if !on_team(e) {
        return None;
    }
    team_seat(
        account_field(e, "seatTier"),
        &[
            account_field(e, "userRateLimitTier"),
            token_field(e, "rateLimitTier")
                .filter(|_| token_field(e, "subscriptionType").is_some_and(is_team)),
        ],
    )
}

/// A seat as the user reads it.
pub fn seat_word(seat: Seat) -> &'static str {
    match seat {
        Seat::Standard => "standard",
        Seat::Premium => "premium",
    }
}

/// The seat tier the profile reported when the login was captured.
fn reported_seat(e: &Entry) -> Option<&str> {
    account_field(e, "seatTier").filter(|t| !t.trim().is_empty())
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

    fn grant(id: &str) -> Value {
        json!({"id": id, "resets_left": 1, "paused": false, "usable_now": true, "use_requires_limit": false})
    }

    fn banked(grants: Vec<Value>) -> Value {
        json!({"eligible": true, "at_limit": false, "grants": grants, "next_grant_id": null})
    }

    #[test]
    fn only_a_grant_the_endpoint_would_accept_now_is_spent() {
        assert_eq!(usable_grant(&banked(vec![grant("g")])), Some("g"));
        assert_eq!(usable_grant(&json!({})), None);
        let mut ineligible = banked(vec![grant("g")]);
        ineligible["eligible"] = json!(false);
        assert_eq!(usable_grant(&ineligible), None);
        for (key, value) in [
            ("resets_left", json!(0)),
            ("paused", json!(true)),
            ("usable_now", json!(false)),
            ("use_requires_limit", json!(true)),
        ] {
            let mut g = grant("g");
            g[key] = value;
            assert_eq!(usable_grant(&banked(vec![g])), None, "{key}");
        }
        let mut at_limit = grant("g");
        at_limit["use_requires_limit"] = json!(true);
        let mut wall = banked(vec![at_limit]);
        wall["at_limit"] = json!(true);
        assert_eq!(usable_grant(&wall), Some("g"));
    }

    #[test]
    fn the_grant_the_endpoint_names_next_goes_first_while_it_is_usable() {
        let mut both = banked(vec![grant("a"), grant("b")]);
        both["next_grant_id"] = json!("b");
        assert_eq!(usable_grant(&both), Some("b"));
        both["next_grant_id"] = json!("retired");
        assert_eq!(usable_grant(&both), Some("a"));
    }

    #[test]
    fn a_request_id_is_a_v4_uuid() {
        let id = request_id().unwrap();
        let parts: Vec<_> = id.split('-').map(str::len).collect();
        assert_eq!(parts, [8, 4, 4, 4, 12], "{id}");
        assert_eq!(&id[14..15], "4");
        assert!("89ab".contains(&id[19..20]), "{id}");
        assert_ne!(id, request_id().unwrap());
    }

    #[test]
    fn an_expired_token_spends_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let claude = Claude::at(tmp.path(), Api::local("http://127.0.0.1:9"));
        let mut identity = crate::ops::testing::identity("u-work");
        identity.oauth_account.as_mut().unwrap()["organizationUuid"] = json!("o-work");
        let creds = crate::ops::testing::oauth("a-work", "r-work", -1000);
        let entry = Entry::new("claude", "work", creds, identity, 1);
        let err = claude.redeem_reset(&entry).unwrap_err();
        assert!(err.to_string().contains("expired"), "{err}");
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
