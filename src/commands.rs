use std::io::Write;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use crate::fsx::now_ms;
use crate::ops::{self, LiveState};
use crate::provider::Provider;
use crate::store::{Entry, Store, validate_name};

pub fn find<'a>(providers: &[&'a dyn Provider], id: &str) -> Result<&'a dyn Provider> {
    providers
        .iter()
        .find(|p| p.id() == id)
        .copied()
        .with_context(|| {
            let ids: Vec<&str> = providers.iter().map(|p| p.id()).collect();
            format!("no CLI called {id}: {}", ids.join(", "))
        })
}

/// `provider/name`, or a bare name stored for exactly one CLI.
pub fn resolve<'a>(
    store: &Store,
    providers: &[&'a dyn Provider],
    spec: &str,
) -> Result<(&'a dyn Provider, String)> {
    if let Some((id, name)) = spec.split_once('/') {
        validate_name(name)?;
        return Ok((find(providers, id)?, name.to_owned()));
    }
    validate_name(spec)?;
    let mut hits = Vec::new();
    for p in providers {
        if store.get(p.id(), spec)?.is_some() {
            hits.push(*p);
        }
    }
    match hits.as_slice() {
        [p] => Ok((*p, spec.to_owned())),
        [] => bail!("no credential named {spec}"),
        many => {
            let named: Vec<String> = many.iter().map(|p| format!("{}/{spec}", p.id())).collect();
            bail!(
                "{spec} is stored for several CLIs, name one: {}",
                named.join(", ")
            )
        }
    }
}

pub struct Live<'a> {
    pub provider: &'a dyn Provider,
    pub state: LiveState,
}

pub fn sync_all<'a>(store: &Store, providers: &[&'a dyn Provider]) -> Result<Vec<Live<'a>>> {
    providers
        .iter()
        .map(|p| {
            Ok(Live {
                provider: *p,
                state: ops::sync_live(store, *p)?,
            })
        })
        .collect()
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

fn import_hint(p: &dyn Provider) -> String {
    if p.id() == "claude" {
        "`remuda import <name>`".to_owned()
    } else {
        format!("`remuda import -p {} <name>`", p.id())
    }
}

fn state_json(live: &Live) -> Value {
    let id = live.provider.id();
    match &live.state {
        LiveState::SignedOut => json!({"provider": id, "state": "signed_out"}),
        LiveState::Stored { name, synced } => {
            json!({"provider": id, "state": "stored", "name": name, "confirmed": synced})
        }
        LiveState::Unstored { email } => {
            json!({"provider": id, "state": "unstored", "email": email})
        }
    }
}

pub fn list_json(store: &Store, lives: &[Live]) -> Result<Value> {
    let mut rows = Vec::new();
    for live in lives {
        let p = live.provider;
        for e in store.list(p.id())? {
            rows.push(json!({
                "provider": p.id(),
                "name": e.name,
                "label": e.meta.label,
                "email": e.meta.email,
                "accountId": e.meta.account_id,
                "plan": p.plan(&e.creds),
                "active": live.state.active_name() == Some(e.name.as_str()),
                "expiresAt": p.expires_at(&e.creds),
                "refreshTokenExpiresAt": p.refresh_expires_at(&e.creds),
            }));
        }
    }
    let live: Vec<Value> = lives.iter().map(state_json).collect();
    Ok(json!({"store": store.root(), "credentials": rows, "live": live}))
}

