use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::fsx::{read_json, write_json};
use crate::provider::Identity;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Meta {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub account_id: String,
    pub email: String,
    pub captured_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oauth_account: Option<Value>,
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub provider: String,
    pub name: String,
    pub creds: Value,
    pub meta: Meta,
}

impl Entry {
    pub fn new(
        provider: &str,
        name: &str,
        creds: Value,
        identity: Identity,
        captured_at: i64,
    ) -> Self {
        Entry {
            provider: provider.to_owned(),
            name: name.to_owned(),
            creds,
            meta: Meta {
                label: None,
                account_id: identity.account_id,
                email: identity.email,
                captured_at,
                oauth_account: identity.oauth_account,
            },
        }
    }

    pub fn qualified(&self) -> String {
        format!("{}/{}", self.provider, self.name)
    }
}

pub fn validate_name(name: &str) -> Result<()> {
    let ok = !name.is_empty()
        && !name.starts_with('.')
        && !name.ends_with(".meta")
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if !ok {
        bail!("{name:?} is not a usable name: letters, digits, '-', '_' and '.' only");
    }
    Ok(())
}

/// `<root>/<provider>/<name>.json` beside `<name>.meta.json`.
pub struct Store {
    root: PathBuf,
}

impl Store {
    pub fn open(root: &Path) -> Self {
        Store {
            root: root.to_owned(),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Held for the length of a command, never across a wait on the user, or
    /// the refresh timer stalls behind it.
    pub fn lock(&self) -> Result<fs::File> {
        crate::fsx::lock(&self.root)
    }

    fn creds_path(&self, provider: &str, name: &str) -> PathBuf {
        self.root.join(provider).join(format!("{name}.json"))
    }

    fn meta_path(&self, provider: &str, name: &str) -> PathBuf {
        self.root.join(provider).join(format!("{name}.meta.json"))
    }

    pub fn get(&self, provider: &str, name: &str) -> Result<Option<Entry>> {
        validate_name(name)?;
        let Some(creds) = read_json(&self.creds_path(provider, name))? else {
            return Ok(None);
        };
        let meta = read_json(&self.meta_path(provider, name))?
            .with_context(|| format!("{provider}/{name} has no {name}.meta.json"))?;
        let meta = serde_json::from_value(meta)
            .with_context(|| format!("{provider}/{name}.meta.json is malformed"))?;
        Ok(Some(Entry {
            provider: provider.to_owned(),
            name: name.to_owned(),
            creds,
            meta,
        }))
    }

    pub fn list(&self, provider: &str) -> Result<Vec<Entry>> {
        let dir = self.root.join(provider);
        let read = match fs::read_dir(&dir) {
            Ok(r) => r,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => {
                return Err(e).with_context(|| format!("failed to read {}", dir.display()));
            }
        };
        let mut names: Vec<String> = read
            .filter_map(|e| e.ok()?.file_name().into_string().ok())
            .filter(|f| !f.starts_with('.') && !f.ends_with(".meta.json"))
            .filter_map(|f| f.strip_suffix(".json").map(str::to_owned))
            .collect();
        names.sort();
        names
            .iter()
            .filter_map(|n| self.get(provider, n).transpose())
            .collect()
    }

    pub fn save(&self, entry: &Entry) -> Result<()> {
        validate_name(&entry.name)?;
        write_json(
            &self.meta_path(&entry.provider, &entry.name),
            &serde_json::to_value(&entry.meta)?,
        )?;
        write_json(&self.creds_path(&entry.provider, &entry.name), &entry.creds)
    }

    /// Keeps tokens nothing else would hold, in a file `list` does not show.
    pub fn set_aside(&self, provider: &str, account: &str, creds: &Value) -> Result<PathBuf> {
        let account: String = account
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let path = self.root.join(provider).join(format!(
            ".set-aside-{account}-{}.json",
            crate::fsx::now_ms()
        ));
        write_json(&path, creds)?;
        Ok(path)
    }

    pub fn rename(&self, provider: &str, from: &str, to: &str) -> Result<()> {
        validate_name(from)?;
        validate_name(to)?;
        if self.get(provider, to)?.is_some() {
            bail!("{provider}/{to} already exists");
        }
        fs::rename(
            self.creds_path(provider, from),
            self.creds_path(provider, to),
        )
        .with_context(|| format!("no credential named {provider}/{from}"))?;
        fs::rename(self.meta_path(provider, from), self.meta_path(provider, to))
            .with_context(|| format!("{provider}/{from} has no sidecar to move"))
    }

    pub fn remove(&self, provider: &str, name: &str) -> Result<()> {
        validate_name(name)?;
        fs::remove_file(self.creds_path(provider, name))
            .with_context(|| format!("no credential named {provider}/{name}"))?;
        let _ = fs::remove_file(self.meta_path(provider, name));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn entry(name: &str, account: &str) -> Entry {
        Entry::new(
            "claude",
            name,
            json!({"claudeAiOauth": {"accessToken": "a", "refreshToken": "r"}}),
            Identity {
                account_id: account.into(),
                email: format!("{name}@example.com"),
                oauth_account: Some(json!({"accountUuid": account})),
            },
            1,
        )
    }

    #[test]
    fn an_entry_round_trips_through_two_files_under_its_provider() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::open(tmp.path());
        store.save(&entry("work", "u-1")).unwrap();
        store.save(&entry("perso", "u-2")).unwrap();
        let all = store.list("claude").unwrap();
        assert_eq!(
            all.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
            ["perso", "work"]
        );
        let work = store.get("claude", "work").unwrap().unwrap();
        assert_eq!(work.meta.account_id, "u-1");
        assert_eq!(work.qualified(), "claude/work");
        assert!(tmp.path().join("claude/work.meta.json").exists());
        assert!(store.list("codex").unwrap().is_empty());
    }

    #[test]
    fn a_sidecar_without_a_config_block_leaves_it_out() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::open(tmp.path());
        let mut e = entry("work", "u-1");
        e.provider = "codex".into();
        e.meta.oauth_account = None;
        store.save(&e).unwrap();
        let meta = read_json(&tmp.path().join("codex/work.meta.json"))
            .unwrap()
            .unwrap();
        assert!(meta.get("oauthAccount").is_none());
        assert_eq!(meta["accountId"], "u-1");
    }

    #[test]
    fn names_cannot_escape_the_store_or_shadow_a_sidecar() {
        for bad in ["", "../x", ".hidden", "a/b", "x.meta"] {
            assert!(validate_name(bad).is_err(), "{bad} accepted");
        }
        validate_name("work-2.max").unwrap();
    }

    #[test]
    fn renaming_moves_both_files_and_never_overwrites() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::open(tmp.path());
        store.save(&entry("work", "u-1")).unwrap();
        store.save(&entry("perso", "u-2")).unwrap();

        let err = store.rename("claude", "work", "perso").unwrap_err();
        assert!(err.to_string().contains("already exists"), "{err}");
        assert!(store.rename("claude", "nope", "other").is_err());

        store.rename("claude", "work", "job").unwrap();
        assert!(store.get("claude", "work").unwrap().is_none());
        assert_eq!(
            store.get("claude", "job").unwrap().unwrap().meta.account_id,
            "u-1"
        );
    }

    #[test]
    fn removing_takes_the_sidecar_with_it() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::open(tmp.path());
        store.save(&entry("work", "u-1")).unwrap();
        store.remove("claude", "work").unwrap();
        assert!(!tmp.path().join("claude/work.meta.json").exists());
        assert!(store.remove("claude", "work").is_err());
    }
}
