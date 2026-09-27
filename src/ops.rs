use anyhow::{Context, Result, bail};

use crate::fsx::now_ms;
use crate::provider::Provider;
use crate::store::{Entry, Store};

#[derive(Debug, PartialEq)]
pub enum LiveState {
    SignedOut,
    /// `synced` is false when the live login could not be tied to this entry
    /// with certainty, so its tokens were not copied back.
    Stored {
        name: String,
        synced: bool,
    },
    Unstored {
        email: Option<String>,
    },
}

impl LiveState {
    pub fn active_name(&self) -> Option<&str> {
        match self {
            LiveState::Stored { name, .. } => Some(name),
            _ => None,
        }
    }
}

/// Copies the live login back into the entry it belongs to. A CLI rotates the
/// refresh token of the account it is signed into, which leaves the stored
/// copy dead unless it is brought forward before anything else reads it.
pub fn sync_live(store: &Store, p: &dyn Provider) -> Result<LiveState> {
    let Some(creds) = p.live()? else {
        return Ok(LiveState::SignedOut);
    };
    let identity = p.live_identity(&creds)?;
    let entries = store.list(p.id())?;

    let refresh = p.refresh_token(&creds);
    let access = p.access_token(&creds);
    let by_token = entries.iter().find(|e| {
        (refresh.is_some() && p.refresh_token(&e.creds) == refresh)
            || (access.is_some() && p.access_token(&e.creds) == access)
    });
    let found = match by_token {
        Some(e) => e,
        None => {
            let by_account = identity
                .as_ref()
                .and_then(|id| entries.iter().find(|e| e.meta.account_id == id.account_id));
            let Some(e) = by_account else {
                return Ok(LiveState::Unstored {
                    email: identity.map(|id| id.email),
                });
            };
            match p.confirm(&creds) {
                Ok(account) if account == e.meta.account_id => e,
                Ok(_) => return Ok(LiveState::Unstored { email: None }),
                Err(err) => {
                    eprintln!(
                        "warning: could not confirm the live login is {}: {err:#}",
                        e.qualified()
                    );
                    return Ok(LiveState::Stored {
                        name: e.name.clone(),
                        synced: false,
                    });
                }
            }
        }
    };
    let mut entry = found.clone();
    let mut changed = false;
    if entry.creds != creds {
        entry.creds = creds;
        changed = true;
    }
    if let Some(id) = identity.filter(|id| id.account_id == entry.meta.account_id) {
        if id.oauth_account.is_some() && entry.meta.oauth_account != id.oauth_account {
            entry.meta.oauth_account = id.oauth_account;
            changed = true;
        }
        if !id.email.is_empty() && entry.meta.email != id.email {
            entry.meta.email = id.email;
            changed = true;
        }
    }
    if changed {
        store.save(&entry)?;
    }
    Ok(LiveState::Stored {
        name: entry.name,
        synced: true,
    })
}

pub fn expires_in_ms(p: &dyn Provider, entry: &Entry) -> Option<i64> {
    p.expires_at(&entry.creds).map(|at| at - now_ms())
}

/// Leaves `entry` unusable on error; only save it on success.
pub fn refresh_entry(p: &dyn Provider, entry: &mut Entry) -> Result<()> {
    if let Some(account) = p.refresh(&mut entry.creds)?
        && account != entry.meta.account_id
    {
        bail!(
            "the token endpoint answered for account {account}, not {}",
            entry.meta.account_id
        );
    }
    Ok(())
}

/// Returns what to tell the user.
pub fn switch(store: &Store, p: &dyn Provider, name: &str, discard: bool) -> Result<String> {
    let mut target = store
        .get(p.id(), name)?
        .with_context(|| format!("no credential named {}/{name}", p.id()))?;
    match sync_live(store, p)? {
        LiveState::Stored { name: current, .. } if current == name => {
            return Ok(format!("{} is already active", target.qualified()));
        }
        LiveState::Stored {
            synced: false,
            name: current,
        } if !discard => {
            bail!(
                "the live {} login looks like {current} but could not be confirmed, so switching could lose its newest tokens; retry online or pass --discard",
                p.name()
            );
        }
        LiveState::Unstored { email } if !discard => {
            let who = email.map(|e| format!(" ({e})")).unwrap_or_default();
            bail!(
                "the live {} login{who} is not in the store: `remuda import` it first, or pass --discard to drop it",
                p.name()
            );
        }
        _ => {}
    }
    if expires_in_ms(p, &target).is_some_and(|ms| ms < 5 * 60 * 1000) {
        refresh_entry(p, &mut target)?;
        store.save(&target)?;
    }
    p.install(&target)?;
    Ok(format!(
        "switched {} to {} ({})",
        p.name(),
        target.name,
        target.meta.email
    ))
}

#[cfg(test)]
pub mod testing {
    use serde_json::{Value, json};

