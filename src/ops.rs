use anyhow::{Context, Result, bail};

use crate::fsx::now_ms;
use crate::provider::{Identity, Provider};
use crate::store::{Entry, Store};
use serde_json::Value;

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
    /// A login remuda cannot store, such as an API key.
    Foreign {
        what: String,
    },
    /// The CLI's files could not be read, so which credential is active is
    /// unknown.
    Unreadable {
        error: String,
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

/// Re-identifies each credential whose sidecar was written for other tokens (a
/// save interrupted between its two renames) and rewrites the sidecar for the
/// tokens actually there. One that cannot be identified yet stays unverified,
/// and nothing refreshes, switches to or matches it.
pub fn heal(store: &Store, p: &dyn Provider, live: Option<&Value>) -> Result<()> {
    let live_refresh = live.and_then(|l| p.refresh_token(l));
    for mut entry in store.list(p.id())?.into_iter().filter(|e| !e.verified) {
        let identified = match p.identify(&entry.creds) {
            Ok(id) => Ok(id),
            // Its tokens are the CLI's own: rotating them would sign the CLI
            // out, so it waits until they can be identified as they are.
            Err(err) if live_refresh.is_some() && p.refresh_token(&entry.creds) == live_refresh => {
                Err(err)
            }
            // An expired access token cannot be identified, so it is refreshed
            // and the new one asked instead. The old refresh token is spent
            // either way, so the result is saved whoever it belongs to.
            Err(_) => match p.refresh(&mut entry.creds) {
                Ok(answered) => Ok(p.identify(&entry.creds).unwrap_or_else(|_| Identity {
                    account_id: answered.unwrap_or_else(|| entry.meta.account_id.clone()),
                    email: String::new(),
                    oauth_account: None,
                })),
                Err(err) => Err(err),
            },
        };
        match identified {
            Ok(id) => {
                if id.account_id != entry.meta.account_id {
                    eprintln!(
                        "note: {} held another account's tokens than its sidecar said; it now says {}",
                        entry.qualified(),
                        if id.email.is_empty() {
                            &id.account_id
                        } else {
                            &id.email
                        }
                    );
                    entry.meta.email = String::new();
                    entry.meta.oauth_account = None;
                }
                entry.meta.account_id = id.account_id;
                if !id.email.is_empty() {
                    entry.meta.email = id.email;
                }
                if id.oauth_account.is_some() {
                    entry.meta.oauth_account = id.oauth_account;
                }
                entry.verified = true;
                store.save(&entry)?;
            }
            Err(err) => eprintln!(
                "warning: {} does not match its sidecar and cannot be identified yet: {err:#}",
                entry.qualified()
            ),
        }
    }
    Ok(())
}

/// Copies the live login back into the entry it belongs to. A CLI rotates the
/// refresh token of the account it is signed into, which leaves the stored
/// copy dead unless it is brought forward before anything else reads it.
pub fn sync_live(store: &Store, p: &dyn Provider) -> Result<LiveState> {
    let live = p.live()?;
    heal(store, p, live.as_ref())?;
    let Some(creds) = live else {
        return Ok(match p.foreign_login()? {
            Some(what) => LiveState::Foreign { what },
            None => LiveState::SignedOut,
        });
    };
    let identity = p.live_identity(&creds)?;
    let all = store.list(p.id())?;

    let refresh = p.refresh_token(&creds);
    let access = p.access_token(&creds);
    let by_token = all.iter().find(|e| {
        (refresh.is_some() && p.refresh_token(&e.creds) == refresh)
            || (access.is_some() && p.access_token(&e.creds) == access)
    });
    // The live login is this one's tokens, but whose they are is not settled:
    // it is neither refreshed nor overwritten, and a switch asks for --discard.
    if let Some(e) = by_token.filter(|e| !e.verified) {
        return Ok(LiveState::Stored {
            name: e.name.clone(),
            synced: false,
        });
    }
    let entries: Vec<&Entry> = all.iter().filter(|e| e.verified).collect();
    let found = match by_token {
        Some(e) => e,
        None if entries.is_empty() => {
            return Ok(LiveState::Unstored {
                email: identity.map(|id| id.email),
            });
        }
        None => match p.identify(&creds) {
            Ok(confirmed) => {
                match entries
                    .iter()
                    .find(|e| e.meta.account_id == confirmed.account_id)
                {
                    Some(e) => e,
                    None => {
                        return Ok(LiveState::Unstored {
                            email: Some(confirmed.email),
                        });
                    }
                }
            }
            Err(err) => {
                let by_account = identity
                    .as_ref()
                    .and_then(|id| entries.iter().find(|e| e.meta.account_id == id.account_id));
                return Ok(match by_account {
                    Some(e) => {
                        eprintln!(
                            "warning: could not confirm the live login is {}: {err:#}",
                            e.qualified()
                        );
                        LiveState::Stored {
                            name: e.name.clone(),
                            synced: false,
                        }
                    }
                    None => LiveState::Unstored {
                        email: identity.map(|id| id.email),
                    },
                });
            }
        },
    };
    let mut entry = found.clone();
    let mut changed = false;
    let older = matches!(
        (p.expires_at(&creds), p.expires_at(&entry.creds)),
        (Some(live), Some(stored)) if live < stored
    );
    if older {
        eprintln!(
            "warning: {} is newer in the store than in {}; keeping the store's copy",
            entry.qualified(),
            p.name()
        );
    } else if entry.creds != creds {
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

/// Leaves `entry` unusable on error; only save it on success. Tokens that
/// turn out to belong to another account are already rotated by then, so they
/// are kept aside rather than dropped.
pub fn refresh_entry(store: &Store, p: &dyn Provider, entry: &mut Entry) -> Result<()> {
    if let Some(account) = p.refresh(&mut entry.creds)?
        && account != entry.meta.account_id
    {
        let kept = store.set_aside(p.id(), &account, &entry.creds)?;
        bail!(
            "the token endpoint answered for account {account}, not {}; its new tokens are in {}",
            entry.meta.account_id,
            kept.display()
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
        LiveState::Foreign { what } if !discard => {
            bail!(
                "{} is signed in with {what}, which remuda cannot store; pass --discard to replace it",
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
    target = store
        .get(p.id(), name)?
        .with_context(|| format!("no credential named {}/{name}", p.id()))?;
    if !target.verified {
        bail!(
            "{} does not match its sidecar; run `remuda ls` online to identify it first",
            target.qualified()
        );
    }
    let refreshed = expires_in_ms(p, &target).is_some_and(|ms| ms < 5 * 60 * 1000);
    if refreshed {
        refresh_entry(store, p, &mut target)?;
        store.save(&target)?;
    }
    let _live = p.lock_live()?;
    let mut attempts = 0;
    loop {
        // The CLI may rotate its own login at any moment, so the login being
        // replaced is read again right before, and the write is refused if it
        // moved in between; its new tokens are then synced back first.
        if refreshed || attempts > 0 {
            sync_live(store, p)?;
        }
        let outgoing = if discard { None } else { p.live()? };
        match p.install(&target, outgoing.as_ref()) {
            Err(e) if e.is::<crate::fsx::Changed>() && attempts < 3 => attempts += 1,
            result => break result?,
        }
    }
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
    /// A loopback port nothing listens on: bound once to reserve a number,
    /// then released, so connecting is refused at once.
    pub static OFFLINE: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        format!("http://{}", listener.local_addr().unwrap())
    });

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
        let e = env(&OFFLINE);
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
    fn tokens_confirmed_as_another_stored_account_are_filed_there() {
        let server = confirming("u-a");
        let e = env(&server.url());
        e.stored("a", "u-a", HOUR);
        e.stored("b", "u-b", HOUR);
        e.sign_in(oauth("a2", "r2", 2 * HOUR), account("u-b"));

        let state = sync_live(&e.store, &e.claude).unwrap();

        assert_eq!(
            state,
            LiveState::Stored {
                name: "a".into(),
                synced: true
            }
        );
        assert_eq!(e.stored_refresh_token("a").as_deref(), Some("r2"));
        assert_eq!(e.stored_refresh_token("b").as_deref(), Some("r-b"));
    }

    #[test]
    fn an_older_live_copy_never_overwrites_a_newer_stored_one() {
        let server = confirming("u-work");
        let e = env(&server.url());
        e.stored("work", "u-work", 8 * HOUR);
        e.sign_in(oauth("a-old", "r-old", HOUR), account("u-work"));

        let state = sync_live(&e.store, &e.claude).unwrap();

        assert!(matches!(state, LiveState::Stored { synced: true, .. }));
        assert_eq!(e.stored_refresh_token("work").as_deref(), Some("r-work"));
    }

    fn tear(e: &Env, name: &str, access: &str) {
        let path = e.store.root().join(format!("claude/{name}.json"));
        crate::fsx::write_json(&path, &oauth(access, &format!("r-{access}"), HOUR)).unwrap();
    }

    #[test]
    fn a_torn_credential_is_relabelled_for_the_account_it_holds() {
        let server = confirming("u-real");
        let e = env(&server.url());
        e.stored("work", "u-work", HOUR);
        tear(&e, "work", "a-real");

        heal(&e.store, &e.claude, None).unwrap();

        let work = e.store.get("claude", "work").unwrap().unwrap();
        assert!(work.verified);
        assert_eq!(work.meta.account_id, "u-real");
        assert_eq!(work.meta.email, "u-real@example.com");
    }

    #[test]
    fn an_unidentified_torn_credential_is_not_switched_to() {
        let e = env(&OFFLINE);
        e.stored("work", "u-work", -HOUR);
        tear(&e, "work", "a-torn");

        let err = switch(&e.store, &e.claude, "work", true).unwrap_err();
        assert!(
            err.to_string().contains("does not match its sidecar"),
            "{err}"
        );
        assert!(e.claude.live().unwrap().is_none());
    }

    #[test]
    fn a_torn_credential_is_healed_even_while_the_cli_is_signed_out() {
        let server = confirming("u-work");
        let e = env(&server.url());
        e.stored("work", "u-work", HOUR);
        tear(&e, "work", "a-torn");

        assert_eq!(
            sync_live(&e.store, &e.claude).unwrap(),
            LiveState::SignedOut
        );

        assert!(e.store.get("claude", "work").unwrap().unwrap().verified);
    }

    #[test]
    fn an_expired_torn_credential_is_refreshed_to_be_identified() {
        let mut server = mockito::Server::new();
        server
            .mock("GET", "/api/oauth/profile")
            .match_header("authorization", "Bearer a-torn")
            .with_status(401)
            .create();
        server
            .mock("POST", "/v1/oauth/token")
            .with_body(
                json!({"access_token": "a-new", "refresh_token": "r-new", "expires_in": 28800,
                       "account": {"uuid": "u-other"}})
                .to_string(),
            )
            .expect(1)
            .create();
        server
            .mock("GET", "/api/oauth/profile")
            .match_header("authorization", "Bearer a-new")
            .with_body(profile_body("u-other"))
            .create();
        let e = env(&server.url());
        e.stored("work", "u-work", HOUR);
        tear(&e, "work", "a-torn");

        heal(&e.store, &e.claude, None).unwrap();

        let work = e.store.get("claude", "work").unwrap().unwrap();
        assert!(work.verified);
        assert_eq!(work.meta.account_id, "u-other");
        assert_eq!(work.meta.email, "u-other@example.com");
        assert_eq!(e.stored_refresh_token("work").as_deref(), Some("r-new"));
    }

    #[test]
    fn a_torn_credential_holding_the_live_login_is_kept_and_never_rotated() {
        let e = env(&OFFLINE);
        e.stored("work", "u-work", HOUR);
        e.stored("perso", "u-perso", HOUR);
        tear(&e, "work", "a-live");
        let torn = e.store.get("claude", "work").unwrap().unwrap();
        e.sign_in(torn.creds, account("u-work"));

        let state = sync_live(&e.store, &e.claude).unwrap();

        assert_eq!(
            state,
            LiveState::Stored {
                name: "work".into(),
                synced: false
            }
        );
        let err = switch(&e.store, &e.claude, "perso", false).unwrap_err();
        assert!(err.to_string().contains("could not be confirmed"), "{err}");
        assert_eq!(e.live_refresh_token().as_deref(), Some("r-a-live"));
    }

    #[test]
    fn a_changed_profile_travels_back_with_the_tokens() {
        let e = env(&OFFLINE);
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
        let e = env(&OFFLINE);
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
    fn a_corrupt_credentials_file_stops_a_switch_before_anything_is_written() {
        let e = env(&OFFLINE);
        e.stored("perso", "u-perso", HOUR);
        e.sign_in(oauth("a", "r", HOUR), account("u-work"));
        std::fs::write(e.tmp.path().join(".credentials.json"), "[1, 2]").unwrap();
        let config = std::fs::read_to_string(e.tmp.path().join(".claude.json")).unwrap();

        let perso = e.store.get("claude", "perso").unwrap().unwrap();
        let err = e.claude.install(&perso, None).unwrap_err();

        assert!(err.to_string().contains("not a JSON object"), "{err}");
        assert_eq!(
            std::fs::read_to_string(e.tmp.path().join(".claude.json")).unwrap(),
            config
        );
        assert_eq!(
            std::fs::read_to_string(e.tmp.path().join(".credentials.json")).unwrap(),
            "[1, 2]"
        );
    }

    #[test]
    fn an_install_refuses_a_live_login_that_moved_and_undoes_its_first_half() {
        let e = env(&OFFLINE);
        e.stored("perso", "u-perso", HOUR);
        let seen = oauth("a1", "r1", HOUR);
        e.sign_in(oauth("a2", "r2-rotated", HOUR), account("u-work"));
        let config_before = std::fs::read_to_string(e.tmp.path().join(".claude.json")).unwrap();

        let perso = e.store.get("claude", "perso").unwrap().unwrap();
        let err = e.claude.install(&perso, Some(&seen)).unwrap_err();

        assert!(err.is::<crate::fsx::Changed>(), "{err}");
        assert_eq!(e.live_refresh_token().as_deref(), Some("r2-rotated"));
        let config: Value = serde_json::from_str(
            &std::fs::read_to_string(e.tmp.path().join(".claude.json")).unwrap(),
        )
        .unwrap();
        let before: Value = serde_json::from_str(&config_before).unwrap();
        assert_eq!(config["oauthAccount"], before["oauthAccount"]);
    }

    #[test]
    fn switching_away_from_a_login_nobody_stored_is_refused() {
        let e = env(&OFFLINE);
        e.stored("perso", "u-perso", HOUR);
        e.sign_in(oauth("x", "y", HOUR), account("u-new"));

        let err = switch(&e.store, &e.claude, "perso", false).unwrap_err();

        assert!(err.to_string().contains("not in the store"), "{err}");
        assert_eq!(e.live_refresh_token().as_deref(), Some("y"));
    }

    #[test]
    fn a_live_login_that_could_not_be_confirmed_is_not_switched_away_from() {
        let e = env(&OFFLINE);
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
        let e = env(&OFFLINE);
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
        let err = refresh_entry(&e.store, &e.claude, &mut entry).unwrap_err();
        assert!(err.to_string().contains("u-someone"), "{err}");
        let kept: Vec<_> = std::fs::read_dir(e.store.root().join("claude"))
            .unwrap()
            .filter_map(|f| f.ok()?.file_name().into_string().ok())
            .filter(|f| f.starts_with(".set-aside-u-someone-"))
            .collect();
        assert_eq!(kept.len(), 1, "{kept:?}");
        assert_eq!(e.store.list("claude").unwrap().len(), 1);
    }
}