pub fn list(store: &Store, lives: &[Live], as_json: bool, out: &mut dyn Write) -> Result<()> {
    if as_json {
        writeln!(
            out,
            "{}",
            serde_json::to_string_pretty(&list_json(store, lives)?)?
        )?;
        return Ok(());
    }
    let now = now_ms();
    let mut printed = false;
    for live in lives {
        let p = live.provider;
        let entries = store.list(p.id())?;
        if entries.is_empty() && live.state == LiveState::SignedOut {
            continue;
        }
        if printed {
            writeln!(out)?;
        }
        printed = true;
        writeln!(out, "{}", p.name())?;
        let w = entries.iter().map(|e| e.name.len()).max().unwrap_or(0);
        let we = entries
            .iter()
            .map(|e| e.meta.email.len())
            .max()
            .unwrap_or(0);
        for e in &entries {
            let mark = if live.state.active_name() == Some(e.name.as_str()) {
                "*"
            } else {
                " "
            };
            let access = p
                .expires_at(&e.creds)
                .map(|t| human(t - now))
                .unwrap_or_else(|| "-".into());
            let refresh = p
                .refresh_expires_at(&e.creds)
                .map(|t| format!(", refresh {}", human(t - now)))
                .unwrap_or_default();
            writeln!(
                out,
                "{mark} {:w$}  {:we$}  {:8}  token {access}{refresh}",
                e.name,
                e.meta.email,
                p.plan(&e.creds)
            )?;
        }
        match &live.state {
            LiveState::SignedOut => writeln!(out, "  signed out")?,
            LiveState::Unstored { email } => writeln!(
                out,
                "  signed into {}, which is not stored: {}",
                email.as_deref().unwrap_or("an unknown account"),
                import_hint(p)
            )?,
            LiveState::Stored {
                synced: false,
                name,
            } => writeln!(
                out,
                "  looks signed into {name}, but it could not be confirmed"
            )?,
            LiveState::Stored { .. } => {}
        }
    }
    if !printed {
        writeln!(out, "no stored credentials in {}", store.root().display())?;
    }
    Ok(())
}

pub fn import(
    store: &Store,
    p: &dyn Provider,
    state: &LiveState,
    name: &str,
    force: bool,
    out: &mut dyn Write,
) -> Result<()> {
    validate_name(name)?;
    if let Some(current) = state.active_name() {
        bail!("the live {} login is already stored as {current}", p.name());
    }
    let creds = p
        .live()?
        .with_context(|| format!("{} is not signed in", p.name()))?;
    let identity = p
        .live_identity(&creds)?
        .with_context(|| format!("{} does not say whose login this is", p.name()))?;
    if !force && store.get(p.id(), name)?.is_some() {
        bail!(
            "{}/{name} already exists, pass --force to replace it",
            p.id()
        );
    }
    if let Some(other) = store
        .list(p.id())?
        .into_iter()
        .find(|e| e.meta.account_id == identity.account_id && e.name != name)
    {
        bail!("this account is already stored as {}", other.qualified());
    }
    let entry = Entry::new(p.id(), name, creds, identity, now_ms());
    store.save(&entry)?;
    writeln!(out, "stored {} ({})", entry.qualified(), entry.meta.email)?;
    Ok(())
}

pub struct Prompt<'a> {
    pub open: &'a dyn Fn(&str),
    pub read_code: &'a mut dyn FnMut() -> Result<String>,
}

pub fn login(
    store: &Store,
    p: &dyn Provider,
    name: &str,
    force: bool,
    prompt: Prompt,
    out: &mut dyn Write,
) -> Result<()> {
    validate_name(name)?;
    if !force && store.get(p.id(), name)?.is_some() {
        bail!(
            "{}/{name} already exists, pass --force to replace it",
            p.id()
        );
    }
    let pending = p.begin_login()?;
    writeln!(
        out,
        "Sign in to {} with the account to store as {name}. A private window avoids the account you are signed into.\n\n{}\n",
        p.name(),
        pending.url()
    )?;
    (prompt.open)(pending.url());
    let code = if pending.needs_code() {
        write!(out, "Paste the code the page shows: ")?;
        out.flush()?;
        Some((prompt.read_code)()?)
    } else {
        writeln!(out, "Waiting for the browser to finish...")?;
        None
    };
    let done = pending.finish(code.as_deref())?;
    let entry = save_login(store, p, name, force, done)?;
    writeln!(
        out,
        "stored {} ({}, {})",
        entry.qualified(),
        entry.meta.email,
        p.plan(&entry.creds)
    )?;
    Ok(())
}

