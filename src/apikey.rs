use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use rusqlite::{Connection, OpenFlags, OptionalExtension, TransactionBehavior};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::fsx::{Changed, read_json, update_json};
use crate::paths;
use crate::provider::{Field, Format, Identity, Login, PendingLogin, Provider, Upstream};
use crate::store::Entry;

/// A provider signed into with an API key. A key has no account behind it to
/// confirm, never expires and never refreshes, so it is filed under a digest
/// of itself: two copies of one key are one credential, and two keys are two.
pub struct ApiKey {
    id: &'static str,
    name: &'static str,
    /// The variables the CLI reads, in its order.
    envs: &'static [&'static str],
    /// The one `remuda env` sets.
    export: &'static str,
    key_page: &'static str,
    asks_for: &'static str,
    /// The entries of `file`, and the integrations of `db`, the CLI reads a
    /// key from, in its order.
    slots: &'static [&'static str],
    /// A JSON file other entries share.
    file: Option<PathBuf>,
    /// opencode 2's `credential` table. Once opencode has imported
    /// `auth.json` into it, opencode reads its logins from here and never
    /// from the file.
    db: Option<PathBuf>,
    env: Box<Env>,
    /// Where the key is read when no variable and no slot holds one.
    fallback: Option<Box<Lookup>>,
    region: Option<Region>,
    extras: &'static [Extra],
    inference: String,
    routes: Routes,
}

/// The region a key answers in, stored as `region`: it picks the slot a key
/// goes in, and a slot says the region of the key it holds.
struct Region {
    /// The variable that names it beside the key's own.
    var: &'static str,
    /// The default first.
    choices: &'static [&'static str],
    of_slot: fn(&str) -> &'static str,
    /// Where a key of a region goes that none of its slots holds, given the
    /// live key's slot.
    slot_for: fn(Option<&str>, &str) -> &'static str,
}

/// opencode's MiniMax entry for `region` of the live key's kind: the coding
/// plan's, or the pay-as-you-go API's.
fn minimax_slot(live: Option<&str>, region: &str) -> &'static str {
    let plan = live.is_some_and(|slot| slot.ends_with("-coding-plan"));
    match (region, plan) {
        ("cn", true) => "minimax-cn-coding-plan",
        ("cn", false) => "minimax-cn",
        (_, true) => "minimax-coding-plan",
        (_, false) => "minimax",
    }
}

/// A stored field beside the key that `remuda env` exports into the first of
/// `vars` and a switch ignores.
struct Extra {
    field: &'static str,
    vars: &'static [&'static str],
    label: &'static str,
}

fn minimax_region(slot: &str) -> &'static str {
    if slot.starts_with("minimax-cn") {
        "cn"
    } else {
        "global"
    }
}

/// What each slot holds, in the CLI's order.
type Found = Vec<(&'static str, Option<Value>)>;

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

/// The proxy reaches none of this provider's models.
fn no_routes(_model: &str) -> &'static [(Format, &'static str)] {
    &[]
}

type Env = dyn Fn(&str) -> Option<String> + Send + Sync;
type Lookup = dyn Fn() -> Option<String> + Send + Sync;

fn process_env(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

/// The token the GitHub CLI is signed in with, which it may keep in the
/// system keyring rather than in a file.
fn gh_token() -> Option<String> {
    #[cfg(debug_assertions)]
    let gh = std::env::var_os("REMUDA_TEST_GH").unwrap_or_else(|| "gh".into());
    #[cfg(not(debug_assertions))]
    let gh = "gh";
    gh_token_from(std::path::Path::new(&gh), GH_TIMEOUT)
}

/// TokenGauge's wait for the same command.
const GH_TIMEOUT: Duration = Duration::from_secs(3);

fn gh_token_from(gh: &std::path::Path, wait: Duration) -> Option<String> {
    let ran = crate::child::run(
        "gh",
        gh,
        &["auth", "token", "--hostname", "github.com"],
        wait,
        64 * 1024,
    )
    .ok()?;
    ran.ok.then(|| String::from_utf8(ran.out).ok()).flatten()
}

fn key_of(creds: &Value) -> Option<&str> {
    key_of_field(creds, "key")
}

fn key_of_field<'a>(creds: &'a Value, field: &str) -> Option<&'a str> {
    creds.get(field)?.as_str().filter(|k| !k.trim().is_empty())
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

/// The slot the CLI reads: the first holding a key, else the first holding
/// anything.
fn read_slot(found: &Found) -> Option<(&'static str, &Value)> {
    let held = || found.iter().filter_map(|(s, v)| Some((*s, v.as_ref()?)));
    held()
        .find(|(_, v)| key_of(v).is_some())
        .or_else(|| held().next())
}

fn read_key(found: &Found) -> Option<&str> {
    read_slot(found).and_then(|(_, v)| key_of(v)).map(str::trim)
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.trim().replace('\'', r"'\''"))
}

impl ApiKey {
    pub fn glm() -> Self {
        ApiKey {
            id: "glm",
            name: "GLM",
            envs: &["Z_AI_API_KEY", "ZAI_API_TOKEN"],
            export: "Z_AI_API_KEY",
            key_page: "https://z.ai/manage-apikey/apikey-list",
            asks_for: "the API key",
            slots: &[],
            file: None,
            db: None,
            env: Box::new(process_env),
            fallback: None,
            region: None,
            extras: &[],
            inference: "https://api.z.ai/api".to_owned(),
            routes: glm_routes,
        }
    }

