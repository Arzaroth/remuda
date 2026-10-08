use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::fsx::{read_record, write_record};
use crate::provider::Provider;
use crate::store::Store;

/// Where each CLI's login was found, kept in the store it was found with.
/// Interactive commands record it; the scheduled refresh, run by a systemd
/// user manager that does not see what a shell exports, checks it still looks
/// in the same places before refreshing anything.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Seen {
    pub homes: BTreeMap<String, PathBuf>,
}

fn record_path(store: &Store) -> PathBuf {
    store.root().join(".dirs.json")
}

pub fn now(providers: &[&dyn Provider]) -> Seen {
    Seen {
        homes: providers
            .iter()
            .map(|p| (p.id().to_owned(), p.home()))
            .collect(),
    }
}

pub fn recorded(store: &Store) -> Option<Seen> {
    read_record(&record_path(store))
}

pub fn record(store: &Store, seen: &Seen) {
    if recorded(store).as_ref() != Some(seen) {
        write_record(
            &record_path(store),
            seen,
            "where the CLIs keep their logins",
        );
    }
}

/// What a scheduled refresh may touch: the CLIs whose login it finds where
/// the last interactive command did, and a line for each one it may not.
pub struct Plan {
    pub usable: Vec<String>,
    pub skipped: Vec<String>,
}

pub fn plan(recorded: Option<&Seen>, now: &Seen, doing: &str) -> Plan {
    let mut plan = Plan {
        usable: Vec::new(),
        skipped: Vec::new(),
    };
    for (id, here) in &now.homes {
        match recorded.map(|r| r.homes.get(id)) {
            Some(Some(there)) if there == here => plan.usable.push(id.clone()),
            Some(Some(there)) => plan.skipped.push(format!(
                "{id}: not {doing}, its login was last found in {} and this run looks in {}; set its directory in ~/.config/environment.d/60-remuda.conf",
                there.display(),
                here.display()
            )),
            _ => plan.skipped.push(format!(
                "{id}: not {doing}, no interactive remuda command has recorded where its login lives in this store yet; run `remuda ls` once"
            )),
        }
    }
    plan
}

/// A unit run sees nothing of what the user's shell exports, so it is checked,
/// never recorded. The units say so themselves: INVOCATION_ID is no sign of
/// one, since a desktop that runs its terminal as a unit hands it to every
/// shell.
pub fn is_service(flag: bool) -> bool {
    flag || std::env::var_os("REMUDA_SERVICE").is_some_and(|v| !v.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seen(claude: &str) -> Seen {
        Seen {
            homes: [
                ("claude".to_owned(), PathBuf::from(claude)),
                ("codex".to_owned(), PathBuf::from("/h/.codex")),
            ]
            .into(),
        }
    }

    #[test]
    fn a_plan_skips_what_moved_and_everything_without_a_record() {
        let p = plan(Some(&seen("/h/.claude")), &seen("/h/.claude"), "refreshing");
        assert_eq!(p.usable, ["claude", "codex"]);
        assert!(p.skipped.is_empty());

        let p = plan(
            Some(&seen("/work/.claude")),
            &seen("/h/.claude"),
            "refreshing",
        );
        assert_eq!(p.usable, ["codex"]);
        assert!(p.skipped[0].contains("/work/.claude"), "{:?}", p.skipped);

        let p = plan(None, &seen("/h/.claude"), "refreshing");
        assert!(p.usable.is_empty());
        assert!(p.skipped.iter().all(|s| s.contains("run `remuda ls` once")));
    }

    #[test]
    fn a_record_is_rewritten_only_when_it_changes_and_never_fails_a_command() {
        use std::os::unix::fs::MetadataExt;
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::open(tmp.path());
        record(&store, &seen("/h/.claude"));
        let inode = |store: &Store| std::fs::metadata(record_path(store)).unwrap().ino();
        let first = inode(&store);
        record(&store, &seen("/h/.claude"));
        assert_eq!(inode(&store), first);
        record(&store, &seen("/w/.claude"));
        assert_ne!(inode(&store), first);
        assert_eq!(recorded(&store).unwrap(), seen("/w/.claude"));

        std::fs::write(record_path(&store), "{ torn").unwrap();
        assert!(recorded(&store).is_none());
        let blocked = Store::open(&tmp.path().join("file"));
        std::fs::write(tmp.path().join("file"), "").unwrap();
        record(&blocked, &seen("/h/.claude"));
    }
}
