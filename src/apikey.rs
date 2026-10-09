use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::fsx::{Changed, read_json, update_json};
use crate::paths;
use crate::provider::{Identity, Login, PendingLogin, Provider};
use crate::store::Entry;

/// Where a CLI that takes an API key finds it, besides its environment: one
/// entry of a JSON file other entries share.
pub struct KeyFile {
    pub path: PathBuf,
    pub entry: &'static str,
}

/// A provider signed into with an API key. A key has no account behind it to
/// confirm, never expires and never refreshes, so it is filed under a digest
/// of itself: two copies of one key are one credential, and two keys are two.
pub struct ApiKey {
    id: &'static str,
    name: &'static str,
    /// The variables the CLI reads, in its order. The first is the one
    /// `remuda env` sets.
    envs: &'static [&'static str],
    key_page: &'static str,
    file: Option<KeyFile>,
    env: Box<Env>,
}

type Env = dyn Fn(&str) -> Option<String> + Send + Sync;

fn process_env(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

fn key_of(creds: &Value) -> Option<&str> {
    creds.get("key")?.as_str().filter(|k| !k.trim().is_empty())
}

fn identity_of(key: &str) -> Identity {
    let digest: String = Sha256::digest(key.trim().as_bytes())
        .iter()
        .take(8)
        .map(|b| format!("{b:02x}"))
        .collect();
    let tail: String = key
        .trim()
        .chars()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    Identity {
        account_id: format!("key-{digest}"),
        email: format!("key ending {tail}"),
        oauth_account: None,
    }
}

impl ApiKey {
    pub fn glm() -> Self {
        ApiKey {
            id: "glm",
            name: "GLM",
            envs: &["Z_AI_API_KEY", "ZAI_API_TOKEN"],
            key_page: "https://z.ai/manage-apikey/apikey-list",
            file: None,
            env: Box::new(process_env),
        }
    }

    pub fn opencode() -> Self {
        ApiKey {
            id: "opencode",
            name: "opencode Go",
            envs: &["OPENCODE_API_KEY", "OPENCODE_GO_API_KEY"],
            key_page: "https://opencode.ai/auth",
            file: Some(KeyFile {
                path: paths::opencode_auth(),
                entry: "opencode-go",
            }),
            env: Box::new(process_env),
        }
    }

    #[cfg(test)]
    pub fn with(
        mut self,
        file: Option<KeyFile>,
        env: impl Fn(&str) -> Option<String> + Send + Sync + 'static,
    ) -> Self {
        self.file = file;
        self.env = Box::new(env);
        self
    }

    /// The first variable that holds a key, and the key.
    fn env_key(&self) -> Option<(&'static str, String)> {
        self.envs.iter().find_map(|name| {
            let key = (self.env)(name)?.trim().to_owned();
            (!key.is_empty()).then_some((*name, key))
        })
    }

    fn file_key(&self) -> Result<Option<String>> {
        let Some(file) = &self.file else {
            return Ok(None);
        };
        let Some(json) = read_json(&file.path)? else {
            return Ok(None);
        };
        Ok(json
            .get(file.entry)
            .and_then(key_of)
            .map(|k| k.trim().to_owned()))
    }
}

impl Provider for ApiKey {
    fn id(&self) -> &'static str {
        self.id
    }

    fn name(&self) -> &'static str {
        self.name
    }

    fn home(&self) -> PathBuf {
        self.file
            .as_ref()
            .and_then(|f| f.path.parent().map(PathBuf::from))
            .unwrap_or_default()
    }

    fn access_token<'a>(&self, creds: &'a Value) -> Option<&'a str> {
        key_of(creds)
    }

    fn refresh_token<'a>(&self, _creds: &'a Value) -> Option<&'a str> {
        None
    }

    fn expires_at(&self, _creds: &Value) -> Option<i64> {
        None
    }

    fn renews(&self) -> bool {
        false
    }

    fn plan(&self, _creds: &Value) -> String {
        "-".to_owned()
    }

    /// The environment wins, as it does for the CLI.
    fn live(&self) -> Result<Option<Value>> {
        let key = match self.env_key() {
            Some((_, key)) => Some(key),
            None => self.file_key()?,
        };
        Ok(key.map(|key| json!({ "key": key })))
    }

    /// A login in the key's slot that is not a key, which a switch would
    /// replace.
    fn foreign_login(&self) -> Result<Option<String>> {
        if self.env_key().is_some() {
            return Ok(None);
        }
        let Some(file) = &self.file else {
            return Ok(None);
        };
        let Some(json) = read_json(&file.path)? else {
            return Ok(None);
        };
        Ok(json
            .get(file.entry)
            .filter(|e| key_of(e).is_none())
            .map(|e| match e.get("type").and_then(Value::as_str) {
                Some(kind) => format!("a login of type {kind}"),
                None => "a login that is not a key".to_owned(),
            }))
    }

    fn live_identity(&self, creds: &Value) -> Result<Option<Identity>> {
        Ok(key_of(creds).map(identity_of))
    }

    fn identify(&self, creds: &Value) -> Result<Identity> {
        key_of(creds)
            .map(identity_of)
            .context("credential holds no key")
    }

    fn env_line(&self, entry: &Entry) -> Result<String> {
        let key = key_of(&entry.creds).context("stored credential holds no key")?;
        Ok(format!(
            "export {}='{}'",
            self.envs[0],
            key.trim().replace('\'', r"'\''")
        ))
    }

    /// Writes the key into the CLI's file. A key the environment sets wins
    /// over the file, and no write can change that.
    fn install(&self, entry: &Entry, outgoing: Option<&Value>) -> Result<()> {
        let key = key_of(&entry.creds).context("stored credential holds no key")?;
        let qualified = entry.qualified();
        if let Some((var, _)) = self.env_key() {
            bail!(
                "{} reads its key from {var}, which remuda cannot change in a running shell: run `eval \"$(remuda env {qualified})\"`",
                self.name
            );
        }
        let Some(file) = &self.file else {
            bail!(
                "{} reads its key from {}: run `eval \"$(remuda env {qualified})\"`",
                self.name,
                self.envs[0]
            );
        };
        update_json(&file.path, |json| {
            if !json.is_object() {
                *json = json!({});
            }
            let current = json.get(file.entry).and_then(key_of).map(str::trim);
            if outgoing.is_some_and(|o| key_of(o).map(str::trim) != current) {
                return Err(Changed.into());
            }
            match json.get_mut(file.entry).and_then(Value::as_object_mut) {
                Some(slot) if slot.get("type").and_then(Value::as_str) == Some("api") => {
                    slot.insert("key".into(), json!(key.trim()));
                }
                _ => json[file.entry] = json!({ "type": "api", "key": key.trim() }),
            }
            Ok(())
        })
    }

    fn refresh(&self, _creds: &mut Value) -> Result<Option<String>> {
        bail!("an API key does not refresh")
    }

    fn begin_login(&self) -> Result<Box<dyn PendingLogin>> {
        Ok(Box::new(KeyPending {
            url: self.key_page.to_owned(),
        }))
    }
}

