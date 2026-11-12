use std::path::{Path, PathBuf};

use anyhow::Result;
use serde_json::Value;

use crate::fsx::{read_json, update_json};

/// What Claude Code calls its secure storage: the JSON object holding its
/// login and its MCP servers' tokens.
pub struct SecureStorage {
    file: PathBuf,
}

impl SecureStorage {
    pub fn in_file(file: PathBuf) -> Self {
        SecureStorage { file }
    }

    pub fn file(&self) -> &Path {
        &self.file
    }

    pub fn read(&self) -> Result<Option<Value>> {
        read_json(&self.file)
    }

    /// Edits the object in place, under the same rules as
    /// [`crate::fsx::update_json`].
    pub fn update(&self, edit: impl Fn(&mut Value) -> Result<()>) -> Result<()> {
        update_json(&self.file, edit)
    }

    pub fn relocated(&self, dir: &Path) -> Self {
        SecureStorage::in_file(dir.join(".credentials.json"))
    }
}
