use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::fsx::{read_record, write_record};
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
    read_record(&record_path(store))
}

pub fn record(store: &Store, run: &Run) {
    write_record(&record_path(store), run, "this refresh for the page");
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
