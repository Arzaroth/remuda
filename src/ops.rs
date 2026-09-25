use anyhow::{Context, Result, bail};
use serde_json::Value;

use crate::fsx::now_ms;
use crate::live::Live;
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

pub trait WhoAmI {
    fn account_uuid(&self, access_token: &str) -> Result<String>;
}

/// Copies the live login back into the entry it belongs to. Claude Code
/// rotates the refresh token of the account it is signed into, which leaves
/// the stored copy dead unless it is brought forward before anything else
/// reads it.
pub fn sync_live(store: &Store, live: &Live, whoami: &dyn WhoAmI) -> Result<LiveState> {
    let Some(oauth) = live.oauth()? else {
        return Ok(LiveState::SignedOut);
    };
    let account = live.account()?;
    let account_uuid = account
        .as_ref()
        .and_then(|a| a.get("accountUuid"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    let token = |k: &str| oauth.get(k).and_then(Value::as_str);
    let entries = store.list()?;

    let by_token = entries.iter().find(|e| {
        (token("refreshToken").is_some() && e.token("refreshToken") == token("refreshToken"))
            || e.token("accessToken") == token("accessToken")
    });
    let found = match by_token {
        Some(e) => e,
        None => {
            let Some(e) = entries
                .iter()
                .find(|e| account_uuid.as_deref() == Some(e.meta.account_uuid.as_str()))
            else {
                return Ok(LiveState::Unstored {
                    email: account
                        .as_ref()
                        .and_then(|a| a.get("emailAddress"))
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                });
            };
            let access = token("accessToken").unwrap_or_default();
            match whoami.account_uuid(access) {
                Ok(uuid) if uuid == e.meta.account_uuid => e,
                Ok(_) => return Ok(LiveState::Unstored { email: None }),
                Err(err) => {
                    eprintln!(
                        "warning: could not confirm the live login is {}: {err:#}",
                        e.name
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
    if entry.oauth() != Some(&oauth) {
        *entry
            .creds
            .get_mut(crate::claude::OAUTH_KEY)
            .expect("entry has a login") = Value::Object(oauth);
        changed = true;
    }
    if let Some(account) =
        account.filter(|_| account_uuid.as_deref() == Some(entry.meta.account_uuid.as_str()))
        && entry.meta.oauth_account != account
    {
        entry.meta.oauth_account = account;
        changed = true;
    }
    if changed {
        store.save(&entry)?;
    }
    Ok(LiveState::Stored {
        name: entry.name,
        synced: true,
    })
}

pub fn expires_in_ms(entry: &Entry) -> Option<i64> {
    entry.millis("expiresAt").map(|at| at - now_ms())
}

pub fn switch(
    store: &Store,
    live: &Live,
    whoami: &dyn WhoAmI,
    name: &str,
    discard: bool,
    refresh: &dyn Fn(&mut Entry) -> Result<()>,
) -> Result<()> {
    let mut target = store
        .get(name)?
        .with_context(|| format!("no credential named {name}"))?;
    match sync_live(store, live, whoami)? {
        LiveState::Stored { name: current, .. } if current == name => {
            println!("{name} is already active");
            return Ok(());
        }
        LiveState::Stored {
            synced: false,
            name: current,
        } if !discard => {
            bail!(
                "the live login looks like {current} but could not be confirmed, so switching could lose its newest tokens; retry online or pass --discard"
            );
        }
        LiveState::Unstored { email } if !discard => {
            let who = email.map(|e| format!(" ({e})")).unwrap_or_default();
            bail!(
                "the live login{who} is not in the store: `remuda import <name>` it first, or pass --discard to drop it"
            );
        }
        _ => {}
    }
    if expires_in_ms(&target).is_some_and(|ms| ms < 5 * 60 * 1000) {
        refresh(&mut target)?;
        store.save(&target)?;
    }
    live.install(&target)?;
    println!("switched to {name} ({})", target.meta.email);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::claude::OAUTH_KEY;
    use crate::fsx::{read_json, write_json};
    use serde_json::json;

    struct Fixed(Result<&'static str, &'static str>);
    impl WhoAmI for Fixed {
        fn account_uuid(&self, _: &str) -> Result<String> {
            self.0.map(str::to_owned).map_err(|e| anyhow::anyhow!(e))
        }
    }
    const OFFLINE: Fixed = Fixed(Err("offline"));

    struct Env {
        _tmp: tempfile::TempDir,
        store: Store,
        live: Live,
    }

    fn env() -> Env {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::open(&tmp.path().join("store"));
        let live = Live {
            creds: tmp.path().join(".credentials.json"),
            config: tmp.path().join(".claude.json"),
        };
        Env {
            _tmp: tmp,
            store,
            live,
        }
    }

    fn oauth(access: &str, refresh: &str) -> Value {
        json!({"accessToken": access, "refreshToken": refresh, "expiresAt": now_ms() + 3_600_000})
    }

    fn account(uuid: &str) -> Value {
        json!({"accountUuid": uuid, "emailAddress": format!("{uuid}@example.com")})
    }

    fn sign_in(e: &Env, oauth: Value, account: Value) {
        write_json(
            &e.live.creds,
            &json!({OAUTH_KEY: oauth, "mcpOAuth": {"srv": {"t": 1}}}),
        )
        .unwrap();
        write_json(
            &e.live.config,
            &json!({"numStartups": 7, "oauthAccount": account}),
        )
        .unwrap();
    }

    #[test]
    fn a_rotated_live_token_is_copied_back_once_the_account_is_confirmed() {
        let e = env();
        e.store
            .save(&Entry::new("work", oauth("a1", "r1"), account("u-work"), 1).unwrap())
            .unwrap();
        sign_in(&e, oauth("a2", "r2"), account("u-work"));

        let state = sync_live(&e.store, &e.live, &Fixed(Ok("u-work"))).unwrap();

        assert_eq!(
            state,
            LiveState::Stored {
                name: "work".into(),
                synced: true
            }
        );
        assert_eq!(
            e.store.get("work").unwrap().unwrap().token("refreshToken"),
            Some("r2")
        );
    }

    #[test]
    fn an_unconfirmed_account_match_copies_nothing() {
        let e = env();
        e.store
            .save(&Entry::new("work", oauth("a1", "r1"), account("u-work"), 1).unwrap())
            .unwrap();
        sign_in(&e, oauth("a2", "r2"), account("u-work"));

        let state = sync_live(&e.store, &e.live, &OFFLINE).unwrap();

        assert_eq!(
            state,
            LiveState::Stored {
                name: "work".into(),
                synced: false
            }
        );
        assert_eq!(
            e.store.get("work").unwrap().unwrap().token("refreshToken"),
            Some("r1")
        );
    }

    #[test]
    fn a_stale_account_block_does_not_file_tokens_under_the_wrong_name() {
        let e = env();
        e.store
            .save(&Entry::new("work", oauth("a1", "r1"), account("u-work"), 1).unwrap())
            .unwrap();
        sign_in(&e, oauth("a9", "r9"), account("u-work"));

        let state = sync_live(&e.store, &e.live, &Fixed(Ok("u-other"))).unwrap();

        assert!(matches!(state, LiveState::Unstored { .. }));
        assert_eq!(
            e.store.get("work").unwrap().unwrap().token("refreshToken"),
            Some("r1")
        );
    }

    #[test]
    fn switching_swaps_the_login_and_the_account_and_keeps_everything_else() {
        let e = env();
        e.store
            .save(&Entry::new("work", oauth("a1", "r1"), account("u-work"), 1).unwrap())
            .unwrap();
        e.store
            .save(&Entry::new("perso", oauth("b1", "s1"), account("u-perso"), 1).unwrap())
            .unwrap();
        sign_in(&e, oauth("a2", "r2"), account("u-work"));

        switch(
            &e.store,
            &e.live,
            &Fixed(Ok("u-work")),
            "perso",
            false,
            &|_| unreachable!(),
        )
        .unwrap();

        let creds = read_json(&e.live.creds).unwrap().unwrap();
        assert_eq!(creds[OAUTH_KEY]["refreshToken"], "s1");
        assert_eq!(creds["mcpOAuth"]["srv"]["t"], 1);
        let config = read_json(&e.live.config).unwrap().unwrap();
        assert_eq!(config["oauthAccount"]["accountUuid"], "u-perso");
        assert_eq!(config["numStartups"], 7);
        assert_eq!(
            e.store.get("work").unwrap().unwrap().token("refreshToken"),
            Some("r2")
        );
    }

    #[test]
    fn switching_away_from_a_login_nobody_stored_is_refused() {
        let e = env();
        e.store
            .save(&Entry::new("perso", oauth("b1", "s1"), account("u-perso"), 1).unwrap())
            .unwrap();
        sign_in(&e, oauth("x", "y"), account("u-new"));

        let err = switch(
            &e.store,
            &e.live,
            &OFFLINE,
            "perso",
            false,
            &|_| unreachable!(),
        )
        .unwrap_err();

        assert!(err.to_string().contains("not in the store"), "{err}");
        assert_eq!(
            read_json(&e.live.creds).unwrap().unwrap()[OAUTH_KEY]["refreshToken"],
            "y"
        );
    }

    #[test]
    fn an_expired_target_is_refreshed_before_it_goes_live() {
        let e = env();
        let mut expired = oauth("b1", "s1");
        expired["expiresAt"] = json!(now_ms() - 1);
        e.store
            .save(&Entry::new("perso", expired, account("u-perso"), 1).unwrap())
            .unwrap();

        switch(&e.store, &e.live, &OFFLINE, "perso", false, &|entry| {
            entry
                .oauth_mut()
                .unwrap()
                .insert("accessToken".into(), json!("fresh"));
            Ok(())
        })
        .unwrap();

        assert_eq!(
            read_json(&e.live.creds).unwrap().unwrap()[OAUTH_KEY]["accessToken"],
            "fresh"
        );
        assert_eq!(
            e.store.get("perso").unwrap().unwrap().token("accessToken"),
            Some("fresh")
        );
    }

    #[test]
    fn a_live_login_that_could_not_be_confirmed_is_not_switched_away_from() {
        let e = env();
        e.store
            .save(&Entry::new("work", oauth("a1", "r1"), account("u-work"), 1).unwrap())
            .unwrap();
        e.store
            .save(&Entry::new("perso", oauth("b1", "s1"), account("u-perso"), 1).unwrap())
            .unwrap();
        sign_in(&e, oauth("a2", "r2"), account("u-work"));

        let err = switch(
            &e.store,
            &e.live,
            &OFFLINE,
            "perso",
            false,
            &|_| unreachable!(),
        )
        .unwrap_err();
        assert!(err.to_string().contains("could not be confirmed"), "{err}");
        assert_eq!(
            read_json(&e.live.creds).unwrap().unwrap()[OAUTH_KEY]["refreshToken"],
            "r2"
        );

        switch(
            &e.store,
            &e.live,
            &OFFLINE,
            "perso",
            true,
            &|_| unreachable!(),
        )
        .unwrap();
        assert_eq!(
            read_json(&e.live.creds).unwrap().unwrap()[OAUTH_KEY]["refreshToken"],
            "s1"
        );
    }

    #[test]
    fn a_changed_profile_travels_back_with_the_tokens() {
        let e = env();
        e.store
            .save(&Entry::new("work", oauth("a1", "r1"), account("u-work"), 1).unwrap())
            .unwrap();
        let mut renamed = account("u-work");
        renamed["displayName"] = json!("New name");
        sign_in(&e, oauth("a1", "r1"), renamed);

        sync_live(&e.store, &e.live, &OFFLINE).unwrap();

        let stored = e.store.get("work").unwrap().unwrap();
        assert_eq!(stored.meta.oauth_account["displayName"], "New name");
    }
}
