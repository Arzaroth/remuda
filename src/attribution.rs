//! What remuda writes to the ledger TokenGauge attributes spend with
//! (`selvedge::usage::ledger`): which login each CLI's files hold, and from
//! when. A failed write costs that line and nothing else: no command fails
//! for it.

use std::path::Path;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, UNIX_EPOCH};

use selvedge::usage::ledger::{self, Entry as Line, Ledger};

use crate::fsx::now_ms;
use crate::ops::LiveState;
use crate::provider::Provider;
use crate::store::{Entry, Store};

/// TokenGauge keeps 400 days of spend, and nothing older needs a login.
const KEEP_MS: i64 = 400 * 86_400_000;

/// A switch put `entry` in the CLI's files just now.
pub fn switched(store: &Store, p: &dyn Provider, entry: &Entry) {
    let _ = ledger::prune(store.root(), now_ms() - KEEP_MS);
    let _ = ledger::append(
        store.root(),
        &Line::Live {
            at: now_ms(),
            provider: p.id().to_owned(),
            account_id: Some(entry.meta.account_id.clone()),
            name: Some(entry.name.clone()),
        },
    );
}

/// The CLI's files were read and hold `live`. When that is not the login the
/// ledger last wrote down, something outside remuda changed it, at the
/// latest when the file was last written.
pub fn noticed(store: &Store, p: &dyn Provider, live: &LiveState) {
    let (account_id, name) = match live {
        LiveState::Stored { name, .. } => match store.get(p.id(), name) {
            Ok(Some(entry)) if entry.verified => (Some(entry.meta.account_id), Some(entry.name)),
            Ok(_) => (None, None),
            Err(_) => return,
        },
        LiveState::SignedOut | LiveState::Unstored { .. } | LiveState::Foreign { .. } => {
            (None, None)
        }
        LiveState::Unreadable { .. } => return,
    };
    let Ok(lines) = ledger::read(store.root()) else {
        return;
    };
    let last = lines.iter().rev().find_map(|line| match line {
        Line::Live {
            at,
            provider,
            account_id,
            ..
        } if provider == p.id() => Some((*at, account_id)),
        _ => None,
    });
    let at = match last {
        Some((_, last)) if *last == account_id => return,
        None if account_id.is_none() => return,
        None => now_ms(),
        Some((then, _)) => p
            .live_file()
            .as_deref()
            .and_then(modified_ms)
            .map_or_else(now_ms, |at| at.min(now_ms()))
            .max(then + 1),
    };
    let _ = ledger::append(
        store.root(),
        &Line::Live {
            at,
            provider: p.id().to_owned(),
            account_id,
            name,
        },
    );
}

fn modified_ms(path: &Path) -> Option<i64> {
    let at = std::fs::metadata(path).ok()?.modified().ok()?;
    i64::try_from(at.duration_since(UNIX_EPOCH).ok()?.as_millis()).ok()
}

/// Writes down each session a `remuda run` CLI moves through, so its usage is
/// the run's login whatever the CLI's own files hold meanwhile. Stops, with a
/// last look, when dropped.
pub struct RunWatch {
    stop: Option<mpsc::Sender<()>>,
    thread: Option<thread::JoinHandle<()>>,
}

/// Claude Code removes its session file on exit, so a session is seen only
/// while it runs: often enough that a short `-p` run or a quick `/clear` is
/// not missed, and the file is a few hundred bytes.
const FIRST_LOOK: Duration = Duration::from_millis(250);
const RUN_POLL: Duration = Duration::from_secs(1);

