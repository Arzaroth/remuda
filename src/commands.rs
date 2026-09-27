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
    let _lock = store.lock()?;
    if !force && store.get(name)?.is_some() {
        bail!("{name} was stored while this login was in progress");
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::claude::OAUTH_KEY;
    use crate::fsx::write_json;
    use mockito::Matcher;

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

    fn oauth(access: &str, refresh: &str, expires_in_ms: i64) -> Value {
        json!({
            "accessToken": access,
            "refreshToken": refresh,
            "expiresAt": now_ms() + expires_in_ms,
            "refreshTokenExpiresAt": now_ms() + 20 * 86_400_000 + HOUR,
            "scopes": ["user:inference", "user:profile"],
            "subscriptionType": "max",
            "rateLimitTier": "default_claude_max_5x",
        })
    }

    fn account(uuid: &str) -> Value {
        json!({"accountUuid": uuid, "emailAddress": format!("{uuid}@example.com")})
    }

    fn stored(e: &Env, name: &str, uuid: &str, expires_in_ms: i64) {
        let entry = Entry::new(
            name,
            oauth(&format!("a-{name}"), &format!("r-{name}"), expires_in_ms),
            account(uuid),
            1,
        )
        .unwrap();
        e.store.save(&entry).unwrap();
    }

    fn sign_in(e: &Env, oauth: Value, account: Value) {
        write_json(&e.live.creds, &json!({ OAUTH_KEY: oauth })).unwrap();
        write_json(&e.live.config, &json!({ "oauthAccount": account })).unwrap();
    }

    fn text(buf: Vec<u8>) -> String {
        String::from_utf8(buf).unwrap()
    }

    const HOUR: i64 = 3_600_000;

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
        let e = env();
        stored(&e, "perso", "u-perso", 2 * HOUR + 60_000);
        stored(&e, "work", "u-work", -HOUR);
        let state = LiveState::Stored {
            name: "work".into(),
            synced: true,
        };
        let mut out = Vec::new();
        list(&e.store, &state, false, &mut out).unwrap();
        let out = text(out);
        let lines: Vec<&str> = out.lines().collect();
        assert!(lines[0].starts_with("  perso"), "{out}");
        assert!(lines[0].contains("max 5x"), "{out}");
        assert!(lines[0].contains("token in 2h, refresh in 20d"), "{out}");
        assert!(lines[1].starts_with("* work"), "{out}");
        assert!(lines[1].contains("token expired 1h ago"), "{out}");
        assert_eq!(lines.len(), 2, "{out}");
    }

    #[test]
    fn the_list_says_when_the_live_login_is_not_accounted_for() {
        let e = env();
        for (state, says) in [
            (LiveState::SignedOut, "signed out"),
            (
                LiveState::Unstored {
                    email: Some("x@example.com".into()),
                },
                "x@example.com, which is not stored",
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
            list(&e.store, &state, false, &mut out).unwrap();
            let out = text(out);
            assert!(out.contains("no stored credentials"), "{out}");
            assert!(out.contains(says), "{out}");
        }
    }

    #[test]
    fn the_json_list_is_what_other_tools_read() {
        let e = env();
        stored(&e, "work", "u-work", HOUR);
        let mut out = Vec::new();
        list(
            &e.store,
            &LiveState::Stored {
                name: "work".into(),
                synced: true,
            },
            true,
            &mut out,
        )
        .unwrap();
        let v: Value = serde_json::from_slice(&out).unwrap();
        let row = &v["credentials"][0];
        assert_eq!(row["name"], "work");
        assert_eq!(row["accountUuid"], "u-work");
        assert_eq!(row["active"], true);
        assert!(row["expiresAt"].is_i64());
        assert!(v["store"].as_str().unwrap().ends_with("store/claude"));
    }

    #[test]
    fn import_stores_the_live_login_once() {
        let e = env();
        sign_in(&e, oauth("a1", "r1", HOUR), account("u-work"));
        let mut out = Vec::new();
        import(
            &e.store,
            &e.live,
            &LiveState::Unstored { email: None },
            "work",
            false,
            &mut out,
        )
        .unwrap();
        assert_eq!(text(out), "stored work (u-work@example.com)\n");
        assert_eq!(
            e.store.get("work").unwrap().unwrap().token("refreshToken"),
            Some("r1")
        );

        let err = import(
            &e.store,
            &e.live,
            &LiveState::Stored {
                name: "work".into(),
                synced: true,
            },
            "again",
            false,
            &mut Vec::new(),
        )
        .unwrap_err();
        assert!(err.to_string().contains("already stored as work"), "{err}");
    }

    #[test]
    fn import_refuses_a_taken_name_or_a_second_copy_of_an_account() {
        let e = env();
        stored(&e, "work", "u-work", HOUR);
        stored(&e, "perso", "u-perso", HOUR);
        sign_in(&e, oauth("new", "new", HOUR), account("u-perso"));
        let unstored = LiveState::Unstored { email: None };

        let err = import(&e.store, &e.live, &unstored, "work", false, &mut Vec::new()).unwrap_err();
        assert!(err.to_string().contains("--force"), "{err}");

        let err = import(&e.store, &e.live, &unstored, "work", true, &mut Vec::new()).unwrap_err();
        assert!(err.to_string().contains("already stored as perso"), "{err}");

        let err = import(&e.store, &e.live, &unstored, "../x", false, &mut Vec::new()).unwrap_err();
        assert!(err.to_string().contains("not a usable name"), "{err}");
    }

    #[test]
    fn import_needs_a_signed_in_claude_code() {
        let e = env();
        let err = import(
            &e.store,
            &e.live,
            &LiveState::SignedOut,
            "work",
            false,
            &mut Vec::new(),
        )
        .unwrap_err();
        assert!(err.to_string().contains("not signed in"), "{err}");

        write_json(&e.live.creds, &json!({ OAUTH_KEY: oauth("a", "r", HOUR) })).unwrap();
        let err = import(
            &e.store,
            &e.live,
            &LiveState::Unstored { email: None },
            "work",
            false,
            &mut Vec::new(),
        )
        .unwrap_err();
        assert!(err.to_string().contains("no oauthAccount"), "{err}");
    }

    #[test]
    fn refresh_skips_the_active_and_the_fresh_and_renews_the_rest() {
        let e = env();
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
        stored(&e, "active", "u-active", -HOUR);
        stored(&e, "fresh", "u-fresh", 5 * HOUR);
        stored(&e, "stale", "u-stale", 10 * 60_000);
        let state = LiveState::Stored {
            name: "active".into(),
            synced: true,
        };
        let mut out = Vec::new();
        refresh(
            &e.store,
            &Api::local(&server.url()),
            &state,
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
        assert_eq!(text(out), "stale: refreshed\n");
        let stale = e.store.get("stale").unwrap().unwrap();
        assert_eq!(stale.token("refreshToken"), Some("r-new"));
        assert!(ops::expires_in_ms(&stale).unwrap() > 7 * HOUR);
        assert_eq!(
            e.store
                .get("active")
                .unwrap()
                .unwrap()
                .token("refreshToken"),
            Some("r-active")
        );
    }

    #[test]
    fn refresh_by_name_says_why_it_leaves_the_active_one_alone() {
        let e = env();
        stored(&e, "active", "u-active", -HOUR);
        stored(&e, "other", "u-other", -HOUR);
        let mut out = Vec::new();
        refresh(
            &e.store,
            &Api::local("http://127.0.0.1:9"),
            &LiveState::Stored {
                name: "active".into(),
                synced: true,
            },
            RefreshScope {
                only: Some("active"),
                force: true,
                within_min: 60,
            },
            &mut out,
            &mut Vec::new(),
        )
        .unwrap();
        assert_eq!(text(out), "active: active, Claude Code refreshes it\n");
    }

    #[test]
    fn a_rejected_refresh_is_reported_and_the_run_fails() {
        let e = env();
        let mut server = mockito::Server::new();
        server
            .mock("POST", "/v1/oauth/token")
            .with_status(400)
            .with_body(r#"{"error":"invalid_grant"}"#)
            .create();
        stored(&e, "dead", "u-dead", HOUR);
        let mut err = Vec::new();
        let result = refresh(
            &e.store,
            &Api::local(&server.url()),
            &LiveState::SignedOut,
            RefreshScope {
                only: Some("dead"),
                force: true,
                within_min: 60,
            },
            &mut Vec::new(),
            &mut err,
        );
        assert!(result.unwrap_err().to_string().contains("1 credential(s)"));
        assert!(text(err).contains("dead: the refresh token was rejected"));
        assert_eq!(
            e.store.get("dead").unwrap().unwrap().token("refreshToken"),
            Some("r-dead")
        );
    }

    #[test]
    fn a_refresh_that_answers_for_another_account_is_not_saved() {
        let e = env();
        let mut server = mockito::Server::new();
        server
            .mock("POST", "/v1/oauth/token")
            .with_body(
                json!({"access_token": "x", "expires_in": 60, "account": {"uuid": "u-someone"}})
                    .to_string(),
            )
            .create();
        stored(&e, "work", "u-work", HOUR);
        let mut entry = e.store.get("work").unwrap().unwrap();
        let err = refresh_entry(&Api::local(&server.url()), &mut entry).unwrap_err();
        assert!(err.to_string().contains("u-someone"), "{err}");
    }

    fn profile_body(uuid: &str) -> String {
        json!({
            "account": {"uuid": uuid, "email": format!("{uuid}@example.com"), "full_name": "Someone", "created_at": "2025-01-01"},
            "organization": {"uuid": "o-1", "organization_type": "claude_pro", "rate_limit_tier": "default_claude_pro"}
        })
        .to_string()
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
        let e = env();
        let server = login_server("u-new");
        let opened = std::cell::RefCell::new(String::new());
        let open = |url: &str| *opened.borrow_mut() = url.to_owned();
        let mut read_code = || Ok(format!("the-code#{}\n", state_of(&opened.borrow())));
        let mut out = Vec::new();

        login(
            &e.store,
            &Api::local(&server.url()),
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
            out.ends_with("stored new (u-new@example.com, pro)\n"),
            "{out}"
        );
        let entry = e.store.get("new").unwrap().unwrap();
        assert_eq!(entry.token("refreshToken"), Some("r-login"));
        assert_eq!(entry.meta.oauth_account["fullName"], "Someone");
        assert_eq!(
            entry.oauth().unwrap()["rateLimitTier"],
            "default_claude_pro"
        );
    }

    #[test]
    fn login_refuses_a_code_from_another_attempt() {
        let e = env();
        let server = login_server("u-new");
        let mut read_code = || Ok("the-code#forged".to_owned());
        let err = login(
            &e.store,
            &Api::local(&server.url()),
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
        assert!(e.store.get("new").unwrap().is_none());
    }

    #[test]
    fn login_will_not_store_an_account_twice() {
        let e = env();
        stored(&e, "work", "u-work", HOUR);
        let server = login_server("u-work");
        let opened = std::cell::RefCell::new(String::new());
        let open = |url: &str| *opened.borrow_mut() = url.to_owned();
        let mut read_code = || Ok(format!("the-code#{}", state_of(&opened.borrow())));

        let err = login(
            &e.store,
            &Api::local(&server.url()),
            "again",
            false,
            Prompt {
                open: &open,
                read_code: &mut read_code,
            },
            &mut Vec::new(),
        )
        .unwrap_err();
        assert!(err.to_string().contains("already stored as work"), "{err}");

        let err = login(
            &e.store,
            &Api::local(&server.url()),
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
        let e = env();
        stored(&e, "work", "u-work", HOUR);
        let state = LiveState::Stored {
            name: "work".into(),
            synced: true,
        };
        let err = remove(&e.store, &state, "work", &mut Vec::new()).unwrap_err();
        assert!(err.to_string().contains("is active"), "{err}");

        let mut out = Vec::new();
        remove(&e.store, &LiveState::SignedOut, "work", &mut out).unwrap();
        assert_eq!(text(out), "removed work\n");
        assert!(e.store.get("work").unwrap().is_none());
        assert!(remove(&e.store, &LiveState::SignedOut, "work", &mut Vec::new()).is_err());
    }

    #[test]
    fn the_profile_check_confirms_whose_token_it_is() {
        use crate::ops::WhoAmI;
        let mut server = mockito::Server::new();
        server
            .mock("GET", "/api/oauth/profile")
            .match_header("authorization", "Bearer good")
            .with_body(profile_body("u-work"))
            .create();
        server
            .mock("GET", "/api/oauth/profile")
            .match_header("authorization", "Bearer revoked")
            .with_status(401)
            .create();
        let api = Api::local(&server.url());
        assert_eq!(api.account_uuid("good").unwrap(), "u-work");
        assert!(
            api.account_uuid("revoked")
                .unwrap_err()
                .to_string()
                .contains("401")
        );
    }
}