/// A sign-in that is a key pasted from the provider's key page.
struct KeyPending {
    url: String,
}

impl PendingLogin for KeyPending {
    fn url(&self) -> &str {
        &self.url
    }

    fn needs_code(&self) -> bool {
        true
    }

    fn asks_for(&self) -> &'static str {
        "the API key"
    }

    fn finish(self: Box<Self>, code: Option<&str>) -> Result<Login> {
        let key = code
            .map(str::trim)
            .filter(|k| !k.is_empty())
            .context("no key was pasted")?;
        Ok(Login {
            creds: json!({ "key": key }),
            identity: identity_of(key),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fsx::write_json;
    use crate::ops::{self, LiveState};
    use crate::store::Store;

    fn opencode(dir: &std::path::Path, env: Option<&'static str>) -> ApiKey {
        ApiKey::opencode().with(
            Some(KeyFile {
                path: dir.join("auth.json"),
                entry: "opencode-go",
            }),
            move |name| {
                (name == "OPENCODE_API_KEY")
                    .then(|| env.map(str::to_owned))
                    .flatten()
            },
        )
    }

    fn save(store: &Store, p: &ApiKey, name: &str, key: &str) {
        let login = Box::new(KeyPending { url: String::new() })
            .finish(Some(key))
            .unwrap();
        store
            .save(&Entry::new(p.id(), name, login.creds, login.identity, 1))
            .unwrap();
    }

    #[test]
    fn a_key_is_filed_under_a_digest_of_itself() {
        let a = identity_of("sk-aaaa1234");
        assert_eq!(a, identity_of(" sk-aaaa1234\n"));
        assert_ne!(a.account_id, identity_of("sk-bbbb1234").account_id);
        assert!(a.account_id.starts_with("key-"));
        assert!(!a.account_id.contains("aaaa"));
        assert_eq!(a.email, "key ending 1234");
    }

    #[test]
    fn a_pasted_key_is_the_login() {
        let login = Box::new(KeyPending { url: "u".into() })
            .finish(Some("  sk-1  "))
            .unwrap();
        assert_eq!(login.creds, json!({"key": "sk-1"}));
        assert!(
            Box::new(KeyPending { url: "u".into() })
                .finish(Some(" "))
                .is_err()
        );
    }

    #[test]
    fn switching_opencode_rewrites_its_entry_and_nothing_else() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::open(&tmp.path().join("store"));
        let p = opencode(tmp.path(), None);
        write_json(
            &tmp.path().join("auth.json"),
            &json!({
                "openrouter": {"type": "api", "key": "or-1"},
                "opencode-go": {"type": "api", "key": "go-work"},
            }),
        )
        .unwrap();
        save(&store, &p, "work", "go-work");
        save(&store, &p, "perso", "go-perso");

        assert_eq!(
            ops::sync_live(&store, &p).unwrap(),
            LiveState::Stored {
                name: "work".into(),
                synced: true
            }
        );
        ops::switch(&store, &p, "perso", false).unwrap();

        let file = read_json(&tmp.path().join("auth.json")).unwrap().unwrap();
        assert_eq!(
            file["opencode-go"],
            json!({"type": "api", "key": "go-perso"})
        );
        assert_eq!(file["openrouter"], json!({"type": "api", "key": "or-1"}));
    }

    #[test]
    fn a_key_in_the_environment_cannot_be_switched_by_a_file() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::open(&tmp.path().join("store"));
        let p = opencode(tmp.path(), Some("go-work"));
        save(&store, &p, "work", "go-work");
        save(&store, &p, "perso", "go-perso");
        let err = ops::switch(&store, &p, "perso", false).unwrap_err();
        assert!(
            format!("{err:#}").contains("eval \"$(remuda env opencode/perso)\""),
            "{err:#}"
        );
        assert!(!tmp.path().join("auth.json").exists());
    }

    #[test]
    fn a_login_in_the_keys_slot_that_is_not_a_key_is_not_replaced_silently() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::open(&tmp.path().join("store"));
        let p = opencode(tmp.path(), None);
        write_json(
            &tmp.path().join("auth.json"),
            &json!({"opencode-go": {"type": "oauth", "access": "t"}}),
        )
        .unwrap();
        save(&store, &p, "work", "go-work");
        let err = ops::switch(&store, &p, "work", false).unwrap_err();
        assert!(err.to_string().contains("a login of type oauth"), "{err}");
        ops::switch(&store, &p, "work", true).unwrap();
        let file = read_json(&tmp.path().join("auth.json")).unwrap().unwrap();
        assert_eq!(
            file["opencode-go"],
            json!({"type": "api", "key": "go-work"})
        );
    }

    #[test]
    fn a_switch_edits_the_key_and_keeps_the_entrys_other_fields() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::open(&tmp.path().join("store"));
        let p = opencode(tmp.path(), None);
        write_json(
            &tmp.path().join("auth.json"),
            &json!({"opencode-go": {"type": "api", "key": "go-work", "note": "x"}}),
        )
        .unwrap();
        save(&store, &p, "work", "go-work");
        save(&store, &p, "perso", "go-perso");
        ops::switch(&store, &p, "perso", false).unwrap();
        let file = read_json(&tmp.path().join("auth.json")).unwrap().unwrap();
        assert_eq!(
            file["opencode-go"],
            json!({"type": "api", "key": "go-perso", "note": "x"})
        );
    }

    #[test]
    fn an_empty_variable_is_no_key() {
        let tmp = tempfile::tempdir().unwrap();
        let p = ApiKey::opencode().with(
            Some(KeyFile {
                path: tmp.path().join("auth.json"),
                entry: "opencode-go",
            }),
            |name| match name {
                "OPENCODE_API_KEY" => Some(" ".into()),
                "OPENCODE_GO_API_KEY" => Some("k1".into()),
                _ => None,
            },
        );
        assert_eq!(p.live().unwrap(), Some(json!({"key": "k1"})));
    }

    #[test]
    fn glm_is_switched_by_the_shell() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::open(&tmp.path().join("store"));
        let p = ApiKey::glm().with(None, |_| None);
        save(&store, &p, "work", "zk-it's");
        let work = store.get("glm", "work").unwrap().unwrap();
        assert_eq!(
            p.env_line(&work).unwrap(),
            r"export Z_AI_API_KEY='zk-it'\''s'"
        );
        assert!(p.install(&work, None).is_err());
        assert_eq!(p.live().unwrap(), None);
        assert!(!p.renews());
    }
}