    use crate::claude::{Api, Claude, OAUTH_KEY};
    use crate::fsx::{now_ms, write_json};
    use crate::provider::{Identity, Provider};
    use crate::store::{Entry, Store};

    pub const HOUR: i64 = 3_600_000;
    pub const OFFLINE: &str = "http://127.0.0.1:9";

    pub struct Env {
        pub tmp: tempfile::TempDir,
        pub store: Store,
        pub claude: Claude,
    }

    pub fn env(api_base: &str) -> Env {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::open(&tmp.path().join("store"));
        let claude = Claude::at(tmp.path(), Api::local(api_base));
        Env { tmp, store, claude }
    }

    pub fn oauth(access: &str, refresh: &str, expires_in_ms: i64) -> Value {
        json!({ OAUTH_KEY: {
            "accessToken": access,
            "refreshToken": refresh,
            "expiresAt": now_ms() + expires_in_ms,
            "refreshTokenExpiresAt": now_ms() + 20 * 86_400_000 + HOUR,
            "scopes": ["user:inference", "user:profile"],
            "subscriptionType": "max",
            "rateLimitTier": "default_claude_max_5x",
        }})
    }

    pub fn account(uuid: &str) -> Value {
        json!({"accountUuid": uuid, "emailAddress": format!("{uuid}@example.com")})
    }

    pub fn identity(uuid: &str) -> Identity {
        Identity {
            account_id: uuid.into(),
            email: format!("{uuid}@example.com"),
            oauth_account: Some(account(uuid)),
        }
    }

    impl Env {
        pub fn stored(&self, name: &str, uuid: &str, expires_in_ms: i64) {
            let creds = oauth(&format!("a-{name}"), &format!("r-{name}"), expires_in_ms);
            self.store
                .save(&Entry::new("claude", name, creds, identity(uuid), 1))
                .unwrap();
        }

        pub fn sign_in(&self, creds: Value, account: Value) {
            let mut file = creds;
            file["mcpOAuth"] = json!({"srv": {"t": 1}});
            write_json(&self.tmp.path().join(".credentials.json"), &file).unwrap();
            write_json(
                &self.tmp.path().join(".claude.json"),
                &json!({"numStartups": 7, "oauthAccount": account}),
            )
            .unwrap();
        }

        pub fn live_refresh_token(&self) -> Option<String> {
            let live = self.claude.live().unwrap()?;
            self.claude.refresh_token(&live).map(str::to_owned)
        }

        pub fn stored_refresh_token(&self, name: &str) -> Option<String> {
            let entry = self.store.get("claude", name).unwrap()?;
            self.claude.refresh_token(&entry.creds).map(str::to_owned)
        }
    }

