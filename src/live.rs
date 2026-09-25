use std::path::PathBuf;

use anyhow::{Context, Result};
use serde_json::{Map, Value, json};

use crate::claude::OAUTH_KEY;
use crate::fsx::{read_json, write_json};
use crate::paths;
use crate::store::Entry;

/// Claude Code's own files: the credentials it signs in with, and the config
/// holding the `oauthAccount` its UI shows.
pub struct Live {
    pub creds: PathBuf,
    pub config: PathBuf,
}

impl Live {
    pub fn from_env() -> Self {
        Live {
            creds: paths::claude_credentials(),
            config: paths::claude_config(),
        }
    }

    pub fn oauth(&self) -> Result<Option<Map<String, Value>>> {
        let Some(file) = read_json(&self.creds)? else {
            return Ok(None);
        };
        Ok(file
            .get(OAUTH_KEY)
            .and_then(Value::as_object)
            .filter(|o| {
                o.get("accessToken")
                    .and_then(Value::as_str)
                    .is_some_and(|t| !t.is_empty())
            })
            .cloned())
    }

    pub fn account(&self) -> Result<Option<Value>> {
        Ok(read_json(&self.config)?.and_then(|c| c.get("oauthAccount").cloned()))
    }

    /// Writes the account first and the login second: Claude Code adopts a
    /// login when the credentials file changes, and by then the account the
    /// UI shows is already the new one. Every other key in both files is kept.
    pub fn install(&self, entry: &Entry) -> Result<()> {
        let oauth = entry
            .oauth()
            .context("stored credential has no login")?
            .clone();
        let mut config = read_json(&self.config)?.unwrap_or_else(|| json!({}));
        config
            .as_object_mut()
            .with_context(|| format!("{} is not a JSON object", self.config.display()))?
            .insert("oauthAccount".into(), entry.meta.oauth_account.clone());
        write_json(&self.config, &config)?;

        let mut creds = read_json(&self.creds)?.unwrap_or_else(|| json!({}));
        creds
            .as_object_mut()
            .with_context(|| format!("{} is not a JSON object", self.creds.display()))?
            .insert(OAUTH_KEY.into(), Value::Object(oauth));
        write_json(&self.creds, &creds)
    }
}
