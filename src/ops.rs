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
