use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use rusqlite::{Connection, OpenFlags, OptionalExtension, TransactionBehavior};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::fsx::{Changed, read_json, update_json};
use crate::paths;
use crate::provider::{Format, Identity, Login, PendingLogin, Provider, Upstream};
use crate::store::Entry;

/// Where a CLI that takes an API key finds it, besides its environment: one
/// entry of a JSON file other entries share.
pub struct KeyFile {
    pub path: PathBuf,
    pub entry: &'static str,
}

/// opencode 2's `credential` table. Once opencode has imported `auth.json`
/// into it, opencode reads its logins from here and never from the file.
pub struct KeyDb {
    pub path: PathBuf,
    pub integration: &'static str,
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
    db: Option<KeyDb>,
    env: Box<Env>,
    inference: String,
    routes: Routes,
}

/// The paths under `inference` that take each format, for a model.
type Routes = fn(&str) -> &'static [(Format, &'static str)];

/// Z.ai's coding plan, whose quota only these paths draw on.
fn glm_routes(_model: &str) -> &'static [(Format, &'static str)] {
    &[
        (Format::Messages, "/anthropic/v1/messages"),
        (Format::Chat, "/coding/paas/v4/chat/completions"),
    ]
}

/// opencode Go serves each model family in its own maker's format; Gemini's
/// is Google's, which the proxy does not speak.
fn opencode_routes(model: &str) -> &'static [(Format, &'static str)] {
    if model.starts_with("gemini-") {
        &[]
    } else if model.starts_with("claude-") {
        &[(Format::Messages, "/anthropic/v1/messages")]
    } else if model.starts_with("gpt-") || model.starts_with("grok-") {
        &[(Format::Responses, "/openai/v1/responses")]
    } else {
        &[(Format::Chat, "/openai/v1/chat/completions")]
    }
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
            db: None,
            env: Box::new(process_env),
            inference: "https://api.z.ai/api".to_owned(),
            routes: glm_routes,
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
            db: Some(KeyDb {
                path: paths::opencode_db(),
                integration: "opencode-go",
            }),
            env: Box::new(process_env),
            inference: "https://opencode.ai/inference/go".to_owned(),
            routes: opencode_routes,
        }
    }

    /// The database moves with the file: a test's file has its database
    /// beside it.
    #[cfg(test)]
    pub fn with(
        mut self,
        file: Option<KeyFile>,
        env: impl Fn(&str) -> Option<String> + Send + Sync + 'static,
    ) -> Self {
        self.db = self.db.take().and_then(|db| {
            Some(KeyDb {
                path: file.as_ref()?.path.with_file_name("opencode.db"),
                integration: db.integration,
            })
        });
        self.file = file;
        self.env = Box::new(env);
        self
    }

    #[cfg(test)]
    pub fn inferring_at(mut self, base: &str) -> Self {
        self.inference = base.to_owned();
        self
    }

    /// The first variable that holds a key, and the key.
    fn env_key(&self) -> Option<(&'static str, String)> {
        self.envs.iter().find_map(|name| {
            let key = (self.env)(name)?.trim().to_owned();
            (!key.is_empty()).then_some((*name, key))
        })
    }

    /// opencode's database, when it is where opencode reads its logins.
    fn database(&self, flags: OpenFlags) -> Result<Option<(&KeyDb, Connection)>> {
        let Some(db) = &self.db else {
            return Ok(None);
        };
        Ok(open_db(&db.path, flags)?.map(|conn| (db, conn)))
    }

    /// The CLI's login: from the database once opencode reads it, else from
    /// the file.
    fn stored_login(&self) -> Result<Option<Value>> {
        if let Some((db, conn)) = self.database(OpenFlags::SQLITE_OPEN_READ_ONLY)? {
            return Ok(selected(&conn, db.integration)?.map(|(_, value)| value));
        }
        let Some(file) = &self.file else {
            return Ok(None);
        };
        Ok(read_json(&file.path)?.and_then(|mut json| json.get_mut(file.entry).map(Value::take)))
    }

    fn stored_key(&self) -> Result<Option<String>> {
        Ok(self
            .stored_login()?
            .as_ref()
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

    /// The database is written by every session, so its date says nothing
    /// about the login.
    fn live_file(&self) -> Option<PathBuf> {
        if self.env_key().is_some()
            || !matches!(self.database(OpenFlags::SQLITE_OPEN_READ_ONLY), Ok(None))
        {
            return None;
        }
        self.file.as_ref().map(|f| f.path.clone())
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
            None => self.stored_key()?,
        };
        Ok(key.map(|key| json!({ "key": key })))
    }

    /// A login in the key's slot that is not a key, which a switch would
    /// replace.
    fn foreign_login(&self) -> Result<Option<String>> {
        if self.env_key().is_some() {
            return Ok(None);
        }
        Ok(self
            .stored_login()?
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
        if let Some((db, mut conn)) = self.database(OpenFlags::SQLITE_OPEN_READ_WRITE)? {
            return install_row(&mut conn, db.integration, key.trim(), outgoing);
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

    fn formats(&self, model: &str) -> Vec<Format> {
        (self.routes)(model).iter().map(|(f, _)| *f).collect()
    }

    fn upstream(&self, creds: &Value, format: Format, model: &str) -> Option<Upstream> {
        let key = key_of(creds)?.trim();
        let (_, path) = (self.routes)(model).iter().find(|(f, _)| *f == format)?;
        let mut headers = vec![("authorization", format!("Bearer {key}"))];
        if format == Format::Messages {
            headers.push(("x-api-key", key.to_owned()));
        }
        Some(Upstream {
            url: format!("{}{path}", self.inference),
            headers,
        })
    }
}

/// The migration after which opencode 2 stops reading `auth.json`.
const IMPORTED_AUTH_JSON: &str = "%_import_legacy_credentials";

/// opencode's database, or None when there is none or opencode has not yet
/// imported `auth.json` into it, which leaves the file the one it reads.
fn open_db(path: &std::path::Path, flags: OpenFlags) -> Result<Option<Connection>> {
    if !path.exists() {
        return Ok(None);
    }
    let read = || -> rusqlite::Result<Option<Connection>> {
        let conn = Connection::open_with_flags(path, flags | OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
        conn.busy_timeout(Duration::from_secs(2))?;
        let migrated: bool = conn.query_row(
            "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'migration')",
            [],
            |row| row.get(0),
        )?;
        let imported = migrated
            && conn.query_row(
                "SELECT EXISTS (SELECT 1 FROM migration WHERE id LIKE ?1)",
                [IMPORTED_AUTH_JSON],
                |row| row.get(0),
            )?;
        Ok(imported.then_some(conn))
    };
    read().with_context(|| format!("could not read {}", path.display()))
}

/// The credential opencode uses for `integration`: the active one, else the
/// newest, as opencode orders them.
fn selected(conn: &Connection, integration: &str) -> Result<Option<(String, Value)>> {
    let row = conn
        .query_row(
            "SELECT id, value FROM credential WHERE integration_id = ?1 \
             ORDER BY active DESC, time_created DESC, id DESC LIMIT 1",
            [integration],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?;
    Ok(row.map(|(id, value)| (id, serde_json::from_str(&value).unwrap_or(Value::Null))))
}

/// Makes `key` the credential opencode uses, as opencode's own switch would:
/// a row already holding it is activated, the key row in use is rekeyed, and
/// otherwise a new row is added in front of a login that is not a key, which
/// stays behind it.
fn install_row(
    conn: &mut Connection,
    integration: &str,
    key: &str,
    outgoing: Option<&Value>,
) -> Result<()> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let current = selected(&tx, integration)?;
    let current_key = current.as_ref().and_then(|(_, v)| key_of(v)).map(str::trim);
    if outgoing.is_some_and(|o| key_of(o).map(str::trim) != current_key) {
        return Err(Changed.into());
    }
    let holder = {
        let mut rows = tx.prepare("SELECT id, value FROM credential WHERE integration_id = ?1")?;
        let rows = rows.query_map([integration], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut holder = None;
        for row in rows {
            let (id, value) = row?;
            let value: Value = serde_json::from_str(&value).unwrap_or(Value::Null);
            if key_of(&value).map(str::trim) == Some(key) {
                holder = Some(id);
                break;
            }
        }
        holder
    };
    let now = jiff::Timestamp::now().as_millisecond();
    match (holder, current) {
        (Some(id), current) if current.as_ref().is_some_and(|(c, _)| *c == id) => {}
        (Some(id), _) => {
            tx.execute(
                "UPDATE credential SET active = 0 WHERE integration_id = ?1",
                [integration],
            )?;
            tx.execute("UPDATE credential SET active = 1 WHERE id = ?1", [&id])?;
        }
        (None, Some((id, mut value))) if key_of(&value).is_some() => {
            value["key"] = json!(key);
            tx.execute(
                "UPDATE credential SET value = ?1, time_updated = ?2 WHERE id = ?3",
                rusqlite::params![value.to_string(), now, id],
            )?;
        }
        (None, _) => {
            tx.execute(
                "UPDATE credential SET active = 0 WHERE integration_id = ?1",
                [integration],
            )?;
            tx.execute(
                "INSERT INTO credential (id, integration_id, label, value, active, time_created, time_updated) \
                 VALUES (?1, ?2, 'API key', ?3, 1, ?4, ?4)",
                rusqlite::params![
                    credential_id(now)?,
                    integration,
                    json!({"type": "key", "key": key}).to_string(),
                    now
                ],
            )?;
        }
    }
    tx.commit()?;
    Ok(())
}

/// An id shaped like opencode's own: `cred_`, the time in hex, then base62.
fn credential_id(now_ms: i64) -> Result<String> {
    const BASE62: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
    let mut random = [0u8; 14];
    getrandom::fill(&mut random).map_err(|e| anyhow::anyhow!("no randomness: {e}"))?;
    let tail: String = random
        .iter()
        .map(|b| BASE62[usize::from(*b) % BASE62.len()] as char)
        .collect();
    Ok(format!(
        "cred_{:012x}{tail}",
        ((now_ms as u64) << 12) & 0xffff_ffff_ffff
    ))
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
    fn a_key_from_the_environment_is_not_dated_by_the_file() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(
            opencode(tmp.path(), None).live_file(),
            Some(tmp.path().join("auth.json"))
        );
        assert_eq!(opencode(tmp.path(), Some("sk-env")).live_file(), None);
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

    /// opencode 2's database, past the import of `auth.json` or not yet.
    fn database(dir: &std::path::Path, imported: bool) -> Connection {
        let conn = Connection::open(dir.join("opencode.db")).unwrap();
        conn.execute_batch(
            "CREATE TABLE migration (id TEXT PRIMARY KEY, time_completed INTEGER NOT NULL);
             INSERT INTO migration VALUES ('20260611192811_lush_chimera', 1);
             CREATE TABLE credential (id text PRIMARY KEY, integration_id text, label text NOT NULL,
               value text NOT NULL, connector_id text, method_id text, active integer,
               time_created integer NOT NULL, time_updated integer NOT NULL);",
        )
        .unwrap();
        if imported {
            conn.execute(
                "INSERT INTO migration VALUES ('20260805200742_import_legacy_credentials', 1)",
                [],
            )
            .unwrap();
        }
        conn
    }

    fn row(conn: &Connection, id: &str, value: Value, active: Option<bool>, created: i64) {
        conn.execute(
            "INSERT INTO credential (id, integration_id, label, value, active, time_created, time_updated) \
             VALUES (?1, 'opencode-go', 'API key', ?2, ?3, ?4, ?4)",
            rusqlite::params![id, value.to_string(), active, created],
        )
        .unwrap();
    }

    fn rows(conn: &Connection) -> Vec<(String, Value, Option<bool>)> {
        let mut statement = conn
            .prepare("SELECT id, value, active FROM credential ORDER BY time_created, id")
            .unwrap();
        statement
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    serde_json::from_str(&r.get::<_, String>(1)?).unwrap(),
                    r.get(2)?,
                ))
            })
            .unwrap()
            .map(Result::unwrap)
            .collect()
    }

    #[test]
    fn opencode_2_is_switched_in_its_database_and_auth_json_is_left_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::open(&tmp.path().join("store"));
        let p = opencode(tmp.path(), None);
        let db = database(tmp.path(), true);
        let auth = json!({"opencode-go": {"type": "api", "key": "go-old"}});
        write_json(&tmp.path().join("auth.json"), &auth).unwrap();
        row(
            &db,
            "cred_1",
            json!({"type": "key", "key": "go-work", "metadata": {"x": "y"}}),
            Some(true),
            1,
        );
        save(&store, &p, "work", "go-work");
        save(&store, &p, "perso", "go-perso");

        assert_eq!(p.live().unwrap(), Some(json!({"key": "go-work"})));
        assert_eq!(p.live_file(), None);
        assert_eq!(
            ops::sync_live(&store, &p).unwrap(),
            LiveState::Stored {
                name: "work".into(),
                synced: true
            }
        );
        ops::switch(&store, &p, "perso", false).unwrap();

        assert_eq!(
            rows(&db),
            vec![(
                "cred_1".to_owned(),
                json!({"type": "key", "key": "go-perso", "metadata": {"x": "y"}}),
                Some(true)
            )]
        );
        assert_eq!(
            read_json(&tmp.path().join("auth.json")).unwrap(),
            Some(auth)
        );
    }

    #[test]
    fn opencode_uses_the_active_credential_else_the_newest() {
        let tmp = tempfile::tempdir().unwrap();
        let p = opencode(tmp.path(), None);
        let db = database(tmp.path(), true);
        row(
            &db,
            "cred_a",
            json!({"type": "key", "key": "k-old"}),
            None,
            1,
        );
        row(
            &db,
            "cred_b",
            json!({"type": "key", "key": "k-new"}),
            None,
            2,
        );
        assert_eq!(p.live().unwrap(), Some(json!({"key": "k-new"})));
        db.execute("UPDATE credential SET active = 1 WHERE id = 'cred_a'", [])
            .unwrap();
        assert_eq!(p.live().unwrap(), Some(json!({"key": "k-old"})));
    }

    #[test]
    fn a_key_opencode_already_holds_is_activated_rather_than_added() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::open(&tmp.path().join("store"));
        let p = opencode(tmp.path(), None);
        let db = database(tmp.path(), true);
        row(
            &db,
            "cred_w",
            json!({"type": "key", "key": "go-work"}),
            Some(true),
            1,
        );
        row(
            &db,
            "cred_p",
            json!({"type": "key", "key": "go-perso"}),
            Some(false),
            2,
        );
        save(&store, &p, "work", "go-work");
        save(&store, &p, "perso", "go-perso");

        ops::switch(&store, &p, "perso", false).unwrap();

        assert_eq!(
            rows(&db),
            vec![
                (
                    "cred_w".to_owned(),
                    json!({"type": "key", "key": "go-work"}),
                    Some(false)
                ),
                (
                    "cred_p".to_owned(),
                    json!({"type": "key", "key": "go-perso"}),
                    Some(true)
                ),
            ]
        );
    }

    #[test]
    fn a_key_goes_in_front_of_an_oauth_login_which_stays() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::open(&tmp.path().join("store"));
        let p = opencode(tmp.path(), None);
        let db = database(tmp.path(), true);
        let oauth = json!({"type": "oauth", "access": "t", "refresh": "r", "expires": 1});
        row(&db, "cred_o", oauth.clone(), Some(true), 1);
        save(&store, &p, "work", "go-work");

        let err = ops::switch(&store, &p, "work", false).unwrap_err();
        assert!(err.to_string().contains("a login of type oauth"), "{err}");
        ops::switch(&store, &p, "work", true).unwrap();

        let after = rows(&db);
        assert_eq!(after.len(), 2);
        assert_eq!(after[0], ("cred_o".to_owned(), oauth, Some(false)));
        assert!(after[1].0.starts_with("cred_"), "{}", after[1].0);
        assert_eq!(after[1].0.len(), "cred_".len() + 26);
        assert_eq!(after[1].1, json!({"type": "key", "key": "go-work"}));
        assert_eq!(after[1].2, Some(true));
        assert_eq!(p.live().unwrap(), Some(json!({"key": "go-work"})));
    }

    #[test]
    fn a_key_changed_in_opencode_since_it_was_read_is_not_switched_over() {
        let tmp = tempfile::tempdir().unwrap();
        let mut db = database(tmp.path(), true);
        row(
            &db,
            "cred_1",
            json!({"type": "key", "key": "go-new"}),
            Some(true),
            1,
        );
        let err = install_row(
            &mut db,
            "opencode-go",
            "go-perso",
            Some(&json!({"key": "go-work"})),
        )
        .unwrap_err();
        assert!(err.is::<Changed>(), "{err:#}");
        assert_eq!(rows(&db)[0].1["key"], "go-new");
    }

    #[test]
    fn until_opencode_imports_auth_json_the_file_is_the_login() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::open(&tmp.path().join("store"));
        let p = opencode(tmp.path(), None);
        let db = database(tmp.path(), false);
        row(
            &db,
            "cred_1",
            json!({"type": "key", "key": "go-db"}),
            Some(true),
            1,
        );
        write_json(
            &tmp.path().join("auth.json"),
            &json!({"opencode-go": {"type": "api", "key": "go-work"}}),
        )
        .unwrap();
        save(&store, &p, "work", "go-work");
        save(&store, &p, "perso", "go-perso");

        assert_eq!(p.live().unwrap(), Some(json!({"key": "go-work"})));
        assert_eq!(p.live_file(), Some(tmp.path().join("auth.json")));
        ops::switch(&store, &p, "perso", false).unwrap();

        let file = read_json(&tmp.path().join("auth.json")).unwrap().unwrap();
        assert_eq!(file["opencode-go"]["key"], "go-perso");
        assert_eq!(rows(&db)[0].1["key"], "go-db");
    }

    #[test]
    fn a_database_that_is_not_one_is_an_error_not_a_signed_out_cli() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("opencode.db"),
            "not sqlite at all, not even close",
        )
        .unwrap();
        let err = opencode(tmp.path(), None).live().unwrap_err();
        assert!(format!("{err:#}").contains("opencode.db"), "{err:#}");
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