    pub fn opencode() -> Self {
        ApiKey {
            id: "opencode",
            name: "opencode Go",
            envs: &["OPENCODE_API_KEY", "OPENCODE_GO_API_KEY"],
            export: "OPENCODE_API_KEY",
            key_page: "https://opencode.ai/auth",
            asks_for: "the API key",
            slots: &["opencode-go"],
            file: Some(paths::opencode_auth()),
            db: Some(paths::opencode_db()),
            env: Box::new(process_env),
            fallback: None,
            region: None,
            extras: &[],
            inference: "https://opencode.ai/inference/go".to_owned(),
            routes: opencode_routes,
        }
    }

    /// The key DeepSeek's API takes, which opencode keeps beside its others.
    pub fn deepseek() -> Self {
        ApiKey {
            id: "deepseek",
            name: "DeepSeek",
            envs: &["DEEPSEEK_API_KEY"],
            export: "DEEPSEEK_API_KEY",
            key_page: "https://platform.deepseek.com/api_keys",
            asks_for: "the API key",
            slots: &["deepseek"],
            file: Some(paths::opencode_auth()),
            db: Some(paths::opencode_db()),
            env: Box::new(process_env),
            fallback: None,
            region: None,
            extras: &[],
            inference: String::new(),
            routes: no_routes,
        }
    }

    /// A MiniMax key, global or China, which opencode files under the
    /// region's entry, of the pay-as-you-go API or of the coding plan.
    pub fn minimax() -> Self {
        ApiKey {
            id: "minimax",
            name: "MiniMax",
            envs: &["MINIMAX_API_KEY"],
            export: "MINIMAX_API_KEY",
            key_page: "https://platform.minimax.io/user-center/basic-information/interface-key",
            asks_for: "the API key",
            slots: &[
                "minimax",
                "minimax-cn",
                "minimax-coding-plan",
                "minimax-cn-coding-plan",
            ],
            file: Some(paths::opencode_auth()),
            db: Some(paths::opencode_db()),
            env: Box::new(process_env),
            fallback: None,
            region: Some(Region {
                var: "MINIMAX_REGION",
                choices: &["global", "cn"],
                of_slot: minimax_region,
                slot_for: minimax_slot,
            }),
            extras: &[],
            inference: String::new(),
            routes: no_routes,
        }
    }

    /// An OpenRouter inference key, read from the environment only, and the
    /// management key the account balance answers to, when there is one.
    pub fn openrouter() -> Self {
        ApiKey {
            id: "openrouter",
            name: "OpenRouter",
            envs: &["OPENROUTER_API_KEY", "OPENROUTER_KEY"],
            export: "OPENROUTER_API_KEY",
            key_page: "https://openrouter.ai/settings/keys",
            asks_for: "the API key",
            slots: &[],
            file: None,
            db: None,
            env: Box::new(process_env),
            fallback: None,
            region: None,
            extras: &[Extra {
                field: "management",
                vars: &["OPENROUTER_MANAGEMENT_KEY", "OPENROUTER_PROVISIONING_KEY"],
                label: "the management key, which the account balance needs",
            }],
            inference: String::new(),
            routes: no_routes,
        }
    }

    /// A GitHub token, OAuth or personal, that Copilot's quota answers to.
    /// The GitHub CLI's own login is read but never written: a switch is
    /// `GH_TOKEN`, which it and the Copilot CLI read first.
    pub fn copilot() -> Self {
        ApiKey {
            id: "copilot",
            name: "Copilot",
            envs: &["COPILOT_QUOTA_TOKEN", "GH_TOKEN", "GITHUB_TOKEN"],
            export: "GH_TOKEN",
            key_page: "https://github.com/settings/tokens",
            asks_for: "the GitHub token (`gh auth token` prints yours)",
            slots: &[],
            file: None,
            db: None,
            env: Box::new(process_env),
            fallback: Some(Box::new(gh_token)),
            region: None,
            extras: &[],
            inference: String::new(),
            routes: no_routes,
        }
    }

    /// The database moves with the file: a test's file has its database
    /// beside it. Nothing outside the test is asked for a key.
    #[cfg(test)]
    pub fn with(
        mut self,
        file: Option<PathBuf>,
        env: impl Fn(&str) -> Option<String> + Send + Sync + 'static,
    ) -> Self {
        self.db = self
            .db
            .take()
            .and_then(|_| Some(file.as_ref()?.with_file_name("opencode.db")));
        self.file = file;
        self.env = Box::new(env);
        self.fallback = None;
        self
    }

    #[cfg(test)]
    pub fn falling_back_to(
        mut self,
        lookup: impl Fn() -> Option<String> + Send + Sync + 'static,
    ) -> Self {
        self.fallback = Some(Box::new(lookup));
        self
    }

    #[cfg(test)]
    pub fn inferring_at(mut self, base: &str) -> Self {
        self.inference = base.to_owned();
        self
    }

    fn fields(&self) -> Vec<Field> {
        let region = self.region.as_ref().map(|r| Field {
            name: "region",
            label: "the region",
            choices: r.choices,
        });
        region
            .into_iter()
            .chain(self.extras.iter().map(|e| Field {
                name: e.field,
                label: e.label,
                choices: &[],
            }))
            .collect()
    }

