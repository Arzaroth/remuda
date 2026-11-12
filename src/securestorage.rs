use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Context, Result, bail};
use serde_json::Value;

use crate::fsx::{create_private_dir, read_json, update_json, write_private};
use crate::keychain::{Item, Keychain};

/// What Claude Code calls its secure storage: the JSON object holding its
/// login and its MCP servers' tokens. On macOS it is the login Keychain's
/// item when there is one, and the file otherwise.
pub struct SecureStorage {
    file: PathBuf,
    keychain: Option<Keychain>,
}

/// Claude Code's lock on its secure storage, a directory it takes for each
/// write and counts as abandoned after 15 seconds.
struct Held(PathBuf);

const STALE: Duration = Duration::from_secs(15);
const WAIT: Duration = Duration::from_secs(10);

impl Held {
    fn take(dir: &Path) -> Result<Held> {
        create_private_dir(dir).with_context(|| format!("failed to create {}", dir.display()))?;
        let path = dir.join(".storage-write.lock");
        let until = Instant::now() + WAIT;
        loop {
            match fs::create_dir(&path) {
                Ok(()) => return Ok(Held(path)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    let stale = fs::metadata(&path)
                        .and_then(|m| m.modified())
                        .is_ok_and(|at| {
                            SystemTime::now()
                                .duration_since(at)
                                .is_ok_and(|age| age > STALE)
                        });
                    if stale {
                        let _ = fs::remove_dir(&path);
                        continue;
                    }
                    if Instant::now() >= until {
                        bail!(
                            "Claude Code kept {} for more than {}s",
                            path.display(),
                            WAIT.as_secs()
                        );
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                Err(e) => {
                    return Err(e).with_context(|| format!("failed to create {}", path.display()));
                }
            }
        }
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        let _ = fs::remove_dir(&self.0);
    }
}

fn parse(keychain: &Keychain, text: &str) -> Result<Value> {
    serde_json::from_str(text)
        .with_context(|| format!("the Keychain's {} is not JSON", keychain.service()))
}

impl SecureStorage {
    pub fn in_file(file: PathBuf) -> Self {
        SecureStorage {
            file,
            keychain: None,
        }
    }

    pub fn with_keychain(file: PathBuf, keychain: Keychain) -> Self {
        SecureStorage {
            file,
            keychain: Some(keychain),
        }
    }

    pub fn file(&self) -> &Path {
        &self.file
    }

    pub fn read(&self) -> Result<Option<Value>> {
        let Some(keychain) = &self.keychain else {
            return read_json(&self.file);
        };
        match keychain.read()? {
            Item::Held(text) => parse(keychain, &text).map(Some),
            Item::Missing => read_json(&self.file),
        }
    }

    /// Edits the object where Claude Code reads it, under the same rules as
    /// [`crate::fsx::update_json`]. An item too long to pass `security`
    /// without a command line moves to the file, which Claude Code reads
    /// once the item is gone and moves back on its next write.
    pub fn update(&self, edit: impl Fn(&mut Value) -> Result<()>) -> Result<()> {
        let Some(keychain) = &self.keychain else {
            return update_json(&self.file, edit);
        };
        let dir = self.file.parent().context("path has no parent")?;
        let _held = Held::take(dir)?;
        for _ in 0..10 {
            let Item::Held(before) = keychain.read()? else {
                return update_json(&self.file, edit);
            };
            let mut value = parse(keychain, &before)?;
            if !value.is_object() {
                bail!("the Keychain's {} is not a JSON object", keychain.service());
            }
            edit(&mut value)?;
            match keychain.read()? {
                Item::Held(now) if now == before => {}
                _ => {
                    std::thread::sleep(Duration::from_millis(20));
                    continue;
                }
            }
            if keychain.write(&serde_json::to_string(&value)?)? {
                return Ok(());
            }
            let mut body = serde_json::to_string_pretty(&value)?;
            body.push('\n');
            write_private(&self.file, &body)?;
            return keychain.delete();
        }
        bail!(
            "the Keychain's {} kept changing while remuda wrote it",
            keychain.service()
        )
    }

    /// Removes what the storage keeps outside the directory.
    pub fn forget(&self) -> Result<()> {
        match &self.keychain {
            Some(keychain) => keychain.delete(),
            None => Ok(()),
        }
    }

    /// The storage Claude Code uses with `CLAUDE_CONFIG_DIR` set to `dir`.
    pub fn relocated(&self, dir: &Path) -> Self {
        SecureStorage {
            file: dir.join(".credentials.json"),
            keychain: self
                .keychain
                .as_ref()
                .map(|k| k.with_service(crate::keychain::service(Some(&dir.to_string_lossy())))),
        }
    }
}

