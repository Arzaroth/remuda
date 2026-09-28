use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::fsx::{read_json, write_json};
use crate::provider::Provider;
use crate::store::Store;

/// Where the store and each CLI's login were found. Interactive commands
/// record it; the scheduled refresh, run by a systemd user manager that does
/// not see what a shell exports, checks it still looks in the same places.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Seen {
    pub store: PathBuf,
    pub homes: BTreeMap<String, PathBuf>,
}

pub fn now(store: &Store, providers: &[&dyn Provider]) -> Seen {
    Seen {
        store: store.root().to_owned(),
        homes: providers
            .iter()
            .map(|p| (p.id().to_owned(), p.home()))
            .collect(),
    }
}

pub fn recorded(path: &std::path::Path) -> Result<Option<Seen>> {
    Ok(read_json(path)?.and_then(|v| serde_json::from_value(v).ok()))
}

pub fn record(path: &std::path::Path, seen: &Seen) -> Result<()> {
    if recorded(path)?.as_ref() != Some(seen) {
        write_json(path, &serde_json::to_value(seen)?)?;
    }
    Ok(())
}

pub struct Drift {
    /// The store the shell uses, when the scheduled run sees another.
    pub store: Option<PathBuf>,
    /// Per CLI: where the shell finds its login, and where this run does.
    pub homes: Vec<(String, PathBuf, PathBuf)>,
}

pub fn drift(recorded: &Seen, now: &Seen) -> Drift {
    Drift {
        store: (recorded.store != now.store).then(|| recorded.store.clone()),
        homes: now
            .homes
            .iter()
            .filter_map(|(id, here)| {
                let there = recorded.homes.get(id)?;
                (there != here).then(|| (id.clone(), there.clone(), here.clone()))
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seen(store: &str, claude: &str) -> Seen {
        Seen {
            store: store.into(),
            homes: [
                ("claude".to_owned(), PathBuf::from(claude)),
                ("codex".to_owned(), PathBuf::from("/h/.codex")),
            ]
            .into(),
        }
    }

    #[test]
    fn drift_names_what_moved_and_nothing_else() {
        let d = drift(&seen("/s", "/h/.claude"), &seen("/s", "/h/.claude"));
        assert!(d.store.is_none() && d.homes.is_empty());

        let d = drift(&seen("/s", "/work/.claude"), &seen("/t", "/h/.claude"));
        assert_eq!(d.store, Some(PathBuf::from("/s")));
        assert_eq!(
            d.homes,
            [(
                "claude".to_owned(),
                PathBuf::from("/work/.claude"),
                PathBuf::from("/h/.claude")
            )]
        );
    }

    #[test]
    fn a_record_is_rewritten_only_when_it_changes() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("dirs.json");
        record(&path, &seen("/s", "/h/.claude")).unwrap();
        let first = std::fs::metadata(&path).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        record(&path, &seen("/s", "/h/.claude")).unwrap();
        assert_eq!(std::fs::metadata(&path).unwrap().modified().unwrap(), first);
        record(&path, &seen("/s", "/w/.claude")).unwrap();
        assert_eq!(
            recorded(&path).unwrap().unwrap().homes["claude"],
            PathBuf::from("/w/.claude")
        );
    }
}
