use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::claude::OAUTH_KEY;
use crate::fsx::{read_json, write_json};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Meta {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub account_uuid: String,
    pub email: String,
    pub captured_at: i64,
    /// Restored into `.claude.json` on a switch.
    pub oauth_account: Value,
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub name: String,
    /// The same shape as Claude Code's `.credentials.json`, holding only the
    /// login and none of the MCP server tokens.
    pub creds: Value,
    pub meta: Meta,
}

impl Entry {
    pub fn new(name: &str, oauth: Value, oauth_account: Value, captured_at: i64) -> Result<Self> {
        let account_uuid = oauth_account
            .get("accountUuid")
            .and_then(Value::as_str)
            .context("login has no account uuid")?
            .to_owned();
        let email = oauth_account
            .get("emailAddress")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        Ok(Entry {
            name: name.to_owned(),
            creds: json!({ OAUTH_KEY: oauth }),
            meta: Meta {
                label: None,
                account_uuid,
                email,
                captured_at,
                oauth_account,
            },
        })
    }

    pub fn oauth(&self) -> Option<&Map<String, Value>> {
        self.creds.get(OAUTH_KEY).and_then(Value::as_object)
    }

    pub fn oauth_mut(&mut self) -> Option<&mut Map<String, Value>> {
        self.creds.get_mut(OAUTH_KEY).and_then(Value::as_object_mut)
    }

    pub fn token(&self, key: &str) -> Option<&str> {
        self.oauth()?.get(key)?.as_str()
    }

    pub fn millis(&self, key: &str) -> Option<i64> {
        self.oauth()?.get(key)?.as_i64()
    }

    pub fn plan(&self) -> String {
        let o = self.oauth();
        let get = |k| o.and_then(|o| o.get(k)).and_then(Value::as_str);
        match (get("subscriptionType"), get("rateLimitTier")) {
            (Some(sub), Some(tier)) if tier.ends_with("_20x") => format!("{sub} 20x"),
            (Some(sub), Some(tier)) if tier.ends_with("_5x") => format!("{sub} 5x"),
            (Some(sub), _) => sub.to_owned(),
            _ => "-".to_owned(),
        }
    }
}

pub struct Store {
    dir: PathBuf,
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

impl Store {
    pub fn open(root: &Path) -> Self {
        Store {
            dir: root.join("claude"),
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn creds_path(&self, name: &str) -> PathBuf {
        self.dir.join(format!("{name}.json"))
    }

    fn meta_path(&self, name: &str) -> PathBuf {
        self.dir.join(format!("{name}.meta.json"))
    }

    pub fn get(&self, name: &str) -> Result<Option<Entry>> {
        validate_name(name)?;
        let Some(creds) = read_json(&self.creds_path(name))? else {
            return Ok(None);
        };
        let meta = read_json(&self.meta_path(name))?
            .with_context(|| format!("{name} has no {name}.meta.json"))?;
        let meta = serde_json::from_value(meta)
            .with_context(|| format!("{name}.meta.json is malformed"))?;
        Ok(Some(Entry {
            name: name.to_owned(),
            creds,
            meta,
        }))
    }

    pub fn list(&self) -> Result<Vec<Entry>> {
        let read = match fs::read_dir(&self.dir) {
            Ok(r) => r,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => {
                return Err(e).with_context(|| format!("failed to read {}", self.dir.display()));
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
            .filter_map(|n| self.get(n).transpose())
            .collect()
    }

    pub fn save(&self, entry: &Entry) -> Result<()> {
        validate_name(&entry.name)?;
        write_json(
            &self.meta_path(&entry.name),
            &serde_json::to_value(&entry.meta)?,
        )?;
        write_json(&self.creds_path(&entry.name), &entry.creds)
    }

    pub fn remove(&self, name: &str) -> Result<()> {
        validate_name(name)?;
        fs::remove_file(self.creds_path(name))
            .with_context(|| format!("no credential named {name}"))?;
        let _ = fs::remove_file(self.meta_path(name));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, uuid: &str) -> Entry {
        Entry::new(
            name,
            json!({"accessToken": "a", "refreshToken": "r", "subscriptionType": "max", "rateLimitTier": "default_claude_max_20x"}),
            json!({"accountUuid": uuid, "emailAddress": format!("{name}@example.com")}),
            1,
        )
        .unwrap()
    }

    #[test]
    fn an_entry_round_trips_through_two_files() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::open(tmp.path());
        store.save(&entry("work", "u-1")).unwrap();
        store.save(&entry("perso", "u-2")).unwrap();
        let all = store.list().unwrap();
        assert_eq!(
            all.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
            ["perso", "work"]
        );
        let work = store.get("work").unwrap().unwrap();
        assert_eq!(work.meta.account_uuid, "u-1");
        assert_eq!(work.token("refreshToken"), Some("r"));
        assert_eq!(work.plan(), "max 20x");
        assert!(tmp.path().join("claude/work.meta.json").exists());
    }

    #[test]
    fn names_cannot_escape_the_store_or_shadow_a_sidecar() {
        for bad in ["", "../x", ".hidden", "a/b", "x.meta"] {
            assert!(validate_name(bad).is_err(), "{bad} accepted");
        }
        validate_name("work-2.max").unwrap();
    }
}
