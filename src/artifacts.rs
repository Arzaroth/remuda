use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::fsx::{now_ms, read_record, write_record};
use crate::logbook::By;
use crate::ops::{self, LiveState};
use crate::provider::Provider;
use crate::store::Store;

pub const TOOLS: [&str; 3] = ["Artifact", "ArtifactComments", "ArtifactData"];

/// How long another session's call may hold the login before a call that
/// needs another one switches anyway.
const WAIT_MS: i64 = 30_000;
/// A switch-back no hook came to make: the session crashed or its call never
/// returned.
const STALE_MS: i64 = 10 * 60_000;
const FORGET_MS: i64 = 30 * 86_400_000;

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Ledger {
    /// An artifact's slug or id, and the account that published it.
    #[serde(default)]
    owners: BTreeMap<String, String>,
    #[serde(default)]
    sessions: BTreeMap<String, Session>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Session {
    /// The file each artifact was published from: a republish names the file,
    /// not the artifact.
    #[serde(default)]
    paths: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    restore: Option<Restore>,
    #[serde(default)]
    seen: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Restore {
    /// The login the call was switched to, and the one to go back to.
    on: String,
    back: String,
    at: i64,
}

#[derive(Deserialize)]
struct Input {
    hook_event_name: String,
    session_id: String,
    tool_name: String,
    #[serde(default)]
    tool_input: Value,
    #[serde(default)]
    tool_response: Value,
}

fn path(store: &Store) -> PathBuf {
    store.root().join(".artifacts.json")
}

/// The slug or id an artifact link ends with, the query and fragment aside.
pub fn key(url: &str) -> Option<String> {
    let url = url.split(['?', '#']).next()?;
    let mut segments = url.split('/').skip_while(|s| *s != "artifact").skip(1);
    segments.next().filter(|s| !s.is_empty()).map(str::to_owned)
}

/// Where the hook may act: the CLI's own login, or a `remuda run` session's,
/// which stays on its login whatever the CLI's files hold.
pub enum Seat<'a> {
    Live,
    Run(&'a str),
}

struct Login {
    name: String,
    account: String,
}

fn live(store: &Store, p: &dyn Provider) -> Result<Option<Login>> {
    let LiveState::Stored { name, .. } = ops::sync_live(store, p)? else {
        return Ok(None);
    };
    login(store, p, &name)
}

fn login(store: &Store, p: &dyn Provider, name: &str) -> Result<Option<Login>> {
    Ok(store.get(p.id(), name)?.map(|e| Login {
        name: e.name,
        account: e.meta.account_id,
    }))
}

fn owner_login(store: &Store, p: &dyn Provider, account: &str) -> Result<Option<String>> {
    Ok(store
        .list(p.id())?
        .into_iter()
        .find(|e| e.verified && e.meta.account_id == account)
        .map(|e| e.name))
}

fn said(event: &str, text: String) -> Value {
    json!({"hookSpecificOutput": {"hookEventName": event, "additionalContext": text}})
}

/// Handles one Claude Code hook event on an artifact tool, given its JSON;
/// what to print back, if anything. `pause` is called, the store unlocked,
/// while another session's call holds the login this one must leave.
pub fn hook(
    store: &Store,
    p: &dyn Provider,
    seat: Seat,
    raw: &str,
    pause: &dyn Fn(),
) -> Result<Option<Value>> {
    let input: Input = serde_json::from_str(raw)?;
    if !TOOLS.contains(&input.tool_name.as_str()) {
        return Ok(None);
    }
    match input.hook_event_name.as_str() {
        "PreToolUse" => before(store, p, &seat, &input, pause),
        "PostToolUse" | "PostToolUseFailure" => after(store, p, &seat, &input),
        _ => Ok(None),
    }
}

fn target(input: &Input, session: &Session) -> Option<String> {
    if let Some(url) = input.tool_input.get("url").and_then(Value::as_str) {
        return key(url);
    }
    let path = input.tool_input.get("file_path").and_then(Value::as_str)?;
    session.paths.get(path).cloned()
}

fn before(
    store: &Store,
    p: &dyn Provider,
    seat: &Seat,
    input: &Input,
    pause: &dyn Fn(),
) -> Result<Option<Value>> {
    const EVENT: &str = "PreToolUse";
    let started = now_ms();
    loop {
        let lock = store.lock()?;
        let mut ledger = load(store);
        restore_stale(store, p, &mut ledger, &input.session_id)?;
        let session = ledger.sessions.entry(input.session_id.clone()).or_default();
        session.seen = now_ms();
        let Some(owner) = target(input, session).and_then(|k| ledger.owners.get(&k).cloned())
        else {
            save(store, &ledger);
            return Ok(None);
        };
        if let Seat::Run(name) = seat {
            save(store, &ledger);
            let here = login(store, p, name)?;
            if here.is_some_and(|l| l.account == owner) {
                return Ok(None);
            }
            let whose = owner_login(store, p, &owner)?.unwrap_or_else(|| owner.clone());
            return Ok(Some(said(
                EVENT,
                format!(
                    "remuda: this session runs on {name} through `remuda run`, but {whose} published this artifact, so the call is likely refused. Publishing again without its url makes a new artifact under {name}."
                ),
            )));
        }
        let Some(current) = live(store, p)? else {
            save(store, &ledger);
            return Ok(None);
        };
        if current.account == owner {
            save(store, &ledger);
            return Ok(None);
        }
        let Some(to) = owner_login(store, p, &owner)? else {
            save(store, &ledger);
            return Ok(Some(said(
                EVENT,
                format!(
                    "remuda: this artifact was published by account {owner}, which remuda does not store, and {} is signed in now: the call is likely refused.",
                    current.name
                ),
            )));
        };
        let busy = ledger.sessions.iter().any(|(id, s)| {
            *id != input.session_id && s.restore.as_ref().is_some_and(|r| r.on == current.name)
        });
        if busy && now_ms() - started < WAIT_MS {
            drop(lock);
            pause();
            continue;
        }
        let why = "for an artifact it published";
        if let Err(e) = ops::switch_when(store, p, &to, false, By::Hook, Some(why), &|| Ok(())) {
            save(store, &ledger);
            return Ok(Some(said(
                EVENT,
                format!(
                    "remuda: {to} published this artifact but could not be switched to ({e:#}); the call runs on {} and is likely refused.",
                    current.name
                ),
            )));
        }
        let session = ledger.sessions.entry(input.session_id.clone()).or_default();
        let back = session
            .restore
            .take()
            .map_or(current.name.clone(), |r| r.back);
        session.restore = Some(Restore {
            on: to.clone(),
            back: back.clone(),
            at: now_ms(),
        });
        save(store, &ledger);
        return Ok(Some(said(
            EVENT,
            format!(
                "remuda switched Claude Code from {} to {to}, which published this artifact; it switches back to {back} once the call returns.",
                current.name
            ),
        )));
    }
}

fn after(store: &Store, p: &dyn Provider, seat: &Seat, input: &Input) -> Result<Option<Value>> {
    const EVENT: &str = "PostToolUse";
    let _lock = store.lock()?;
    let mut ledger = load(store);
    let account = match seat {
        Seat::Run(name) => login(store, p, name)?,
        Seat::Live => live(store, p)?,
    };
    let session = ledger.sessions.entry(input.session_id.clone()).or_default();
    session.seen = now_ms();
    if input.hook_event_name == "PostToolUse"
        && let Some(account) = &account
    {
        learn(&mut ledger, input, &account.account);
    }
    let Some(restore) = ledger
        .sessions
        .get_mut(&input.session_id)
        .and_then(|s| s.restore.take())
    else {
        save(store, &ledger);
        return Ok(None);
    };
    let said_back = put_back(store, p, &restore, account.as_ref());
    save(store, &ledger);
    Ok(Some(said(
        if input.hook_event_name == "PostToolUseFailure" {
            "PostToolUseFailure"
        } else {
            EVENT
        },
        said_back?,
    )))
}

/// Switches back to the login a call left, unless something else moved the
/// CLI meanwhile.
fn put_back(
    store: &Store,
    p: &dyn Provider,
    restore: &Restore,
    now_on: Option<&Login>,
) -> Result<String> {
    if now_on.is_none_or(|l| l.name != restore.on) {
        return Ok(format!(
            "remuda left Claude Code where it is: it was moved off {} since the artifact call began.",
            restore.on
        ));
    }
    let why = "back after an artifact call";
    match ops::switch_when(store, p, &restore.back, false, By::Hook, Some(why), &|| {
        Ok(())
    }) {
        Ok(_) => Ok(format!(
            "remuda switched Claude Code back to {} after the artifact call.",
            restore.back
        )),
        Err(e) => Ok(format!(
            "remuda could not switch Claude Code back to {} ({e:#}); it stays on {} until auto mode or `remuda use` moves it.",
            restore.back, restore.on
        )),
    }
}

fn restore_stale(store: &Store, p: &dyn Provider, ledger: &mut Ledger, except: &str) -> Result<()> {
    let now = now_ms();
    let stale: Vec<String> = ledger
        .sessions
        .iter()
        .filter(|(id, s)| {
            *id != except && s.restore.as_ref().is_some_and(|r| now - r.at > STALE_MS)
        })
        .map(|(id, _)| id.clone())
        .collect();
    for id in stale {
        let Some(restore) = ledger.sessions.get_mut(&id).and_then(|s| s.restore.take()) else {
            continue;
        };
        let current = live(store, p)?;
        put_back(store, p, &restore, current.as_ref())?;
    }
    ledger
        .sessions
        .retain(|_, s| s.restore.is_some() || now - s.seen < FORGET_MS);
    Ok(())
}

/// What a call's result says about which account owns which artifact.
fn learn(ledger: &mut Ledger, input: &Input, account: &str) {
    if input.tool_name != "Artifact" {
        return;
    }
    let r = &input.tool_response;
    let action = input
        .tool_input
        .get("action")
        .and_then(Value::as_str)
        .unwrap_or("publish");
    match action {
        "publish" => {
            let Some(slug) = r.get("url").and_then(Value::as_str).and_then(key) else {
                return;
            };
            if let Some(id) = r.get("artifact_id").and_then(Value::as_str) {
                ledger.owners.insert(id.to_owned(), account.to_owned());
            }
            let file = r
                .get("path")
                .or_else(|| input.tool_input.get("file_path"))
                .and_then(Value::as_str);
            if let Some(file) = file
                && let Some(session) = ledger.sessions.get_mut(&input.session_id)
            {
                session.paths.insert(file.to_owned(), slug.clone());
            }
            ledger.owners.insert(slug, account.to_owned());
        }
        "list" => {
            let scope = input.tool_input.get("scope").and_then(Value::as_str);
            if !matches!(scope, None | Some("mine")) || input.tool_input.get("type").is_some() {
                return;
            }
            let urls = r
                .get("artifacts")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|a| a.get("url").and_then(Value::as_str))
                .filter_map(key);
            for slug in urls {
                ledger.owners.insert(slug, account.to_owned());
            }
        }
        _ => {}
    }
}

fn load(store: &Store) -> Ledger {
    read_record(&path(store)).unwrap_or_default()
}

fn save(store: &Store, ledger: &Ledger) {
    write_record(&path(store), ledger, "which account owns which artifact");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::testing::*;
    use std::cell::Cell;

    fn event(name: &str, session: &str, tool_input: Value, response: Value) -> String {
        json!({
            "hook_event_name": name,
            "session_id": session,
            "tool_name": "Artifact",
            "tool_input": tool_input,
            "tool_response": response,
        })
        .to_string()
    }

    fn published(slug: &str, file: &str) -> Value {
        json!({"url": format!("https://claude.ai/artifact/{slug}"), "path": file, "artifact_id": format!("id-{slug}"), "updated": false})
    }

    fn on(e: &Env, name: &str) {
        let entry = e.store.get("claude", name).unwrap().unwrap();
        e.sign_in(entry.creds, account(&entry.meta.account_id));
    }

    fn active(e: &Env) -> String {
        match ops::sync_live(&e.store, &e.claude).unwrap() {
            LiveState::Stored { name, .. } => name,
            other => panic!("{other:?}"),
        }
    }

    fn two() -> Env {
        let e = env(&OFFLINE);
        e.stored("work", "u-work", HOUR);
        e.stored("perso", "u-perso", HOUR);
        on(&e, "perso");
        e
    }

    fn run(e: &Env, raw: &str) -> Option<Value> {
        hook(&e.store, &e.claude, Seat::Live, raw, &|| panic!("no wait")).unwrap()
    }

    fn context(v: &Option<Value>) -> &str {
        v.as_ref().unwrap()["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap()
    }

    #[test]
    fn a_link_names_its_artifact_by_its_last_segment() {
        assert_eq!(
            key("https://claude.ai/artifact/6uEPq?x=1#y").as_deref(),
            Some("6uEPq")
        );
        assert_eq!(
            key("https://claude.ai/code/artifact/2fc8-9f9b/").as_deref(),
            Some("2fc8-9f9b")
        );
        assert_eq!(key("https://claude.ai/chat/abc"), None);
        assert_eq!(key("https://claude.ai/artifact/"), None);
    }

    #[test]
    fn a_call_on_an_artifact_another_login_published_switches_to_it_and_back() {
        let e = two();
        let publish = json!({"file_path": "/s/page.html"});
        assert!(run(&e, &event("PreToolUse", "s1", publish.clone(), Value::Null)).is_none());
        assert!(
            run(
                &e,
                &event(
                    "PostToolUse",
                    "s1",
                    publish,
                    published("Abc", "/s/page.html")
                )
            )
            .is_none()
        );

        ops::switch(&e.store, &e.claude, "work", false).unwrap();
        let again = json!({"file_path": "/s/page.html"});
        let said = run(&e, &event("PreToolUse", "s1", again.clone(), Value::Null));
        assert_eq!(active(&e), "perso");
        assert!(context(&said).contains("from work to perso"), "{said:?}");

        let said = run(
            &e,
            &event("PostToolUse", "s1", again, published("Abc", "/s/page.html")),
        );
        assert_eq!(active(&e), "work");
        assert!(context(&said).contains("back to work"), "{said:?}");
        let switches: Vec<_> = crate::logbook::read(&e.store)
            .into_iter()
            .filter(|ev| ev.by == Some(By::Hook))
            .collect();
        assert_eq!(switches.len(), 2);
    }

    #[test]
    fn a_link_learnt_from_a_list_is_switched_for_and_a_failed_call_still_switches_back() {
        let e = two();
        let list = json!({"action": "list"});
        let rows = json!({"artifacts": [{"url": "https://claude.ai/artifact/Old"}], "total": 1});
        run(&e, &event("PostToolUse", "s1", list, rows));
        ops::switch(&e.store, &e.claude, "work", false).unwrap();

        let read = json!({"action": "read", "url": "https://claude.ai/artifact/Old"});
        run(&e, &event("PreToolUse", "s2", read.clone(), Value::Null));
        assert_eq!(active(&e), "perso");
        run(&e, &event("PostToolUseFailure", "s2", read, Value::Null));
        assert_eq!(active(&e), "work");
    }

    #[test]
    fn a_shared_list_and_an_unknown_artifact_leave_the_login_alone() {
        let e = two();
        let shared = json!({"action": "list", "scope": "shared"});
        let rows = json!({"artifacts": [{"url": "https://claude.ai/artifact/Theirs"}]});
        run(&e, &event("PostToolUse", "s1", shared, rows));
        ops::switch(&e.store, &e.claude, "work", false).unwrap();

        for slug in ["Theirs", "Never"] {
            let read =
                json!({"action": "read", "url": format!("https://claude.ai/artifact/{slug}")});
            assert!(run(&e, &event("PreToolUse", "s1", read, Value::Null)).is_none());
            assert_eq!(active(&e), "work");
        }
    }

    #[test]
    fn a_login_moved_during_the_call_is_not_moved_back() {
        let e = two();
        let publish = json!({"file_path": "/p.html"});
        run(
            &e,
            &event("PostToolUse", "s1", publish, published("P", "/p.html")),
        );
        ops::switch(&e.store, &e.claude, "work", false).unwrap();
        let read = json!({"action": "read", "url": "https://claude.ai/artifact/P"});
        run(&e, &event("PreToolUse", "s1", read.clone(), Value::Null));
        ops::switch(&e.store, &e.claude, "work", false).unwrap();

        let said = run(&e, &event("PostToolUse", "s1", read, Value::Null));

        assert_eq!(active(&e), "work");
        assert!(
            context(&said).contains("left Claude Code where it is"),
            "{said:?}"
        );
    }

    #[test]
    fn consecutive_calls_go_back_to_the_login_before_the_first() {
        let e = two();
        run(
            &e,
            &event("PostToolUse", "s1", json!({}), published("P", "/p.html")),
        );
        ops::switch(&e.store, &e.claude, "work", false).unwrap();
        let read = json!({"action": "read", "url": "https://claude.ai/artifact/P"});
        run(&e, &event("PreToolUse", "s1", read.clone(), Value::Null));
        run(&e, &event("PreToolUse", "s1", read.clone(), Value::Null));
        assert_eq!(active(&e), "perso");
        run(&e, &event("PostToolUse", "s1", read, Value::Null));
        assert_eq!(active(&e), "work");
    }

    #[test]
    fn a_call_waits_while_another_session_holds_the_login_it_must_leave() {
        let e = two();
        run(
            &e,
            &event("PostToolUse", "a", json!({}), published("P", "/p.html")),
        );
        ops::switch(&e.store, &e.claude, "work", false).unwrap();
        run(
            &e,
            &event("PostToolUse", "b", json!({}), published("W", "/w.html")),
        );
        let read_p = json!({"action": "read", "url": "https://claude.ai/artifact/P"});
        run(&e, &event("PreToolUse", "a", read_p.clone(), Value::Null));
        assert_eq!(active(&e), "perso");

        let read_w = json!({"action": "read", "url": "https://claude.ai/artifact/W"});
        let waits = Cell::new(0);
        let pause = || {
            waits.set(waits.get() + 1);
            hook(
                &e.store,
                &e.claude,
                Seat::Live,
                &event("PostToolUse", "a", read_p.clone(), Value::Null),
                &|| {},
            )
            .unwrap();
        };
        hook(
            &e.store,
            &e.claude,
            Seat::Live,
            &event("PreToolUse", "b", read_w, Value::Null),
            &pause,
        )
        .unwrap();
        assert_eq!(waits.get(), 1);
        assert_eq!(active(&e), "work");
    }

    #[test]
    fn a_run_session_is_never_switched_but_hears_whose_artifact_it_is() {
        let e = two();
        run(
            &e,
            &event("PostToolUse", "s1", json!({}), published("P", "/p.html")),
        );
        let read = json!({"action": "read", "url": "https://claude.ai/artifact/P"});
        let said = hook(
            &e.store,
            &e.claude,
            Seat::Run("work"),
            &event("PreToolUse", "s2", read, Value::Null),
            &|| panic!(),
        )
        .unwrap();
        assert_eq!(active(&e), "perso");
        assert!(context(&said).contains("perso published"), "{said:?}");
    }

    #[test]
    fn other_tools_and_events_are_ignored() {
        let e = two();
        let raw = json!({"hook_event_name": "PreToolUse", "session_id": "s", "tool_name": "Bash", "tool_input": {}}).to_string();
        assert!(run(&e, &raw).is_none());
        let raw = json!({"hook_event_name": "Stop", "session_id": "s", "tool_name": "Artifact"})
            .to_string();
        assert!(run(&e, &raw).is_none());
    }
}
