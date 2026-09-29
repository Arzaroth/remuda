use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::commands::{Failed, Report};
use crate::fsx::{read_record, write_record};
use crate::store::Store;

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

impl Run {
    pub fn of(at: i64, skipped: &[String], report: Report, outcome: &Result<()>) -> Run {
        let mut problems = skipped.to_vec();
        problems.extend(report.problems);
        if let Err(e) = outcome
            && e.downcast_ref::<Failed>().is_none()
        {
            problems.push(format!("{e:#}"));
        }
        Run {
            at,
            refreshed: report.refreshed,
            problems,
        }
    }
}

pub fn last(store: &Store) -> Option<Run> {
    read_record(&record_path(store))
}

pub fn record(store: &Store, run: &Run) {
    write_record(&record_path(store), run, "this refresh for the page");
}

fn switches_path(store: &Store) -> PathBuf {
    store.root().join(".last-switch.json")
}

pub fn switches(store: &Store) -> BTreeMap<String, i64> {
    read_record(&switches_path(store)).unwrap_or_default()
}

pub fn switched(store: &Store, provider: &str, at: i64) {
    let mut all = switches(store);
    all.insert(provider.to_owned(), at);
    write_record(&switches_path(store), &all, "when this switch happened");
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::anyhow;

    fn report() -> Report {
        Report {
            refreshed: vec!["claude/work".to_owned()],
            problems: vec!["claude/perso: invalid_grant".to_owned()],
        }
    }

    #[test]
    fn a_run_keeps_what_it_refreshed_and_everything_that_went_wrong() {
        let skipped = ["codex: not refreshing, ...".to_owned()];
        let run = Run::of(42, &skipped, report(), &Err(Failed(1).into()));
        assert_eq!(run.refreshed, ["claude/work"]);
        assert_eq!(
            run.problems,
            ["codex: not refreshing, ...", "claude/perso: invalid_grant"]
        );

        let run = Run::of(
            42,
            &[],
            Report::default(),
            &Err(anyhow!("store unreadable")),
        );
        assert_eq!(run.problems, ["store unreadable"]);

        let tmp = tempfile::tempdir().unwrap();
        let store = Store::open(tmp.path());
        assert!(last(&store).is_none());
        let run = Run::of(42, &skipped, report(), &Ok(()));
        record(&store, &run);
        assert_eq!(last(&store), Some(run));

        std::fs::write(record_path(&store), "{ torn").unwrap();
        assert!(last(&store).is_none());
    }
}