impl RunWatch {
    pub fn start(
        store: &Store,
        p: &dyn Provider,
        entry: &Entry,
        home: &Path,
        pid: u32,
    ) -> Option<RunWatch> {
        let session_of = p.run_sessions()?;
        let start = crate::pidstart::start_time(pid)?;
        let root = store.root().to_owned();
        let home = home.to_owned();
        let provider = p.id().to_owned();
        let account_id = entry.meta.account_id.clone();
        let name = entry.name.clone();
        let (stop, stopped) = mpsc::channel();
        let thread = thread::spawn(move || {
            let mut last: Option<String> = None;
            let mut wait = FIRST_LOOK;
            loop {
                let finished = !matches!(
                    stopped.recv_timeout(std::mem::replace(&mut wait, RUN_POLL)),
                    Err(mpsc::RecvTimeoutError::Timeout)
                );
                if let Some(session) = session_of(&home, pid, start)
                    && last.as_ref() != Some(&session)
                {
                    let claimed = Ledger::load(&root).is_ok_and(|l| l.claims(&provider, &session));
                    if !claimed {
                        let _ = ledger::append(
                            &root,
                            &Line::Run {
                                at: now_ms(),
                                provider: provider.clone(),
                                session: session.clone(),
                                account_id: account_id.clone(),
                                name: Some(name.clone()),
                            },
                        );
                    }
                    last = Some(session);
                }
                if finished {
                    break;
                }
            }
        });
        Some(RunWatch {
            stop: Some(stop),
            thread: Some(thread),
        })
    }
}

impl Drop for RunWatch {
    fn drop(&mut self) {
        drop(self.stop.take());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use selvedge::usage::ledger::{self, Entry as Line, Ledger};

    use crate::fsx::now_ms;
    use crate::ops::testing::*;
    use crate::ops::{switch, sync_live};
    use crate::provider::Provider;

    fn lines(e: &Env) -> Vec<Line> {
        ledger::read(e.store.root()).unwrap()
    }

    fn account(line: &Line) -> Option<&str> {
        match line {
            Line::Live { account_id, .. } => account_id.as_deref(),
            Line::Run { account_id, .. } => Some(account_id),
        }
    }

    #[test]
    fn the_live_login_is_written_down_once_and_a_switch_moves_it() {
        let e = env(&OFFLINE);
        e.stored("work", "u-work", HOUR);
        e.stored("perso", "u-perso", HOUR);
        let work = e.store.get("claude", "work").unwrap().unwrap();
        e.sign_in(work.creds, crate::ops::testing::account("u-work"));
        let before = now_ms();

        sync_live(&e.store, &e.claude).unwrap();
        sync_live(&e.store, &e.claude).unwrap();
        assert_eq!(lines(&e).len(), 1);
        assert_eq!(account(&lines(&e)[0]), Some("u-work"));
        assert!(lines(&e)[0].at() >= before);

        switch(&e.store, &e.claude, "perso", false).unwrap();
        sync_live(&e.store, &e.claude).unwrap();

        let written = lines(&e);
        assert_eq!(written.len(), 2);
        assert_eq!(
            written[1],
            Line::Live {
                at: written[1].at(),
                provider: "claude".into(),
                account_id: Some("u-perso".into()),
                name: Some("perso".into()),
            }
        );
        let ledger = Ledger::load(e.store.root()).unwrap();
        assert_eq!(ledger.login_at("claude", None, now_ms()), Some("u-perso"));
    }

    #[test]
    fn a_login_changed_outside_remuda_is_dated_by_its_file() {
        let e = env(&OFFLINE);
        e.stored("work", "u-work", HOUR);
        e.stored("perso", "u-perso", HOUR);
        let work = e.store.get("claude", "work").unwrap().unwrap();
        e.sign_in(work.creds, crate::ops::testing::account("u-work"));
        ledger::append(
            e.store.root(),
            &Line::Live {
                at: now_ms() - 10 * HOUR,
                provider: "claude".into(),
                account_id: Some("u-work".into()),
                name: Some("work".into()),
            },
        )
        .unwrap();
        let perso = e.store.get("claude", "perso").unwrap().unwrap();
        e.sign_in(perso.creds, crate::ops::testing::account("u-perso"));
        let changed = std::time::SystemTime::now() - std::time::Duration::from_secs(3 * 3600);
        std::fs::File::options()
            .write(true)
            .open(e.claude.live_file().unwrap())
            .unwrap()
            .set_modified(changed)
            .unwrap();

        sync_live(&e.store, &e.claude).unwrap();

        let written = lines(&e);
        assert_eq!(written.len(), 2);
        assert_eq!(account(&written[1]), Some("u-perso"));
        let at = changed
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        assert_eq!(written[1].at(), at);
    }

    #[test]
    fn signing_out_ends_the_last_login_and_nothing_starts_the_ledger_but_a_login() {
        let e = env(&OFFLINE);
        e.stored("work", "u-work", HOUR);

        sync_live(&e.store, &e.claude).unwrap();
        assert!(lines(&e).is_empty());

        let work = e.store.get("claude", "work").unwrap().unwrap();
        e.sign_in(work.creds, crate::ops::testing::account("u-work"));
        sync_live(&e.store, &e.claude).unwrap();
        std::fs::remove_file(e.claude.live_file().unwrap()).unwrap();
        sync_live(&e.store, &e.claude).unwrap();

        let written = lines(&e);
        assert_eq!(written.len(), 2);
        assert_eq!(account(&written[1]), None);
        assert!(written[1].at() > written[0].at());
    }

    #[test]
    fn a_login_nobody_stored_is_nobody_s() {
        let e = env(&OFFLINE);
        e.stored("work", "u-work", HOUR);
        let work = e.store.get("claude", "work").unwrap().unwrap();
        e.sign_in(work.creds, crate::ops::testing::account("u-work"));
        sync_live(&e.store, &e.claude).unwrap();
        e.sign_in(
            oauth("a-x", "r-x", HOUR),
            crate::ops::testing::account("u-x"),
        );

        sync_live(&e.store, &e.claude).unwrap();

        assert_eq!(account(&lines(&e)[1]), None);
    }

    fn running(home: &std::path::Path, session: &str, start: u64) {
        let dir = home.join("sessions");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(format!("{}.json", std::process::id())),
            serde_json::json!({
                "pid": std::process::id(),
                "sessionId": session,
                "procStart": crate::pidstart::claude_proc_start(start),
                "kind": "interactive",
            })
            .to_string(),
        )
        .unwrap();
    }