/// Takes the store lock itself: a login has spent minutes in the browser by
/// the time it gets here, and nothing may hold the lock across that.
pub fn save_login(
    store: &Store,
    p: &dyn Provider,
    name: &str,
    force: bool,
    done: crate::provider::Login,
) -> Result<Entry> {
    let entry = Entry::new(p.id(), name, done.creds, done.identity, now_ms());
    let _lock = store.lock()?;
    if !force && store.get(p.id(), name)?.is_some() {
        bail!(
            "{} was stored while this login was in progress",
            entry.qualified()
        );
    }
    if let Some(other) = store
        .list(p.id())?
        .into_iter()
        .find(|e| e.meta.account_id == entry.meta.account_id && e.name != name)
    {
        bail!(
            "{} is already stored as {}; remove it first to store it again",
            entry.meta.email,
            other.qualified()
        );
    }
    store.save(&entry)?;
    Ok(entry)
}

pub struct RefreshScope<'a> {
    pub only: Option<(&'a dyn Provider, &'a str)>,
    pub force: bool,
    pub within_min: i64,
}

pub fn refresh(
    store: &Store,
    lives: &[Live],
    scope: RefreshScope,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> Result<()> {
    let mut failed = 0;
    for live in lives {
        let p = live.provider;
        if scope.only.is_some_and(|(only, _)| only.id() != p.id()) {
            continue;
        }
        for mut entry in store.list(p.id())? {
            if scope.only.is_some_and(|(_, name)| name != entry.name) {
                continue;
            }
            if live.state.active_name() == Some(entry.name.as_str()) {
                if scope.only.is_some() {
                    writeln!(
                        out,
                        "{}: active, {} refreshes it",
                        entry.qualified(),
                        p.name()
                    )?;
                }
                continue;
            }
            let due = ops::expires_in_ms(p, &entry).is_none_or(|ms| ms < scope.within_min * 60_000);
            if !scope.force && !due {
                continue;
            }
            match ops::refresh_entry(p, &mut entry).and_then(|()| store.save(&entry)) {
                Ok(()) => writeln!(out, "{}: refreshed", entry.qualified())?,
                Err(e) => {
                    failed += 1;
                    writeln!(err, "{}: {e:#}", entry.qualified())?;
                }
            }
        }
    }
    if failed > 0 {
        bail!("{failed} credential(s) failed to refresh");
    }
    Ok(())
}

pub fn remove(
    store: &Store,
    p: &dyn Provider,
    state: &LiveState,
    name: &str,
    out: &mut dyn Write,
) -> Result<()> {
    if state.active_name() == Some(name) {
        bail!(
            "{}/{name} is active; switch to another credential first",
            p.id()
        );
    }
    store.remove(p.id(), name)?;
    writeln!(out, "removed {}/{name}", p.id())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::claude::OAUTH_KEY;
    use crate::fsx::write_json;
    use crate::ops::testing::*;
    use mockito::Matcher;

    fn text(buf: Vec<u8>) -> String {
        String::from_utf8(buf).unwrap()
    }

    fn live<'a>(e: &'a Env, state: LiveState) -> Vec<Live<'a>> {
        vec![Live {
            provider: &e.claude,
            state,
        }]
    }

    fn active(name: &str) -> LiveState {
        LiveState::Stored {
            name: name.into(),
            synced: true,
        }
    }

    #[test]
    fn durations_read_in_the_largest_whole_unit() {
        assert_eq!(human(30_000), "in 30s");
        assert_eq!(human(5 * 60_000), "in 5m");
        assert_eq!(human(3 * HOUR), "in 3h");
        assert_eq!(human(49 * HOUR), "in 2d");
        assert_eq!(human(-2 * HOUR), "expired 2h ago");
    }

    #[test]
    fn the_list_marks_the_active_credential_and_its_expiries() {
        let e = env(OFFLINE);
        e.stored("perso", "u-perso", 2 * HOUR + 60_000);
        e.stored("work", "u-work", -HOUR);
        let mut out = Vec::new();
        list(&e.store, &live(&e, active("work")), false, &mut out).unwrap();
        let out = text(out);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines[0], "Claude Code", "{out}");
        assert!(lines[1].starts_with("  perso"), "{out}");
        assert!(lines[1].contains("max 5x"), "{out}");
        assert!(lines[1].contains("token in 2h, refresh in 20d"), "{out}");
        assert!(lines[2].starts_with("* work"), "{out}");
        assert!(lines[2].contains("token expired 1h ago"), "{out}");
        assert_eq!(lines.len(), 3, "{out}");
    }

    #[test]
    fn the_list_says_when_the_live_login_is_not_accounted_for() {
        let e = env(OFFLINE);
        let mut out = Vec::new();
        list(&e.store, &live(&e, LiveState::SignedOut), false, &mut out).unwrap();
        assert!(text(out).starts_with("no stored credentials in "));

        for (state, says) in [
            (
                LiveState::Unstored {
                    email: Some("x@example.com".into()),
                },
                "signed into x@example.com, which is not stored: `remuda import <name>`",
            ),
            (LiveState::Unstored { email: None }, "an unknown account"),
            (
                LiveState::Stored {
                    name: "work".into(),
                    synced: false,
                },
                "could not be confirmed",
            ),
        ] {
            let mut out = Vec::new();
            list(&e.store, &live(&e, state), false, &mut out).unwrap();
            let out = text(out);
            assert!(out.starts_with("Claude Code\n"), "{out}");
            assert!(out.contains(says), "{out}");
        }

        e.stored("work", "u-work", HOUR);
        let mut out = Vec::new();
        list(&e.store, &live(&e, LiveState::SignedOut), false, &mut out).unwrap();
        assert!(text(out).ends_with("  signed out\n"));
    }

    #[test]
    fn the_json_list_is_what_other_tools_read() {
        let e = env(OFFLINE);
        e.stored("work", "u-work", HOUR);
        let mut out = Vec::new();
        list(&e.store, &live(&e, active("work")), true, &mut out).unwrap();
        let v: Value = serde_json::from_slice(&out).unwrap();
        let row = &v["credentials"][0];
        assert_eq!(row["provider"], "claude");
        assert_eq!(row["name"], "work");
        assert_eq!(row["accountId"], "u-work");
        assert_eq!(row["plan"], "max 5x");
        assert_eq!(row["active"], true);
        assert!(row["expiresAt"].is_i64());
        assert_eq!(v["live"][0]["state"], "stored");
        assert!(v["store"].as_str().unwrap().ends_with("store"));
    }

    #[test]
    fn import_stores_the_live_login_once() {
        let e = env(OFFLINE);
        e.sign_in(oauth("a1", "r1", HOUR), account("u-work"));
        let mut out = Vec::new();
        import(
            &e.store,
            &e.claude,
            &LiveState::Unstored { email: None },
            "work",
            false,
            &mut out,
        )
        .unwrap();
        assert_eq!(text(out), "stored claude/work (u-work@example.com)\n");
        assert_eq!(e.stored_refresh_token("work").as_deref(), Some("r1"));
        let stored = e.store.get("claude", "work").unwrap().unwrap();
        assert!(stored.creds.get("mcpOAuth").is_none());

        let err = import(
            &e.store,
            &e.claude,
            &active("work"),
            "again",
            false,
            &mut Vec::new(),
        )
        .unwrap_err();
        assert!(err.to_string().contains("already stored as work"), "{err}");
    }

    #[test]
    fn import_refuses_a_taken_name_or_a_second_copy_of_an_account() {
        let e = env(OFFLINE);
        e.stored("work", "u-work", HOUR);
        e.stored("perso", "u-perso", HOUR);
        e.sign_in(oauth("new", "new", HOUR), account("u-perso"));
        let unstored = LiveState::Unstored { email: None };
        let run = |name: &str, force: bool| {
            import(&e.store, &e.claude, &unstored, name, force, &mut Vec::new())
                .unwrap_err()
                .to_string()
        };

        assert!(run("work", false).contains("--force"));
        assert!(run("work", true).contains("already stored as claude/perso"));
        assert!(run("../x", false).contains("not a usable name"));
    }

    #[test]
    fn import_needs_a_signed_in_cli_that_says_who_it_is() {
        let e = env(OFFLINE);
        let run = |state: LiveState| {
            import(&e.store, &e.claude, &state, "work", false, &mut Vec::new())
                .unwrap_err()
                .to_string()
        };
        assert!(run(LiveState::SignedOut).contains("not signed in"));

        write_json(
            &e.tmp.path().join(".credentials.json"),
            &oauth("a", "r", HOUR),
        )
        .unwrap();
        assert!(run(LiveState::Unstored { email: None }).contains("whose login"));
    }

    #[test]
    fn refresh_skips_the_active_and_the_fresh_and_renews_the_rest() {
        let mut server = mockito::Server::new();
        let token = server
            .mock("POST", "/v1/oauth/token")
            .match_body(Matcher::PartialJson(json!({
                "grant_type": "refresh_token",
                "refresh_token": "r-stale",
                "scope": "user:inference user:profile",
            })))
            .with_body(
                json!({
                    "access_token": "a-new", "refresh_token": "r-new", "expires_in": 28800,
                    "account": {"uuid": "u-stale"}
                })
                .to_string(),
            )
            .expect(1)
            .create();
        let e = env(&server.url());
        e.stored("active", "u-active", -HOUR);
        e.stored("fresh", "u-fresh", 5 * HOUR);
        e.stored("stale", "u-stale", 10 * 60_000);
        let mut out = Vec::new();
        refresh(
            &e.store,
            &live(&e, active("active")),
            RefreshScope {
                only: None,
                force: false,
                within_min: 60,
            },
            &mut out,
            &mut Vec::new(),
        )
        .unwrap();

        token.assert();
        assert_eq!(text(out), "claude/stale: refreshed\n");
        assert_eq!(e.stored_refresh_token("stale").as_deref(), Some("r-new"));
        let stale = e.store.get("claude", "stale").unwrap().unwrap();
        assert!(ops::expires_in_ms(&e.claude, &stale).unwrap() > 7 * HOUR);
        assert_eq!(
            e.stored_refresh_token("active").as_deref(),
            Some("r-active")
        );
    }

    #[test]
    fn refresh_by_name_says_why_it_leaves_the_active_one_alone() {
        let e = env(OFFLINE);
        e.stored("active", "u-active", -HOUR);
        e.stored("other", "u-other", -HOUR);
        let mut out = Vec::new();
        refresh(
            &e.store,
            &live(&e, active("active")),
            RefreshScope {
                only: Some((&e.claude, "active")),
                force: true,
                within_min: 60,
            },
            &mut out,
            &mut Vec::new(),
        )
        .unwrap();
        assert_eq!(
            text(out),
            "claude/active: active, Claude Code refreshes it\n"
        );
    }

    #[test]
    fn a_rejected_refresh_is_reported_and_the_run_fails() {
        let mut server = mockito::Server::new();
        server
            .mock("POST", "/v1/oauth/token")
            .with_status(400)
            .with_body(r#"{"error":"invalid_grant"}"#)
            .create();
        let e = env(&server.url());
        e.stored("dead", "u-dead", HOUR);
        let mut err = Vec::new();
        let result = refresh(
            &e.store,
            &live(&e, LiveState::SignedOut),
            RefreshScope {
                only: Some((&e.claude, "dead")),
                force: true,
                within_min: 60,
            },
            &mut Vec::new(),
            &mut err,
        );
        assert!(result.unwrap_err().to_string().contains("1 credential(s)"));
        assert!(text(err).contains("claude/dead: the refresh token was rejected"));
        assert_eq!(e.stored_refresh_token("dead").as_deref(), Some("r-dead"));
    }

    fn login_server(uuid: &str) -> mockito::ServerGuard {
        let mut server = mockito::Server::new();
        server
            .mock("POST", "/v1/oauth/token")
            .match_body(Matcher::PartialJson(json!({
                "grant_type": "authorization_code", "code": "the-code"
            })))
            .with_body(
                json!({
                    "access_token": "a-login", "refresh_token": "r-login", "expires_in": 28800,
                    "refresh_token_expires_in": 2_592_000, "scope": "user:inference user:profile"
                })
                .to_string(),
            )
            .create();
        server
            .mock("GET", "/api/oauth/profile")
            .match_header("authorization", "Bearer a-login")
            .with_body(profile_body(uuid))
            .create();
        server
    }

    fn state_of(url: &str) -> String {
        let url = reqwest::Url::parse(url).unwrap();
        url.query_pairs()
            .find(|(k, _)| k == "state")
            .unwrap()
            .1
            .into_owned()
    }

    #[test]
    fn login_exchanges_the_pasted_code_and_stores_the_account() {
        let server = login_server("u-new");
        let e = env(&server.url());
        let opened = std::cell::RefCell::new(String::new());
        let open = |url: &str| *opened.borrow_mut() = url.to_owned();
        let mut read_code = || Ok(format!("the-code#{}\n", state_of(&opened.borrow())));
        let mut out = Vec::new();

        login(
            &e.store,
            &e.claude,
            "new",
            false,
            Prompt {
                open: &open,
                read_code: &mut read_code,
            },
            &mut out,
        )
        .unwrap();

        let out = text(out);
        assert!(
            out.contains("https://claude.com/cai/oauth/authorize?"),
            "{out}"
        );
        assert!(
            out.ends_with("stored claude/new (u-new@example.com, pro)\n"),
            "{out}"
        );
        let entry = e.store.get("claude", "new").unwrap().unwrap();
        assert_eq!(e.stored_refresh_token("new").as_deref(), Some("r-login"));
        assert_eq!(
            entry.meta.oauth_account.as_ref().unwrap()["fullName"],
            "Someone"
        );
        assert_eq!(
            entry.creds[OAUTH_KEY]["rateLimitTier"],
            "default_claude_pro"
        );
    }

    #[test]
    fn login_refuses_a_code_from_another_attempt() {
        let server = login_server("u-new");
        let e = env(&server.url());
        let mut read_code = || Ok("the-code#forged".to_owned());
        let err = login(
            &e.store,
            &e.claude,
            "new",
            false,
            Prompt {
                open: &|_| {},
                read_code: &mut read_code,
            },
            &mut Vec::new(),
        )
        .unwrap_err();
        assert!(err.to_string().contains("another login attempt"), "{err}");
        assert!(e.store.get("claude", "new").unwrap().is_none());
    }

    #[test]
    fn login_will_not_store_an_account_twice() {
        let server = login_server("u-work");
        let e = env(&server.url());
        e.stored("work", "u-work", HOUR);
        let opened = std::cell::RefCell::new(String::new());
        let open = |url: &str| *opened.borrow_mut() = url.to_owned();
        let mut read_code = || Ok(format!("the-code#{}", state_of(&opened.borrow())));

        let err = login(
            &e.store,
            &e.claude,
            "again",
            false,
            Prompt {
                open: &open,
                read_code: &mut read_code,
            },
            &mut Vec::new(),
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("already stored as claude/work"),
            "{err}"
        );

        let err = login(
            &e.store,
            &e.claude,
            "work",
            false,
            Prompt {
                open: &|_| unreachable!(),
                read_code: &mut || unreachable!(),
            },
            &mut Vec::new(),
        )
        .unwrap_err();
        assert!(err.to_string().contains("--force"), "{err}");
    }

    #[test]
    fn remove_keeps_the_active_credential() {
        let e = env(OFFLINE);
        e.stored("work", "u-work", HOUR);
        let err = remove(
            &e.store,
            &e.claude,
            &active("work"),
            "work",
            &mut Vec::new(),
        )
        .unwrap_err();
        assert!(err.to_string().contains("is active"), "{err}");

        let mut out = Vec::new();
        remove(&e.store, &e.claude, &LiveState::SignedOut, "work", &mut out).unwrap();
        assert_eq!(text(out), "removed claude/work\n");
        assert!(e.store.get("claude", "work").unwrap().is_none());
    }

    #[test]
    fn a_name_resolves_bare_or_qualified() {
        let e = env(OFFLINE);
        e.stored("work", "u-work", HOUR);
        let providers: [&dyn Provider; 1] = [&e.claude];
        let (p, name) = resolve(&e.store, &providers, "work").unwrap();
        assert_eq!((p.id(), name.as_str()), ("claude", "work"));
        let (p, name) = resolve(&e.store, &providers, "claude/other").unwrap();
        assert_eq!((p.id(), name.as_str()), ("claude", "other"));
        let err = resolve(&e.store, &providers, "nope").err().unwrap();
        assert!(err.to_string().contains("no credential named nope"));
        let err = resolve(&e.store, &providers, "gemini/work").err().unwrap();
        assert!(err.to_string().contains("no CLI called gemini"), "{err}");
    }
}