    /// The region `creds` say, else the default.
    fn region_of(&self, creds: &Value) -> Option<&'static str> {
        let r = self.region.as_ref()?;
        let said = creds.get("region").and_then(Value::as_str).unwrap_or("");
        Some(
            r.choices
                .iter()
                .copied()
                .find(|c| *c == said)
                .unwrap_or(r.choices[0]),
        )
    }

    /// The credential the CLI reads, `key` in `region`.
    fn creds(&self, key: &str, region: Option<&str>) -> Value {
        let mut creds = json!({ "key": key });
        if let Some(region) = region.filter(|_| self.region.is_some()) {
            creds["region"] = json!(region);
        }
        creds
    }

    /// The first variable that holds a key, and the key.
    fn env_key(&self) -> Option<(&'static str, String)> {
        self.envs.iter().find_map(|name| {
            let key = (self.env)(name)?.trim().to_owned();
            (!key.is_empty()).then_some((*name, key))
        })
    }

    /// opencode's database, when it is where opencode reads its logins.
    fn database(&self, flags: OpenFlags) -> Result<Option<Connection>> {
        let Some(db) = &self.db else {
            return Ok(None);
        };
        open_db(db, flags)
    }

    fn found_in_db(&self, conn: &Connection) -> Result<Found> {
        self.slots
            .iter()
            .map(|slot| Ok((*slot, selected(conn, slot)?.map(|(_, value)| value))))
            .collect()
    }

    fn found_in_file(&self, json: &Value) -> Found {
        self.slots
            .iter()
            .map(|slot| (*slot, json.get(slot).cloned()))
            .collect()
    }

    /// What the CLI's slots hold: from the database once opencode reads it,
    /// else from the file.
    fn found(&self) -> Result<Found> {
        if let Some(conn) = self.database(OpenFlags::SQLITE_OPEN_READ_ONLY)? {
            return self.found_in_db(&conn);
        }
        let Some(file) = &self.file else {
            return Ok(Vec::new());
        };
        Ok(read_json(file)?.map_or_else(Vec::new, |json| self.found_in_file(&json)))
    }

    /// The CLI's login, and the slot it is in.
    fn stored_login(&self) -> Result<Option<(&'static str, Value)>> {
        Ok(read_slot(&self.found()?).map(|(slot, value)| (slot, value.clone())))
    }

    /// The slot a key goes in, and the live key's slot when the switch
    /// moves it out of there. Without a region it is the slot the CLI reads.
    /// With one it is the region's slot holding a key, else the region's
    /// counterpart of the live key's slot, which is emptied when it is
    /// another region's. Any other key read ahead of it would still be the
    /// one the CLI uses.
    fn target(
        &self,
        found: &Found,
        region: Option<&str>,
    ) -> Result<(&'static str, Option<&'static str>)> {
        let live = read_slot(found)
            .filter(|(_, v)| key_of(v).is_some())
            .map(|(slot, _)| slot);
        let (slot, emptied) = match (&self.region, region) {
            (Some(r), Some(want)) => {
                let held = found
                    .iter()
                    .find(|(s, v)| (r.of_slot)(s) == want && v.as_ref().and_then(key_of).is_some())
                    .map(|(s, _)| *s);
                (
                    held.unwrap_or_else(|| (r.slot_for)(live, want)),
                    live.filter(|l| (r.of_slot)(l) != want),
                )
            }
            _ => (read_slot(found).map_or(self.slots[0], |(s, _)| s), None),
        };
        if let Some((other, _)) = found
            .iter()
            .take_while(|(s, _)| *s != slot)
            .find(|(s, v)| Some(*s) != emptied && v.as_ref().and_then(key_of).is_some())
        {
            bail!(
                "{} reads the key in {other} before {slot}, where this login's key goes: remove {other} from opencode first",
                self.name
            );
        }
        Ok((slot, emptied))
    }

    /// Makes `key` the credential opencode uses, as opencode's own switch
    /// would: a row already holding it is activated, the key row in use is
    /// rekeyed, and otherwise a new row is added in front of a login that is
    /// not a key, which stays behind it.
    fn install_row(
        &self,
        conn: &mut Connection,
        key: &str,
        region: Option<&str>,
        outgoing: Option<&Value>,
    ) -> Result<()> {
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let found = self.found_in_db(&tx)?;
        if outgoing.is_some_and(|o| key_of(o).map(str::trim) != read_key(&found)) {
            return Err(Changed.into());
        }
        let live = read_key(&found).map(str::to_owned);
        let (integration, emptied) = self.target(&found, region)?;
        if let Some(emptied) = emptied {
            let live = live.clone().unwrap_or_default();
            let rows: Vec<(String, String)> = tx
                .prepare("SELECT id, value FROM credential WHERE integration_id = ?1")?
                .query_map([emptied], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect::<rusqlite::Result<_>>()?;
            for (id, value) in rows {
                let value: Value = serde_json::from_str(&value).unwrap_or(Value::Null);
                if key_of(&value).map(str::trim) == Some(live.as_str()) {
                    tx.execute("DELETE FROM credential WHERE id = ?1", [&id])?;
                }
            }
            if let (_, Some(still)) = self.target(&self.found_in_db(&tx)?, region)? {
                bail!(
                    "{} holds more keys in {still}, read before {integration}: remove them from opencode first",
                    self.name
                );
            }
        }
        let current = selected(&tx, integration)?;
        let holder = {
            let mut rows =
                tx.prepare("SELECT id, value FROM credential WHERE integration_id = ?1")?;
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
            (None, Some((id, mut value)))
                if key_of(&value).is_some() && key_of(&value).map(str::trim) == live.as_deref() =>
            {
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
            .and_then(|f| f.parent().map(PathBuf::from))
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
        self.file.clone()
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
        if let Some((_, key)) = self.env_key() {
            let region = self.region.as_ref().map(|r| {
                let said = (self.env)(r.var).unwrap_or_default();
                r.choices
                    .iter()
                    .copied()
                    .find(|c| *c == said.trim())
                    .unwrap_or(r.choices[0])
            });
            let mut creds = self.creds(&key, region);
            for extra in self.extras {
                let value = extra.vars.iter().find_map(|var| {
                    Some((self.env)(var)?.trim().to_owned()).filter(|v| !v.is_empty())
                });
                if let Some(value) = value {
                    creds[extra.field] = json!(value);
                }
            }
            return Ok(Some(creds));
        }
        if let Some((slot, login)) = self.stored_login()?
            && let Some(key) = key_of(&login)
        {
            let region = self.region.as_ref().map(|r| (r.of_slot)(slot));
            return Ok(Some(self.creds(key.trim(), region)));
        }
        Ok(self
            .fallback
            .as_ref()
            .and_then(|lookup| lookup())
            .map(|key| key.trim().to_owned())
            .filter(|key| !key.is_empty())
            .map(|key| self.creds(&key, None)))
    }

    /// What the store holds beside the key outlives a sync: the CLI's slot
    /// carries only the key.
    fn synced(&self, stored: &Value, live: Value) -> Value {
        match (stored.as_object(), live) {
            (Some(stored), Value::Object(live)) => {
                let mut merged = stored.clone();
                merged.extend(live);
                Value::Object(merged)
            }
            (_, live) => live,
        }
    }

    /// A login in the key's slot that is not a key, which a switch would
    /// replace.
    fn foreign_login(&self) -> Result<Option<String>> {
        if self.env_key().is_some() {
            return Ok(None);
        }
        Ok(self
            .stored_login()?
            .map(|(_, login)| login)
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
        let mut lines: Vec<String> = self
            .envs
            .iter()
            .take_while(|var| **var != self.export)
            .map(|var| format!("unset {var}"))
            .collect();
        lines.push(format!("export {}={}", self.export, shell_quote(key)));
        if let (Some(r), Some(region)) = (&self.region, self.region_of(&entry.creds)) {
            lines.push(format!("export {}={}", r.var, shell_quote(region)));
        }
        for extra in self.extras {
            match key_of_field(&entry.creds, extra.field) {
                Some(value) => {
                    lines.push(format!("export {}={}", extra.vars[0], shell_quote(value)))
                }
                None => lines.extend(extra.vars.iter().map(|var| format!("unset {var}"))),
            }
        }
        Ok(lines.join("\n"))
    }

    /// Writes the key into the CLI's file. A key the environment sets wins
    /// over the file, and no write can change that.
    fn install(&self, entry: &Entry, outgoing: Option<&Value>) -> Result<()> {
        let key = key_of(&entry.creds).context("stored credential holds no key")?;
        let qualified = entry.qualified();
        let region = self.region_of(&entry.creds);
        if let Some((var, _)) = self.env_key() {
            bail!(
                "{} reads its key from {var}, which remuda cannot change in a running shell: run `eval \"$(remuda env {qualified})\"`",
                self.name
            );
        }
        if let Some(mut conn) = self.database(OpenFlags::SQLITE_OPEN_READ_WRITE)? {
            return self.install_row(&mut conn, key.trim(), region, outgoing);
        }
        let Some(file) = &self.file else {
            bail!(
                "{} reads its key from {}: run `eval \"$(remuda env {qualified})\"`",
                self.name,
                self.export
            );
        };
        update_json(file, |json| {
            if !json.is_object() {
                *json = json!({});
            }
            let found = self.found_in_file(json);
            if outgoing.is_some_and(|o| key_of(o).map(str::trim) != read_key(&found)) {
                return Err(Changed.into());
            }
            let (slot, emptied) = self.target(&found, region)?;
            let held = found
                .iter()
                .find(|(s, _)| *s == slot)
                .and_then(|(_, v)| v.as_ref());
            let live = read_slot(&found).map(|(s, _)| s);
            if held.is_some_and(|held| {
                Some(slot) != live && key_of(held).map(str::trim) != Some(key.trim())
            }) {
                bail!(
                    "{}'s {slot} entry holds a login that is not the live one, which this switch would overwrite: remove it from {} first",
                    self.name,
                    file.display()
                );
            }
            if let Some(emptied) = emptied
                && let Some(entries) = json.as_object_mut()
            {
                entries.remove(emptied);
            }
            match json.get_mut(slot).and_then(Value::as_object_mut) {
                Some(entry) if entry.get("type").and_then(Value::as_str) == Some("api") => {
                    entry.insert("key".into(), json!(key.trim()));
                }
                _ => json[slot] = json!({ "type": "api", "key": key.trim() }),
            }
            Ok(())
        })
    }

    fn refresh(&self, _creds: &mut Value) -> Result<Option<String>> {
        bail!("an API key does not refresh")
    }

    fn begin_login(&self, _paste: bool) -> Result<Box<dyn PendingLogin>> {
        Ok(Box::new(KeyPending {
            url: self.key_page.to_owned(),
            asks_for: self.asks_for,
            fields: self.fields(),
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
    match std::fs::metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("could not read {}", path.display())),
        Ok(_) => {}
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
    asks_for: &'static str,
    fields: Vec<Field>,
}

impl PendingLogin for KeyPending {
    fn url(&self) -> &str {
        &self.url
    }

    fn needs_code(&self) -> bool {
        true
    }

    fn asks_for(&self) -> &'static str {
        self.asks_for
    }

    fn fields(&self) -> Vec<Field> {
        self.fields.clone()
    }

    fn finish(self: Box<Self>, code: Option<&str>) -> Result<Login> {
        self.finish_with(code, &serde_json::Map::new())
    }

    fn finish_with(
        self: Box<Self>,
        code: Option<&str>,
        answers: &serde_json::Map<String, Value>,
    ) -> Result<Login> {
        let key = code
            .map(str::trim)
            .filter(|k| !k.is_empty())
            .context("no key was pasted")?;
        let mut creds = json!({ "key": key });
        for field in &self.fields {
            let answer = answers
                .get(field.name)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|a| !a.is_empty());
            let value = match (field.choices, answer) {
                ([], None) => continue,
                ([], Some(answer)) => answer,
                ([first, ..], None) => first,
                (choices, Some(answer)) => choices
                    .iter()
                    .copied()
                    .find(|c| c.eq_ignore_ascii_case(answer))
                    .with_context(|| {
                        format!("{} is {}, not {answer}", field.label, choices.join(" or "))
                    })?,
            };
            creds[field.name] = json!(value);
        }
        Ok(Login {
            creds,
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
        ApiKey::opencode().with(Some(dir.join("auth.json")), move |name| {
            (name == "OPENCODE_API_KEY")
                .then(|| env.map(str::to_owned))
                .flatten()
        })
    }

    fn save(store: &Store, p: &ApiKey, name: &str, key: &str) {
        let login = Box::new(KeyPending {
            url: String::new(),
            asks_for: "the API key",
            fields: Vec::new(),
        })
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
        let login = Box::new(KeyPending {
            url: "u".into(),
            asks_for: "the API key",
            fields: Vec::new(),
        })
        .finish(Some("  sk-1  "))
        .unwrap();
        assert_eq!(login.creds, json!({"key": "sk-1"}));
        assert!(
            Box::new(KeyPending {
                url: "u".into(),
                asks_for: "the API key",
                fields: Vec::new(),
            })
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
        let p = ApiKey::opencode().with(Some(tmp.path().join("auth.json")), |name| match name {
            "OPENCODE_API_KEY" => Some(" ".into()),
            "OPENCODE_GO_API_KEY" => Some("k1".into()),
            _ => None,
        });
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
        row_in(conn, "opencode-go", id, value, active, created);
    }

    fn row_in(
        conn: &Connection,
        integration: &str,
        id: &str,
        value: Value,
        active: Option<bool>,
        created: i64,
    ) {
        conn.execute(
            "INSERT INTO credential (id, integration_id, label, value, active, time_created, time_updated) \
             VALUES (?1, ?2, 'API key', ?3, ?4, ?5, ?5)",
            rusqlite::params![id, integration, value.to_string(), active, created],
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
        let err = opencode(tmp.path(), None)
            .install_row(&mut db, "go-perso", None, Some(&json!({"key": "go-work"})))
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

    #[cfg(unix)]
    #[test]
    fn a_database_whose_directory_cannot_be_searched_is_an_error() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("auth.json"),
            r#"{"opencode-go": {"type": "api", "key": "go"}}"#,
        )
        .unwrap();
        let hidden = tmp.path().join("hidden");
        std::fs::create_dir(&hidden).unwrap();
        let mut p = opencode(tmp.path(), None);
        p.db = Some(hidden.join("opencode.db"));
        std::fs::set_permissions(&hidden, std::fs::Permissions::from_mode(0o000)).unwrap();
        let enforced = std::fs::metadata(hidden.join("opencode.db"))
            .is_err_and(|e| e.kind() == std::io::ErrorKind::PermissionDenied);
        let live = p.live();
        std::fs::set_permissions(&hidden, std::fs::Permissions::from_mode(0o755)).unwrap();
        if enforced {
            let err = live.unwrap_err();
            assert!(format!("{err:#}").contains("opencode.db"), "{err:#}");
        }
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
        assert!(!p.signs_out());
    }

    /// A login signed in through `p`'s own sign-in, answering its fields.
    fn sign_in(store: &Store, p: &ApiKey, name: &str, key: &str, answers: Value) {
        let login = p
            .begin_login(false)
            .unwrap()
            .finish_with(Some(key), answers.as_object().unwrap())
            .unwrap();
        store
            .save(&Entry::new(p.id(), name, login.creds, login.identity, 1))
            .unwrap();
    }

    fn vars(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |name| {
            pairs
                .iter()
                .find(|(n, _)| *n == name)
                .map(|(_, v)| (*v).to_owned())
        }
    }

    #[test]
    fn a_key_is_named_as_tokengauge_names_it() {
        let hex: String = Sha256::digest(b"sk-or-v1-abc")
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(
            identity_of(" sk-or-v1-abc ").account_id,
            format!("key-{}", &hex[..16])
        );
    }

    #[test]
    fn deepseek_is_switched_in_opencodes_deepseek_entry() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::open(&tmp.path().join("store"));
        let p = ApiKey::deepseek().with(Some(tmp.path().join("auth.json")), |_| None);
        write_json(
            &tmp.path().join("auth.json"),
            &json!({
                "deepseek": {"type": "api", "key": "ds-work"},
                "opencode-go": {"type": "api", "key": "go-1"},
            }),
        )
        .unwrap();
        save(&store, &p, "work", "ds-work");
        save(&store, &p, "perso", "ds-perso");
        assert_eq!(p.live().unwrap(), Some(json!({"key": "ds-work"})));
        ops::switch(&store, &p, "perso", false).unwrap();
        let file = read_json(&tmp.path().join("auth.json")).unwrap().unwrap();
        assert_eq!(file["deepseek"], json!({"type": "api", "key": "ds-perso"}));
        assert_eq!(file["opencode-go"], json!({"type": "api", "key": "go-1"}));
    }

    #[test]
    fn deepseek_in_the_environment_is_switched_by_the_shell() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::open(&tmp.path().join("store"));
        let p = ApiKey::deepseek().with(
            Some(tmp.path().join("auth.json")),
            vars(&[("DEEPSEEK_API_KEY", "ds-work")]),
        );
        save(&store, &p, "work", "ds-work");
        save(&store, &p, "perso", "ds-perso");
        let err = ops::switch(&store, &p, "perso", false).unwrap_err();
        assert!(
            format!("{err:#}").contains("eval \"$(remuda env deepseek/perso)\""),
            "{err:#}"
        );
        let perso = store.get("deepseek", "perso").unwrap().unwrap();
        assert_eq!(
            p.env_line(&perso).unwrap(),
            "export DEEPSEEK_API_KEY='ds-perso'"
        );
        assert!(!tmp.path().join("auth.json").exists());
    }

    #[test]
    fn deepseek_is_switched_in_opencode_2s_database() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::open(&tmp.path().join("store"));
        let p = ApiKey::deepseek().with(Some(tmp.path().join("auth.json")), |_| None);
        let db = database(tmp.path(), true);
        row_in(
            &db,
            "deepseek",
            "cred_d",
            json!({"type": "key", "key": "ds-work"}),
            Some(true),
            1,
        );
        row(
            &db,
            "cred_g",
            json!({"type": "key", "key": "go-1"}),
            Some(true),
            2,
        );
        save(&store, &p, "work", "ds-work");
        save(&store, &p, "perso", "ds-perso");
        ops::switch(&store, &p, "perso", false).unwrap();
        let after = rows(&db);
        assert_eq!(after[0].1, json!({"type": "key", "key": "ds-perso"}));
        assert_eq!(after[1].1, json!({"type": "key", "key": "go-1"}));
    }

    fn minimax(dir: &std::path::Path, env: &'static [(&'static str, &'static str)]) -> ApiKey {
        ApiKey::minimax().with(Some(dir.join("auth.json")), vars(env))
    }

    #[test]
    fn a_minimax_sign_in_stores_its_region() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::open(&tmp.path().join("store"));
        let p = minimax(tmp.path(), &[]);
        let fields = p.begin_login(false).unwrap().fields();
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].name, "region");
        assert_eq!(fields[0].choices, ["global", "cn"]);
        sign_in(&store, &p, "cn", "mm-cn", json!({"region": "CN"}));
        sign_in(&store, &p, "global", "mm-global", json!({}));
        assert_eq!(
            store.get("minimax", "cn").unwrap().unwrap().creds,
            json!({"key": "mm-cn", "region": "cn"})
        );
        assert_eq!(
            store.get("minimax", "global").unwrap().unwrap().creds,
            json!({"key": "mm-global", "region": "global"})
        );
        let err = p
            .begin_login(false)
            .unwrap()
            .finish_with(Some("k"), json!({"region": "eu"}).as_object().unwrap())
            .err()
            .unwrap();
        assert!(err.to_string().contains("global or cn"), "{err}");
    }

    #[test]
    fn minimax_reads_the_region_off_the_entry_or_the_environment() {
        let tmp = tempfile::tempdir().unwrap();
        write_json(
            &tmp.path().join("auth.json"),
            &json!({"minimax-cn-coding-plan": {"type": "api", "key": "mm-cn"}}),
        )
        .unwrap();
        assert_eq!(
            minimax(tmp.path(), &[]).live().unwrap(),
            Some(json!({"key": "mm-cn", "region": "cn"}))
        );
        assert_eq!(
            minimax(
                tmp.path(),
                &[("MINIMAX_API_KEY", "mm-env"), ("MINIMAX_REGION", "cn")]
            )
            .live()
            .unwrap(),
            Some(json!({"key": "mm-env", "region": "cn"}))
        );
        assert_eq!(
            minimax(tmp.path(), &[("MINIMAX_API_KEY", "mm-env")])
                .live()
                .unwrap(),
            Some(json!({"key": "mm-env", "region": "global"}))
        );
    }

    #[test]
    fn the_shells_minimax_region_is_global_unless_it_says_cn() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::open(&tmp.path().join("store"));
        let p = minimax(tmp.path(), &[("MINIMAX_API_KEY", "mm-cn")]);
        sign_in(&store, &p, "cn", "mm-cn", json!({"region": "cn"}));
        assert_eq!(
            ops::sync_live(&store, &p).unwrap(),
            LiveState::Stored {
                name: "cn".into(),
                synced: true
            }
        );
        assert_eq!(
            store.get("minimax", "cn").unwrap().unwrap().creds,
            json!({"key": "mm-cn", "region": "global"})
        );
        let p = minimax(
            tmp.path(),
            &[("MINIMAX_API_KEY", "mm-cn"), ("MINIMAX_REGION", "cn")],
        );
        ops::sync_live(&store, &p).unwrap();
        assert_eq!(
            store.get("minimax", "cn").unwrap().unwrap().creds,
            json!({"key": "mm-cn", "region": "cn"})
        );
    }

    #[test]
    fn minimax_is_switched_in_its_regions_entry() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::open(&tmp.path().join("store"));
        let p = minimax(tmp.path(), &[]);
        write_json(
            &tmp.path().join("auth.json"),
            &json!({"minimax-coding-plan": {"type": "api", "key": "mm-g1"}}),
        )
        .unwrap();
        sign_in(&store, &p, "g1", "mm-g1", json!({"region": "global"}));
        sign_in(&store, &p, "g2", "mm-g2", json!({"region": "global"}));
        sign_in(&store, &p, "cn", "mm-cn", json!({"region": "cn"}));
        let file = || read_json(&tmp.path().join("auth.json")).unwrap().unwrap();

        ops::switch(&store, &p, "g2", false).unwrap();
        assert_eq!(
            file(),
            json!({"minimax-coding-plan": {"type": "api", "key": "mm-g2"}})
        );

        ops::switch(&store, &p, "cn", false).unwrap();
        assert_eq!(
            file(),
            json!({"minimax-cn-coding-plan": {"type": "api", "key": "mm-cn"}})
        );
        assert_eq!(
            p.live().unwrap(),
            Some(json!({"key": "mm-cn", "region": "cn"}))
        );

        ops::switch(&store, &p, "g1", false).unwrap();
        assert_eq!(
            file(),
            json!({"minimax-coding-plan": {"type": "api", "key": "mm-g1"}})
        );
        assert_eq!(
            p.live().unwrap(),
            Some(json!({"key": "mm-g1", "region": "global"}))
        );
    }

    #[test]
    fn a_minimax_key_that_is_not_the_live_one_is_never_overwritten() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::open(&tmp.path().join("store"));
        let p = minimax(tmp.path(), &[]);
        let before = json!({
            "minimax": {"type": "api", "key": "mm-g1"},
            "minimax-cn": {"type": "api", "key": "mm-other"},
        });
        write_json(&tmp.path().join("auth.json"), &before).unwrap();
        sign_in(&store, &p, "g1", "mm-g1", json!({"region": "global"}));
        sign_in(&store, &p, "cn2", "mm-cn2", json!({"region": "cn"}));
        let err = ops::switch(&store, &p, "cn2", false).unwrap_err();
        assert!(
            format!("{err:#}").contains("minimax-cn entry holds a login"),
            "{err:#}"
        );
        assert_eq!(
            read_json(&tmp.path().join("auth.json")).unwrap(),
            Some(before)
        );
    }

    #[test]
    fn a_minimax_key_read_ahead_of_the_live_one_refuses_the_switch() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::open(&tmp.path().join("store"));
        let p = minimax(tmp.path(), &[]);
        let before = json!({
            "minimax": {"type": "api", "key": "mm-g1"},
            "minimax-coding-plan": {"type": "api", "key": "mm-g-other"},
            "minimax-cn-coding-plan": {"type": "api", "key": "mm-cn"},
        });
        write_json(&tmp.path().join("auth.json"), &before).unwrap();
        sign_in(&store, &p, "g1", "mm-g1", json!({"region": "global"}));
        sign_in(&store, &p, "cn", "mm-cn", json!({"region": "cn"}));
        let err = ops::switch(&store, &p, "cn", false).unwrap_err();
        assert!(
            format!("{err:#}").contains("remove minimax-coding-plan from opencode"),
            "{err:#}"
        );
        assert_eq!(
            read_json(&tmp.path().join("auth.json")).unwrap(),
            Some(before)
        );
    }

    #[test]
    fn minimax_moves_regions_in_opencode_2s_database() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::open(&tmp.path().join("store"));
        let p = minimax(tmp.path(), &[]);
        let db = database(tmp.path(), true);
        row_in(
            &db,
            "minimax-cn",
            "cred_c",
            json!({"type": "key", "key": "mm-cn"}),
            Some(true),
            1,
        );
        row_in(
            &db,
            "minimax-coding-plan",
            "cred_g",
            json!({"type": "key", "key": "mm-old"}),
            Some(true),
            2,
        );
        sign_in(&store, &p, "cn", "mm-cn", json!({"region": "cn"}));
        sign_in(&store, &p, "g", "mm-g", json!({"region": "global"}));
        ops::switch(&store, &p, "g", false).unwrap();
        let after = rows(&db);
        assert_eq!(after.len(), 2, "{after:?}");
        assert_eq!(
            after[0],
            (
                "cred_g".to_owned(),
                json!({"type": "key", "key": "mm-old"}),
                Some(false)
            )
        );
        assert_eq!(after[1].1, json!({"type": "key", "key": "mm-g"}));
        assert_eq!(
            p.live().unwrap(),
            Some(json!({"key": "mm-g", "region": "global"}))
        );

        let before = rows(&db);
        let err = ops::switch(&store, &p, "cn", false).unwrap_err();
        assert!(
            format!("{err:#}").contains("remove them from opencode"),
            "{err:#}"
        );
        assert_eq!(rows(&db), before);
    }

    #[test]
    fn minimax_puts_its_region_in_the_environment() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::open(&tmp.path().join("store"));
        let p = minimax(tmp.path(), &[]);
        sign_in(&store, &p, "cn", "mm-cn", json!({"region": "cn"}));
        assert_eq!(
            p.env_line(&store.get("minimax", "cn").unwrap().unwrap())
                .unwrap(),
            "export MINIMAX_API_KEY='mm-cn'\nexport MINIMAX_REGION='cn'"
        );
    }

    #[test]
    fn openrouter_is_switched_by_the_shell_with_its_management_key() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::open(&tmp.path().join("store"));
        let p = ApiKey::openrouter().with(None, |_| None);
        let fields = p.begin_login(false).unwrap().fields();
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].name, "management");
        assert!(fields[0].choices.is_empty());
        sign_in(&store, &p, "full", "or-1", json!({"management": " mg-1 "}));
        sign_in(&store, &p, "bare", "or-2", json!({"management": ""}));
        let full = store.get("openrouter", "full").unwrap().unwrap();
        let bare = store.get("openrouter", "bare").unwrap().unwrap();
        assert_eq!(full.creds, json!({"key": "or-1", "management": "mg-1"}));
        assert_eq!(bare.creds, json!({"key": "or-2"}));
        assert_eq!(full.meta.account_id, identity_of("or-1").account_id);
        assert_eq!(
            p.env_line(&full).unwrap(),
            "export OPENROUTER_API_KEY='or-1'\nexport OPENROUTER_MANAGEMENT_KEY='mg-1'"
        );
        assert_eq!(
            p.env_line(&bare).unwrap(),
            "export OPENROUTER_API_KEY='or-2'\nunset OPENROUTER_MANAGEMENT_KEY\nunset OPENROUTER_PROVISIONING_KEY"
        );
        let err = p.install(&full, None).unwrap_err();
        assert!(
            err.to_string()
                .contains("eval \"$(remuda env openrouter/full)\""),
            "{err}"
        );
        assert_eq!(p.live().unwrap(), None);
    }

    #[test]
    fn openrouter_keeps_the_management_key_when_the_shell_has_the_inference_key() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::open(&tmp.path().join("store"));
        let p = ApiKey::openrouter().with(None, vars(&[("OPENROUTER_KEY", "or-1")]));
        sign_in(&store, &p, "full", "or-1", json!({"management": "mg-1"}));
        assert_eq!(p.live().unwrap(), Some(json!({"key": "or-1"})));
        ops::sync_live(&store, &p).unwrap();
        assert_eq!(
            store.get("openrouter", "full").unwrap().unwrap().creds,
            json!({"key": "or-1", "management": "mg-1"})
        );
    }

    #[test]
    fn copilot_reads_the_environment_then_the_github_cli() {
        let p = ApiKey::copilot()
            .with(None, |_| None)
            .falling_back_to(|| Some("gho_cli\n".into()));
        assert_eq!(p.live().unwrap(), Some(json!({"key": "gho_cli"})));
        let p = ApiKey::copilot()
            .with(
                None,
                vars(&[
                    ("GITHUB_TOKEN", "ghp_late"),
                    ("COPILOT_QUOTA_TOKEN", "ghp_first"),
                ]),
            )
            .falling_back_to(|| Some("gho_cli".into()));
        assert_eq!(p.live().unwrap(), Some(json!({"key": "ghp_first"})));
        let p = ApiKey::copilot()
            .with(None, |_| None)
            .falling_back_to(|| Some(" ".into()));
        assert_eq!(p.live().unwrap(), None);
    }

    #[test]
    fn copilot_is_switched_by_gh_token() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::open(&tmp.path().join("store"));
        let p = ApiKey::copilot()
            .with(None, |_| None)
            .falling_back_to(|| Some("gho_work".into()));
        save(&store, &p, "work", "gho_work");
        save(&store, &p, "perso", "ghp_perso");
        assert_eq!(
            ops::sync_live(&store, &p).unwrap(),
            LiveState::Stored {
                name: "work".into(),
                synced: true
            }
        );
        let perso = store.get("copilot", "perso").unwrap().unwrap();
        assert_eq!(
            p.env_line(&perso).unwrap(),
            "unset COPILOT_QUOTA_TOKEN\nexport GH_TOKEN='ghp_perso'"
        );
        let err = ops::switch(&store, &p, "perso", false).unwrap_err();
        assert!(
            format!("{err:#}").contains("reads its key from GH_TOKEN"),
            "{err:#}"
        );
        assert_eq!(perso.meta.account_id, identity_of("ghp_perso").account_id);
    }
}