    fn watch(e: &Env, home: &std::path::Path) -> Option<super::RunWatch> {
        let entry = e.store.get("claude", "work").unwrap().unwrap();
        super::RunWatch::start(&e.store, &e.claude, &entry, home, std::process::id())
    }

    #[test]
    fn a_run_writes_down_its_session_and_the_next_one_after_a_clear() {
        let e = env(&OFFLINE);
        e.stored("work", "u-work", HOUR);
        let home = e.tmp.path().join("run-home");
        let me = crate::pidstart::start_time(std::process::id()).unwrap();
        running(&home, "s-1", me);

        let watching = watch(&e, &home).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1500));
        running(&home, "s-2", me);
        drop(watching);

        let sessions: Vec<(String, String)> = lines(&e)
            .into_iter()
            .filter_map(|line| match line {
                Line::Run {
                    session,
                    account_id,
                    ..
                } => Some((session, account_id)),
                Line::Live { .. } => None,
            })
            .collect();
        assert_eq!(
            sessions,
            vec![
                ("s-1".to_string(), "u-work".to_string()),
                ("s-2".to_string(), "u-work".to_string())
            ]
        );
        let ledger = Ledger::load(e.store.root()).unwrap();
        assert_eq!(ledger.login_at("claude", Some("s-2"), 0), Some("u-work"));
    }

    #[test]
    fn a_session_file_left_by_another_process_with_this_pid_is_not_this_run_s() {
        let e = env(&OFFLINE);
        e.stored("work", "u-work", HOUR);
        let home = e.tmp.path().join("run-home");
        let me = crate::pidstart::start_time(std::process::id()).unwrap();
        running(&home, "s-old", me + 1);

        drop(watch(&e, &home).unwrap());

        assert!(lines(&e).is_empty());
    }

    #[test]
    fn a_session_already_claimed_is_not_written_twice() {
        let e = env(&OFFLINE);
        e.stored("work", "u-work", HOUR);
        let home = e.tmp.path().join("run-home");
        let me = crate::pidstart::start_time(std::process::id()).unwrap();
        running(&home, "s-1", me);

        drop(watch(&e, &home).unwrap());
        drop(watch(&e, &home).unwrap());

        assert_eq!(lines(&e).len(), 1);
    }
}
