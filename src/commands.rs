use std::io::Write;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use crate::claude::{self, Api};
use crate::fsx::now_ms;
use crate::live::Live;
use crate::ops::{self, LiveState};
use crate::store::{Entry, Store, validate_name};

pub fn refresh_entry(api: &Api, entry: &mut Entry) -> Result<()> {
    let expected = entry.meta.account_uuid.clone();
    let oauth = entry
        .oauth_mut()
        .context("stored credential has no login")?;
    if let Some(uuid) = claude::refresh(api, oauth)?
        && uuid != expected
    {
        bail!("the token endpoint answered for account {uuid}, not {expected}");
    }
    Ok(())
}

fn human(ms: i64) -> String {
    let secs = ms.abs() / 1000;
    let span = match secs {
        s if s < 60 => format!("{s}s"),
        s if s < 3600 => format!("{}m", s / 60),
        s if s < 86_400 => format!("{}h", s / 3600),
        s => format!("{}d", s / 86_400),
    };
    if ms < 0 {
        format!("expired {span} ago")
    } else {
        format!("in {span}")
    }
}

pub fn list(store: &Store, state: &LiveState, as_json: bool, out: &mut dyn Write) -> Result<()> {
    let entries = store.list()?;
    let active = state.active_name();
    let now = now_ms();
    if as_json {
        let rows: Vec<Value> = entries
            .iter()
            .map(|e| {
                json!({
                    "name": e.name,
                    "email": e.meta.email,
                    "accountUuid": e.meta.account_uuid,
                    "plan": e.plan(),
                    "active": Some(e.name.as_str()) == active,
                    "expiresAt": e.millis("expiresAt"),
                    "refreshTokenExpiresAt": e.millis("refreshTokenExpiresAt"),
                })
            })
            .collect();
        writeln!(
            out,
            "{}",
            serde_json::to_string_pretty(&json!({"store": store.dir(), "credentials": rows}))?
        )?;
        return Ok(());
    }
    if entries.is_empty() {
        writeln!(out, "no stored credentials in {}", store.dir().display())?;
    }
    let w = entries.iter().map(|e| e.name.len()).max().unwrap_or(0);
    let we = entries
        .iter()
        .map(|e| e.meta.email.len())
        .max()
        .unwrap_or(0);
    for e in &entries {
        let mark = if Some(e.name.as_str()) == active {
            "*"
        } else {
            " "
        };
        let access = e
            .millis("expiresAt")
            .map(|t| human(t - now))
            .unwrap_or_else(|| "-".into());
        let refresh = e
            .millis("refreshTokenExpiresAt")
            .map(|t| format!(", refresh {}", human(t - now)))
            .unwrap_or_default();
        writeln!(
            out,
            "{mark} {:w$}  {:we$}  {:8}  token {access}{refresh}",
            e.name,
            e.meta.email,
            e.plan()
        )?;
    }
    match state {
        LiveState::SignedOut => writeln!(out, "\nClaude Code is signed out")?,
        LiveState::Unstored { email } => writeln!(
            out,
            "\nClaude Code is signed into {}, which is not stored: `remuda import <name>`",
            email.as_deref().unwrap_or("an unknown account")
        )?,
        LiveState::Stored {
            synced: false,
            name,
        } => writeln!(
            out,
            "\nClaude Code looks signed into {name}, but it could not be confirmed"
        )?,
        LiveState::Stored { .. } => {}
    }
    Ok(())
}

pub fn import(
    store: &Store,
    live: &Live,
    state: &LiveState,
    name: &str,
    force: bool,
    out: &mut dyn Write,
) -> Result<()> {
    validate_name(name)?;
    if let Some(current) = state.active_name() {
        bail!("the live login is already stored as {current}");
    }
    let oauth = live.oauth()?.context("Claude Code is not signed in")?;
    let account = live
        .account()?
        .context("Claude Code's config has no oauthAccount")?;
    if !force && store.get(name)?.is_some() {
        bail!("{name} already exists, pass --force to replace it");
    }
    let uuid = account
        .get("accountUuid")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if let Some(other) = store
        .list()?
        .into_iter()
        .find(|e| e.meta.account_uuid == uuid && e.name != name)
    {
        bail!("this account is already stored as {}", other.name);
    }
    let entry = Entry::new(name, Value::Object(oauth), account, now_ms())?;
    store.save(&entry)?;
    writeln!(out, "stored {name} ({})", entry.meta.email)?;
    Ok(())
}

pub struct Prompt<'a> {
    pub open: &'a dyn Fn(&str),
    pub read_code: &'a mut dyn FnMut() -> Result<String>,
}

pub fn login(
    store: &Store,
    api: &Api,
    name: &str,
    force: bool,
    prompt: Prompt,
    out: &mut dyn Write,
) -> Result<()> {
    validate_name(name)?;
    if !force && store.get(name)?.is_some() {
        bail!("{name} already exists, pass --force to replace it");
    }
    let pkce = claude::start_login()?;
    writeln!(
        out,
        "Sign in with the account to store as {name}. A private window avoids the account you are signed into.\n\n{}\n",
        pkce.url
    )?;
    (prompt.open)(&pkce.url);
    write!(out, "Paste the code the page shows: ")?;
    out.flush()?;
    let pasted = (prompt.read_code)()?;
    let result = claude::finish_login(api, &pkce, &pasted)?;
    let entry = Entry::new(name, result.oauth, result.oauth_account, now_ms())?;
    if let Some(other) = store
        .list()?
        .into_iter()
        .find(|e| e.meta.account_uuid == entry.meta.account_uuid && e.name != name)
    {
        bail!(
            "{} is already stored as {}; remove it first to store it again",
            entry.meta.email,
            other.name
        );
    }
    store.save(&entry)?;
    writeln!(
        out,
        "stored {name} ({}, {})",
        entry.meta.email,
        entry.plan()
    )?;
    Ok(())
}

pub struct RefreshScope<'a> {
    pub only: Option<&'a str>,
    pub force: bool,
    pub within_min: i64,
}

pub fn refresh(
    store: &Store,
    api: &Api,
    state: &LiveState,
    scope: RefreshScope,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> Result<()> {
    let active = state.active_name();
    let mut failed = 0;
    for mut entry in store.list()? {
        if scope.only.is_some_and(|n| n != entry.name) {
            continue;
        }
        if Some(entry.name.as_str()) == active {
            if scope.only.is_some() {
                writeln!(out, "{}: active, Claude Code refreshes it", entry.name)?;
            }
            continue;
        }
        let due = ops::expires_in_ms(&entry).is_none_or(|ms| ms < scope.within_min * 60_000);
        if !scope.force && !due {
            continue;
        }
        match refresh_entry(api, &mut entry).and_then(|()| store.save(&entry)) {
            Ok(()) => writeln!(out, "{}: refreshed", entry.name)?,
            Err(e) => {
                failed += 1;
                writeln!(err, "{}: {e:#}", entry.name)?;
            }
        }
    }
    if failed > 0 {
        bail!("{failed} credential(s) failed to refresh");
    }
    Ok(())
}

pub fn remove(store: &Store, state: &LiveState, name: &str, out: &mut dyn Write) -> Result<()> {
    if state.active_name() == Some(name) {
        bail!("{name} is active; switch to another credential first");
    }
    store.remove(name)?;
    writeln!(out, "removed {name}")?;
    Ok(())
}