    pub fn profile_body(uuid: &str) -> String {
        json!({
            "account": {"uuid": uuid, "email": format!("{uuid}@example.com"), "full_name": "Someone", "created_at": "2025-01-01"},
            "organization": {"uuid": "o-1", "organization_type": "claude_pro", "rate_limit_tier": "default_claude_pro"}
        })
        .to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::testing::*;
    use super::*;
    use crate::claude::OAUTH_KEY;
    use crate::fsx::read_json;
    use serde_json::json;

    fn confirming(uuid: &str) -> mockito::ServerGuard {
        let mut server = mockito::Server::new();
        server
            .mock("GET", "/api/oauth/profile")
            .with_body(profile_body(uuid))
            .create();
        server
    }

    #[test]
    fn a_rotated_live_token_is_copied_back_once_the_account_is_confirmed() {
        let server = confirming("u-work");
        let e = env(&server.url());
        e.stored("work", "u-work", HOUR);
        e.sign_in(oauth("a2", "r2", HOUR), account("u-work"));

        let state = sync_live(&e.store, &e.claude).unwrap();

        assert_eq!(
            state,
            LiveState::Stored {
                name: "work".into(),
                synced: true
            }
        );
        assert_eq!(e.stored_refresh_token("work").as_deref(), Some("r2"));
    }

    #[test]
    fn an_unconfirmed_account_match_copies_nothing() {
        let e = env(OFFLINE);
        e.stored("work", "u-work", HOUR);
        e.sign_in(oauth("a2", "r2", HOUR), account("u-work"));

        let state = sync_live(&e.store, &e.claude).unwrap();

        assert_eq!(
            state,
            LiveState::Stored {
                name: "work".into(),
                synced: false
            }
        );
        assert_eq!(e.stored_refresh_token("work").as_deref(), Some("r-work"));
    }

    #[test]
    fn a_stale_account_block_does_not_file_tokens_under_the_wrong_name() {
        let server = confirming("u-other");
        let e = env(&server.url());
        e.stored("work", "u-work", HOUR);
        e.sign_in(oauth("a9", "r9", HOUR), account("u-work"));

        let state = sync_live(&e.store, &e.claude).unwrap();

        assert!(matches!(state, LiveState::Unstored { .. }));
        assert_eq!(e.stored_refresh_token("work").as_deref(), Some("r-work"));
    }

    #[test]
    fn a_changed_profile_travels_back_with_the_tokens() {
        let e = env(OFFLINE);
        e.stored("work", "u-work", HOUR);
        let mut renamed = account("u-work");
        renamed["displayName"] = json!("New name");
        let stored = e.store.get("claude", "work").unwrap().unwrap();
        e.sign_in(stored.creds, renamed);

        sync_live(&e.store, &e.claude).unwrap();

        let stored = e.store.get("claude", "work").unwrap().unwrap();
        assert_eq!(
            stored.meta.oauth_account.unwrap()["displayName"],
            "New name"
        );
    }

    #[test]
    fn switching_swaps_the_login_and_the_account_and_keeps_everything_else() {
        let e = env(OFFLINE);
        e.stored("work", "u-work", HOUR);
        e.stored("perso", "u-perso", HOUR);
        let work = e.store.get("claude", "work").unwrap().unwrap();
        e.sign_in(work.creds, account("u-work"));

        let said = switch(&e.store, &e.claude, "perso", false).unwrap();

        assert_eq!(said, "switched Claude Code to perso (u-perso@example.com)");
        let creds = read_json(&e.tmp.path().join(".credentials.json"))
            .unwrap()
            .unwrap();
        assert_eq!(creds[OAUTH_KEY]["refreshToken"], "r-perso");
        assert_eq!(creds["mcpOAuth"]["srv"]["t"], 1);
        let config = read_json(&e.tmp.path().join(".claude.json"))
            .unwrap()
            .unwrap();
        assert_eq!(config["oauthAccount"]["accountUuid"], "u-perso");
        assert_eq!(config["numStartups"], 7);
    }

    #[test]
    fn switching_away_from_a_login_nobody_stored_is_refused() {
        let e = env(OFFLINE);
        e.stored("perso", "u-perso", HOUR);
        e.sign_in(oauth("x", "y", HOUR), account("u-new"));

        let err = switch(&e.store, &e.claude, "perso", false).unwrap_err();

        assert!(err.to_string().contains("not in the store"), "{err}");
        assert_eq!(e.live_refresh_token().as_deref(), Some("y"));
    }

    #[test]
    fn a_live_login_that_could_not_be_confirmed_is_not_switched_away_from() {
        let e = env(OFFLINE);
        e.stored("work", "u-work", HOUR);
        e.stored("perso", "u-perso", HOUR);
        e.sign_in(oauth("a2", "r2", HOUR), account("u-work"));

        let err = switch(&e.store, &e.claude, "perso", false).unwrap_err();
        assert!(err.to_string().contains("could not be confirmed"), "{err}");
        assert_eq!(e.live_refresh_token().as_deref(), Some("r2"));

        switch(&e.store, &e.claude, "perso", true).unwrap();
        assert_eq!(e.live_refresh_token().as_deref(), Some("r-perso"));
    }

    #[test]
    fn an_expired_target_is_refreshed_before_it_goes_live() {
        let mut server = mockito::Server::new();
        server
            .mock("POST", "/v1/oauth/token")
            .with_body(
                json!({"access_token": "fresh", "expires_in": 28800, "account": {"uuid": "u-perso"}})
                    .to_string(),
            )
            .create();
        let e = env(&server.url());
        e.stored("perso", "u-perso", -1);

        switch(&e.store, &e.claude, "perso", false).unwrap();

        let live = e.claude.live().unwrap().unwrap();
        assert_eq!(e.claude.access_token(&live), Some("fresh"));
        let stored = e.store.get("claude", "perso").unwrap().unwrap();
        assert_eq!(e.claude.access_token(&stored.creds), Some("fresh"));
    }

    #[test]
    fn switching_to_the_active_credential_changes_nothing() {
        let e = env(OFFLINE);
        e.stored("work", "u-work", HOUR);
        let work = e.store.get("claude", "work").unwrap().unwrap();
        e.sign_in(work.creds, account("u-work"));
        assert_eq!(
            switch(&e.store, &e.claude, "work", false).unwrap(),
            "claude/work is already active"
        );
    }

    #[test]
    fn a_refresh_that_answers_for_another_account_is_refused() {
        let mut server = mockito::Server::new();
        server
            .mock("POST", "/v1/oauth/token")
            .with_body(
                json!({"access_token": "x", "expires_in": 60, "account": {"uuid": "u-someone"}})
                    .to_string(),
            )
            .create();
        let e = env(&server.url());
        e.stored("work", "u-work", HOUR);
        let mut entry = e.store.get("claude", "work").unwrap().unwrap();
        let err = refresh_entry(&e.claude, &mut entry).unwrap_err();
        assert!(err.to_string().contains("u-someone"), "{err}");
    }
}
