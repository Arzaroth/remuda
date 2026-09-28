use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::fsx::{read_json, write_json};
use crate::store::Store;

/// What the last scheduled refresh did, kept in the store it ran against so
/// the page can say whether the timer is keeping the inactive logins fresh.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Run {
    pub at: i64,
    pub refreshed: Vec<String>,
    pub problems: Vec<String>,
}

fn record_path(store: &Store) -> PathBuf {
    store.root().join(".last-refresh.json")
}

fn lines(bytes: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(bytes)
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_owned)
        .collect()
}

impl Run {
    pub fn from_output(at: i64, skipped: &[String], out: &[u8], err: &[u8]) -> Run {
        Run {
            at,
            refreshed: lines(out),
            problems: skipped.iter().cloned().chain(lines(err)).collect(),
        }
    }
}

pub fn last(store: &Store) -> Option<Run> {
    read_json(&record_path(store))
        .ok()
        .flatten()
        .and_then(|v| serde_json::from_value(v).ok())
}

/// Best effort, like the directory record: a run that cannot be noted still
/// refreshed what it refreshed.
pub fn record(store: &Store, run: &Run) {
    let written = serde_json::to_value(run)
        .map_err(anyhow::Error::from)
        .and_then(|v| write_json(&record_path(store), &v));
    if let Err(e) = written {
        eprintln!("warning: could not note this refresh for the page: {e:#}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_run_keeps_what_it_refreshed_and_everything_that_went_wrong() {
        let run = Run::from_output(
            42,
            &["codex: not refreshing, ...".to_owned()],
            b"claude/work: refreshed\n\n",
            b"claude/perso: invalid_grant\n",
        );
        assert_eq!(run.refreshed, ["claude/work: refreshed"]);
        assert_eq!(
            run.problems,
            ["codex: not refreshing, ...", "claude/perso: invalid_grant"]
        );

        let tmp = tempfile::tempdir().unwrap();
        let store = Store::open(tmp.path());
        assert!(last(&store).is_none());
        record(&store, &run);
        assert_eq!(last(&store), Some(run));

        std::fs::write(record_path(&store), "{ torn").unwrap();
        assert!(last(&store).is_none());
    }
}
