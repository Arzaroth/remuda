use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::fsx::{now_ms, read_json, write_json};
use crate::pkce;

pub const DEFAULT_PORT: u16 = 8317;
const KEY_PREFIX: &str = "rmd-";

fn default_port() -> u16 {
    DEFAULT_PORT
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default)]
    pub keys: Vec<Key>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            enabled: false,
            port: DEFAULT_PORT,
            keys: Vec::new(),
        }
    }
}

/// An access key a client sends the proxy. Kept in clear, like the
/// credentials beside it, so the page can show and copy it again.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Key {
    pub id: String,
    pub name: String,
    pub key: String,
    pub created: i64,
}

/// The settings file, read again on every use so the page, the listener and
/// an editor never disagree.
pub struct File {
    path: PathBuf,
}

impl File {
    pub fn at(path: PathBuf) -> Self {
        File { path }
    }

    pub fn read(&self) -> Result<Settings> {
        match read_json(&self.path)? {
            Some(v) => serde_json::from_value(v)
                .with_context(|| format!("{} is not proxy settings", self.path.display())),
            None => Ok(Settings::default()),
        }
    }

    fn write(&self, settings: &Settings) -> Result<()> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)
                .with_context(|| format!("cannot create {}", dir.display()))?;
        }
        write_json(&self.path, &serde_json::to_value(settings)?)
    }

    pub fn edit(&self, change: impl FnOnce(&mut Settings) -> Result<()>) -> Result<Settings> {
        let mut settings = self.read()?;
        change(&mut settings)?;
        self.write(&settings)?;
        Ok(settings)
    }

    pub fn add_key(&self, name: &str) -> Result<Key> {
        let name = name.trim();
        if name.is_empty() {
            bail!("a key needs a name");
        }
        let key = Key {
            id: pkce::uuid()?,
            name: name.to_owned(),
            key: format!("{KEY_PREFIX}{}", pkce::random()?),
            created: now_ms(),
        };
        let added = key.clone();
        self.edit(|s| {
            if s.keys.iter().any(|k| k.name == added.name) {
                bail!("a key named {} exists already", added.name);
            }
            s.keys.push(added);
            Ok(())
        })?;
        Ok(key)
    }

    pub fn remove_key(&self, id: &str) -> Result<()> {
        self.edit(|s| {
            let before = s.keys.len();
            s.keys.retain(|k| k.id != id);
            if s.keys.len() == before {
                bail!("no such key");
            }
            Ok(())
        })
        .map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file() -> (tempfile::TempDir, File) {
        let tmp = tempfile::tempdir().unwrap();
        let f = File::at(tmp.path().join("remuda/proxy.json"));
        (tmp, f)
    }

    #[test]
    fn a_missing_file_is_off_on_the_default_port() {
        let (_tmp, f) = file();
        assert_eq!(f.read().unwrap(), Settings::default());
        assert_eq!(f.read().unwrap().port, 8317);
    }

    #[test]
    fn keys_are_added_named_and_removed() {
        let (_tmp, f) = file();
        let key = f.add_key(" laptop ").unwrap();
        assert!(key.key.starts_with("rmd-"));
        assert_eq!(key.name, "laptop");
        assert!(f.add_key("laptop").is_err());
        assert!(f.add_key("  ").is_err());
        assert_eq!(f.read().unwrap().keys, vec![key.clone()]);
        f.remove_key(&key.id).unwrap();
        assert!(f.read().unwrap().keys.is_empty());
        assert!(f.remove_key(&key.id).is_err());
    }

    #[test]
    fn the_file_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let (_tmp, f) = file();
        f.add_key("a").unwrap();
        let mode = std::fs::metadata(&f.path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn settings_written_by_hand_keep_their_defaults() {
        let (_tmp, f) = file();
        std::fs::create_dir_all(f.path.parent().unwrap()).unwrap();
        std::fs::write(&f.path, r#"{"enabled": true}"#).unwrap();
        let s = f.read().unwrap();
        assert!(s.enabled);
        assert_eq!(s.port, 8317);
        assert!(s.keys.is_empty());
    }
}
