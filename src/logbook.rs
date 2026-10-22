use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::fsx::{lock_file, now_ms};
use crate::store::Store;

/// Past this size the log moves to `.log.1.jsonl`, replacing the one there,
/// so the two files never hold much more than twice this.
const ROTATE_AT: u64 = 1 << 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Switch,
    Auto,
    Refresh,
    Warm,
    Reset,
    Proxy,
    Store,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Switch => "switch",
            Kind::Auto => "auto",
            Kind::Refresh => "refresh",
            Kind::Warm => "warm",
            Kind::Reset => "reset",
            Kind::Proxy => "proxy",
            Kind::Store => "store",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum By {
    Cli,
    Page,
    Auto,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Event {
    pub at: i64,
    pub kind: Kind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<By>,
    pub ok: bool,
    pub message: String,
}

impl Event {
    pub fn new(kind: Kind, ok: bool, message: impl Into<String>) -> Event {
        Event {
            at: now_ms(),
            kind,
            provider: None,
            name: None,
            by: None,
            ok,
            message: message.into(),
        }
    }

    pub fn about(mut self, provider: &str, name: Option<&str>) -> Event {
        self.provider = Some(provider.to_owned());
        self.name = name.map(str::to_owned);
        self
    }

    pub fn by(mut self, by: By) -> Event {
        self.by = Some(by);
        self
    }

    /// `claude/work`, `claude`, or nothing for an event about the whole store.
    pub fn subject(&self) -> String {
        match (&self.provider, &self.name) {
            (Some(p), Some(n)) => format!("{p}/{n}"),
            (Some(p), None) => p.clone(),
            _ => String::new(),
        }
    }

    /// Whether `spec` (a provider, `provider/name` or a bare name) names
    /// what this event is about.
    pub fn concerns(&self, spec: &str) -> bool {
        let provider = self.provider.as_deref();
        let name = self.name.as_deref();
        match spec.split_once('/') {
            Some((p, n)) => provider == Some(p) && name == Some(n),
            None => provider == Some(spec) || name == Some(spec),
        }
    }

    fn same_subject(&self, other: &Event) -> bool {
        self.kind == other.kind && self.provider == other.provider && self.name == other.name
    }
}

fn path(store: &Store) -> PathBuf {
    store.root().join(".log.jsonl")
}

fn rotated_path(store: &Store) -> PathBuf {
    store.root().join(".log.1.jsonl")
}

/// Best effort, as the other records: a log that cannot be written never
/// fails the command it describes.
pub fn note(store: &Store, event: Event) {
    if let Err(e) = append(store, &event, false) {
        eprintln!("warning: could not log this: {e:#}");
    }
}

/// Logs a failure only when the latest event about the same thing does not
/// already say it, so a login every timer run fails to refresh is one line
/// rather than one per run.
pub fn note_once(store: &Store, event: Event) {
    if let Err(e) = append(store, &event, true) {
        eprintln!("warning: could not log this: {e:#}");
    }
}

fn append(store: &Store, event: &Event, once: bool) -> Result<()> {
    if !store.root().is_dir() {
        return Ok(());
    }
    let _lock = lock_file(&store.root().join(".log.lock"))?;
    if once
        && !event.ok
        && read(store)
            .iter()
            .rev()
            .find(|e| e.same_subject(event))
            .is_some_and(|last| !last.ok && last.message == event.message)
    {
        return Ok(());
    }
    let path = path(store);
    if fs::metadata(&path).is_ok_and(|m| m.len() >= ROTATE_AT) {
        fs::rename(&path, rotated_path(store))
            .with_context(|| format!("failed to rotate {}", path.display()))?;
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .read(true)
        .mode(0o600)
        .open(&path)
        .with_context(|| format!("failed to open {}", path.display()))?;
    let mut line = String::new();
    if !ends_a_line(&mut file)? {
        line.push('\n');
    }
    line.push_str(&serde_json::to_string(event)?);
    line.push('\n');
    file.write_all(line.as_bytes())
        .with_context(|| format!("failed to write {}", path.display()))
}

/// An append cut short leaves a line without its end; the next one starts
/// on a line of its own rather than spoiling both.
fn ends_a_line(file: &mut File) -> Result<bool> {
    if file.metadata()?.len() == 0 {
        return Ok(true);
    }
    file.seek(SeekFrom::End(-1))?;
    let mut last = [0u8];
    file.read_exact(&mut last)?;
    Ok(last[0] == b'\n')
}

/// Every event kept, oldest first. A line that does not parse is skipped.
pub fn read(store: &Store) -> Vec<Event> {
    [rotated_path(store), path(store)]
        .iter()
        .filter_map(|p| fs::read_to_string(p).ok())
        .flat_map(|text| {
            text.lines()
                .filter_map(|l| serde_json::from_str(l).ok())
                .collect::<Vec<Event>>()
        })
        .collect()
}

/// The latest `limit` events `spec` concerns, or of any login without one,
/// oldest first.
pub fn recent(store: &Store, spec: Option<&str>, limit: usize) -> Vec<Event> {
    let mut all: Vec<Event> = read(store)
        .into_iter()
        .filter(|e| spec.is_none_or(|s| e.concerns(s)))
        .collect();
    let skip = all.len().saturating_sub(limit);
    all.drain(..skip);
    all
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, Store) {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::open(tmp.path());
        (tmp, store)
    }

    fn failed(message: &str) -> Event {
        Event::new(Kind::Refresh, false, message).about("claude", Some("work"))
    }

    #[test]
    fn events_are_kept_in_order_with_what_they_are_about() {
        let (_tmp, store) = store();
        assert!(read(&store).is_empty());
        let switch = Event::new(Kind::Switch, true, "switched Claude Code to work")
            .about("claude", Some("work"))
            .by(By::Page);
        note(&store, switch.clone());
        note(
            &store,
            Event::new(Kind::Proxy, false, "signed out").about("codex", None),
        );
        let all = read(&store);
        assert_eq!(all.len(), 2);
        assert_eq!(all[0], switch);
        assert_eq!(all[0].subject(), "claude/work");
        assert_eq!(all[1].subject(), "codex");

        let line = fs::read_to_string(path(&store)).unwrap();
        let first: serde_json::Value = serde_json::from_str(line.lines().next().unwrap()).unwrap();
        assert_eq!(first["kind"], "switch");
        assert_eq!(first["by"], "page");
        assert!(first.get("name").is_some());
        let second: serde_json::Value = serde_json::from_str(line.lines().nth(1).unwrap()).unwrap();
        assert!(second.get("name").is_none() && second.get("by").is_none());
    }

    #[test]
    fn the_log_is_private_and_never_creates_the_store() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = Store::open(&tmp.path().join("store"));
        note(&missing, failed("x"));
        assert!(!tmp.path().join("store").exists());

        let (_tmp, store) = store();
        note(&store, failed("x"));
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(path(&store)).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn a_repeated_failure_is_logged_once_until_something_else_happens() {
        let (_tmp, store) = store();
        note_once(&store, failed("invalid_grant"));
        note_once(&store, failed("invalid_grant"));
        note_once(
            &store,
            Event::new(Kind::Refresh, false, "invalid_grant").about("claude", Some("perso")),
        );
        assert_eq!(read(&store).len(), 2);
        note_once(&store, failed("timed out"));
        assert_eq!(read(&store).len(), 3);
        note(
            &store,
            Event::new(Kind::Refresh, true, "refreshed").about("claude", Some("work")),
        );
        note_once(&store, failed("timed out"));
        assert_eq!(read(&store).len(), 5);
    }

    #[test]
    fn a_torn_line_costs_only_itself() {
        let (_tmp, store) = store();
        note(&store, failed("a"));
        let mut file = OpenOptions::new().append(true).open(path(&store)).unwrap();
        file.write_all(b"{\"at\": 1, \"ki").unwrap();
        note(&store, failed("b"));
        let messages: Vec<String> = read(&store).into_iter().map(|e| e.message).collect();
        assert_eq!(messages, ["a", "b"]);
    }

    #[test]
    fn a_full_log_rotates_and_both_halves_are_read() {
        let (_tmp, store) = store();
        note(&store, failed("old"));
        let big = format!("{}\n", "x".repeat(ROTATE_AT as usize));
        let mut file = OpenOptions::new().append(true).open(path(&store)).unwrap();
        file.write_all(big.as_bytes()).unwrap();
        note(&store, failed("new"));
        assert!(rotated_path(&store).exists());
        assert!(fs::metadata(path(&store)).unwrap().len() < 1024);
        let messages: Vec<String> = read(&store).into_iter().map(|e| e.message).collect();
        assert_eq!(messages, ["old", "new"]);
    }

    #[test]
    fn recent_filters_by_provider_login_or_bare_name_and_keeps_the_latest() {
        let (_tmp, store) = store();
        for (p, n) in [("claude", "work"), ("codex", "work"), ("claude", "perso")] {
            note(
                &store,
                Event::new(Kind::Switch, true, format!("{p}/{n}")).about(p, Some(n)),
            );
        }
        note(
            &store,
            Event::new(Kind::Proxy, false, "codex").about("codex", None),
        );
        let messages = |spec, limit| -> Vec<String> {
            recent(&store, spec, limit)
                .into_iter()
                .map(|e| e.message)
                .collect()
        };
        assert_eq!(
            messages(Some("claude"), 10),
            ["claude/work", "claude/perso"]
        );
        assert_eq!(messages(Some("work"), 10), ["claude/work", "codex/work"]);
        assert_eq!(messages(Some("codex/work"), 10), ["codex/work"]);
        assert_eq!(messages(None, 2), ["claude/perso", "codex"]);
    }
}
