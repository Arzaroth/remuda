use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Context, Result, bail};
use serde_json::Value;

use crate::claude::OAUTH_KEY;
use crate::fsx::{create_private_dir, read_json, update_json};
use crate::keychain::{Item, Keychain, ascii_json};

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

impl Held {
    fn touch(&self) {
        if let Ok(dir) = fs::File::open(&self.0) {
            let _ = dir.set_modified(SystemTime::now());
        }
    }
}

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
        let held = Held::take(dir)?;
        for _ in 0..10 {
            held.touch();
            let Item::Held(before) = keychain.read()? else {
                return update_json(&self.file, edit);
            };
            let mut value = parse(keychain, &before)?;
            if !value.is_object() {
                bail!("the Keychain's {} is not a JSON object", keychain.service());
            }
            let was = value.clone();
            edit(&mut value)?;
            held.touch();
            match keychain.read()? {
                Item::Held(now) if now == before => {}
                _ => {
                    std::thread::sleep(Duration::from_millis(20));
                    continue;
                }
            }
            held.touch();
            if keychain.write(&ascii_json(&value)?)? {
                return Ok(());
            }
            update_json(&self.file, |file| {
                let login = file.get(OAUTH_KEY);
                if login.is_some_and(|l| {
                    Some(l) != was.get(OAUTH_KEY) && Some(l) != value.get(OAUTH_KEY)
                }) {
                    bail!(
                        "{} holds another Claude Code login than the Keychain, which remuda does not write over",
                        self.file.display()
                    );
                }
                *file = value.clone();
                Ok(())
            })?;
            return keychain.delete();
        }
        bail!(
            "the Keychain's {} kept changing while remuda wrote it",
            keychain.service()
        )
    }

    /// The file, unless the Keychain holds the object instead.
    pub fn live_file(&self) -> Option<&Path> {
        match &self.keychain {
            Some(k) if !matches!(k.read(), Ok(Item::Missing)) => None,
            _ => Some(&self.file),
        }
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

#[cfg(test)]
pub mod testing {
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};

    use anyhow::Result;

    use crate::child::Ran;
    use crate::keychain::{Keychain, Security};

    type Hook = Box<dyn FnMut(&mut BTreeMap<String, String>) + Send>;

    /// A login Keychain in memory, answering as `security` does.
    #[derive(Default)]
    pub struct Fake {
        pub items: Mutex<BTreeMap<String, String>>,
        pub locked: Mutex<bool>,
        pub calls: Mutex<Vec<(Vec<String>, Option<String>)>>,
        pub after_read: Mutex<Option<Hook>>,
    }

    fn ran(code: i32, out: &str) -> Ran {
        Ran {
            ok: code == 0,
            code: Some(code),
            out: out.as_bytes().to_vec(),
            err: Vec::new(),
        }
    }

    fn unquote(s: &str) -> String {
        let mut out = String::new();
        let mut chars = s.chars();
        while let Some(c) = chars.next() {
            match c {
                '\\' => out.extend(chars.next()),
                c => out.push(c),
            }
        }
        out
    }

    impl Fake {
        pub fn keychain(self: &Arc<Self>, service: &str) -> Keychain {
            Keychain::new(self.clone(), service.to_owned(), "me".to_owned())
        }

        pub fn put(&self, service: &str, value: &str) {
            self.items
                .lock()
                .unwrap()
                .insert(service.to_owned(), value.to_owned());
        }

        pub fn get(&self, service: &str) -> Option<String> {
            self.items.lock().unwrap().get(service).cloned()
        }
    }

    impl Security for Fake {
        fn run(&self, args: &[&str], input: Option<&[u8]>) -> Result<Ran> {
            let input = input.map(|i| String::from_utf8(i.to_vec()).unwrap());
            self.calls.lock().unwrap().push((
                args.iter().map(|a| (*a).to_owned()).collect(),
                input.clone(),
            ));
            let locked = *self.locked.lock().unwrap();
            let mut items = self.items.lock().unwrap();
            let service = |args: &[&str]| {
                let at = args.iter().position(|a| *a == "-s").unwrap();
                args[at + 1].to_owned()
            };
            Ok(match args[0] {
                "find-generic-password" if args.contains(&"-w") => {
                    if locked {
                        ran(36, "")
                    } else {
                        let found = items.get(&service(args)).cloned();
                        if let Some(hook) = self.after_read.lock().unwrap().as_mut() {
                            hook(&mut items);
                        }
                        match found {
                            Some(v) => ran(0, &format!("{v}\n")),
                            None => ran(44, ""),
                        }
                    }
                }
                "find-generic-password" => match items.contains_key(&service(args)) {
                    true => ran(0, ""),
                    false => ran(44, ""),
                },
                "delete-generic-password" => match items.remove(&service(args)) {
                    Some(_) => ran(0, ""),
                    None => ran(44, ""),
                },
                "-i" if locked => ran(36, ""),
                "-i" => {
                    let line = input.unwrap();
                    assert!(line.len() <= 4096 && line.ends_with('\n'));
                    let line = line.trim_end_matches('\n');
                    let rest = line
                        .strip_prefix("add-generic-password -U -a \"me\" -s \"")
                        .unwrap();
                    let (svc, rest) = rest.split_once('"').unwrap();
                    let value = if let Some(q) = rest.strip_prefix(" -w \"") {
                        unquote(q.strip_suffix('"').unwrap())
                    } else {
                        let hex = rest
                            .strip_prefix(" -X \"")
                            .unwrap()
                            .strip_suffix('"')
                            .unwrap();
                        let bytes: Vec<u8> = (0..hex.len())
                            .step_by(2)
                            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
                            .collect();
                        String::from_utf8(bytes).unwrap()
                    };
                    items.insert(svc.to_owned(), value);
                    ran(0, "")
                }
                other => panic!("unexpected security {other}"),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serde_json::json;

    use super::testing::Fake;
    use super::*;
    use crate::fsx::write_json;

    const SERVICE: &str = "Claude Code-credentials";

    fn storage(tmp: &tempfile::TempDir) -> (Arc<Fake>, SecureStorage) {
        let fake = Arc::new(Fake::default());
        let storage = SecureStorage::with_keychain(
            tmp.path().join(".credentials.json"),
            fake.keychain(SERVICE),
        );
        (fake, storage)
    }

    fn held(fake: &Fake) -> Value {
        serde_json::from_str(&fake.get(SERVICE).unwrap()).unwrap()
    }

    fn mcp(n: usize) -> Value {
        let servers: serde_json::Map<String, Value> = (0..n)
            .map(|i| {
                (
                    format!("server-{i}"),
                    json!({"accessToken": format!("mcp-{i}-{}", "x".repeat(200))}),
                )
            })
            .collect();
        Value::Object(servers)
    }

    #[test]
    fn the_keychain_item_is_read_before_the_file() {
        let tmp = tempfile::tempdir().unwrap();
        let (fake, storage) = storage(&tmp);
        write_json(
            storage.file(),
            &json!({"claudeAiOauth": {"accessToken": "file"}}),
        )
        .unwrap();
        assert_eq!(
            storage.read().unwrap().unwrap()["claudeAiOauth"]["accessToken"],
            "file"
        );

        fake.put(SERVICE, r#"{"claudeAiOauth":{"accessToken":"keychain"}}"#);
        assert_eq!(
            storage.read().unwrap().unwrap()["claudeAiOauth"]["accessToken"],
            "keychain"
        );
    }

    #[test]
    fn a_locked_keychain_holding_the_login_is_refused_and_one_without_it_reads_the_file() {
        let tmp = tempfile::tempdir().unwrap();
        let (fake, storage) = storage(&tmp);
        write_json(
            storage.file(),
            &json!({"claudeAiOauth": {"accessToken": "file"}}),
        )
        .unwrap();
        *fake.locked.lock().unwrap() = true;
        assert_eq!(
            storage.read().unwrap().unwrap()["claudeAiOauth"]["accessToken"],
            "file"
        );
        storage
            .update(|v| {
                v["claudeAiOauth"]["accessToken"] = json!("new");
                Ok(())
            })
            .unwrap();
        assert_eq!(
            read_json(storage.file()).unwrap().unwrap()["claudeAiOauth"]["accessToken"],
            "new"
        );

        fake.put(SERVICE, r#"{"claudeAiOauth":{"accessToken":"keychain"}}"#);
        let err = storage.read().unwrap_err().to_string();
        assert!(err.contains("security unlock-keychain"), "{err}");
        let err = storage.update(|_| Ok(())).unwrap_err().to_string();
        assert!(err.contains("locked"), "{err}");
        assert!(!tmp.path().join(".storage-write.lock").exists());
    }

    #[test]
    fn an_update_rewrites_the_item_through_stdin_and_keeps_every_other_key() {
        let tmp = tempfile::tempdir().unwrap();
        let (fake, storage) = storage(&tmp);
        let mut start = json!({"claudeAiOauth": {"accessToken": "old"}, "mcpOAuth": mcp(3)});
        start["mcpOAuth"]["server-0"]["note"] = json!("a \"quoted\" back\\slash");
        fake.put(SERVICE, &serde_json::to_string(&start).unwrap());

        storage
            .update(|v| {
                v["claudeAiOauth"] = json!({"accessToken": "secret-new"});
                Ok(())
            })
            .unwrap();

        let now = held(&fake);
        assert_eq!(now["claudeAiOauth"]["accessToken"], "secret-new");
        assert_eq!(now["mcpOAuth"], start["mcpOAuth"]);
        assert!(!storage.file().exists());
        assert!(!tmp.path().join(".storage-write.lock").exists());
        for (args, _) in fake.calls.lock().unwrap().iter() {
            assert!(args.iter().all(|a| !a.contains("secret-new")), "{args:?}");
        }
    }

    #[test]
    fn an_item_too_long_for_stdin_moves_to_the_file_whole() {
        let tmp = tempfile::tempdir().unwrap();
        let (fake, storage) = storage(&tmp);
        let start = json!({"claudeAiOauth": {"accessToken": "old"}, "mcpOAuth": mcp(20)});
        fake.put(SERVICE, &serde_json::to_string(&start).unwrap());

        storage
            .update(|v| {
                v["claudeAiOauth"] = json!({"accessToken": "new"});
                Ok(())
            })
            .unwrap();

        assert_eq!(fake.get(SERVICE), None);
        let file = read_json(storage.file()).unwrap().unwrap();
        assert_eq!(file["claudeAiOauth"]["accessToken"], "new");
        assert_eq!(file["mcpOAuth"], start["mcpOAuth"]);
        assert_eq!(storage.read().unwrap().unwrap(), file);
    }

    #[test]
    fn with_no_item_the_file_is_updated_and_no_item_is_made() {
        let tmp = tempfile::tempdir().unwrap();
        let (fake, storage) = storage(&tmp);
        write_json(storage.file(), &json!({"mcpOAuth": {"a": 1}})).unwrap();
        storage
            .update(|v| {
                v["claudeAiOauth"] = json!({"accessToken": "new"});
                Ok(())
            })
            .unwrap();
        assert_eq!(fake.get(SERVICE), None);
        let file = read_json(storage.file()).unwrap().unwrap();
        assert_eq!(
            file,
            json!({"mcpOAuth": {"a": 1}, "claudeAiOauth": {"accessToken": "new"}})
        );
    }

    #[test]
    fn an_item_changed_while_it_was_edited_is_edited_again() {
        let tmp = tempfile::tempdir().unwrap();
        let (fake, storage) = storage(&tmp);
        fake.put(SERVICE, r#"{"claudeAiOauth":{"accessToken":"a"}}"#);
        let mut once = true;
        *fake.after_read.lock().unwrap() = Some(Box::new(move |items| {
            if std::mem::take(&mut once) {
                items.insert(
                    SERVICE.to_owned(),
                    r#"{"claudeAiOauth":{"accessToken":"b"},"x":1}"#.to_owned(),
                );
            }
        }));
        storage
            .update(|v| {
                v["y"] = json!(2);
                Ok(())
            })
            .unwrap();
        assert_eq!(
            held(&fake),
            json!({"claudeAiOauth": {"accessToken": "b"}, "x": 1, "y": 2})
        );
    }

    #[test]
    fn an_edit_that_says_changed_writes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let (fake, storage) = storage(&tmp);
        fake.put(SERVICE, r#"{"claudeAiOauth":{"accessToken":"a"}}"#);
        let err = storage
            .update(|_| Err(crate::fsx::Changed.into()))
            .unwrap_err();
        assert!(err.is::<crate::fsx::Changed>());
        assert_eq!(
            fake.get(SERVICE).unwrap(),
            r#"{"claudeAiOauth":{"accessToken":"a"}}"#
        );
    }

    #[test]
    fn a_lock_claude_code_abandoned_is_taken_over() {
        let tmp = tempfile::tempdir().unwrap();
        let (fake, storage) = storage(&tmp);
        fake.put(SERVICE, "{}");
        let lock = tmp.path().join(".storage-write.lock");
        fs::create_dir(&lock).unwrap();
        let old = SystemTime::now() - Duration::from_secs(60);
        fs::File::open(&lock).unwrap().set_modified(old).unwrap();
        storage
            .update(|v| {
                v["a"] = json!(1);
                Ok(())
            })
            .unwrap();
        assert_eq!(held(&fake), json!({"a": 1}));
        assert!(!lock.exists());
    }

    #[test]
    fn a_session_directory_has_an_item_of_its_own_and_forgets_it() {
        let tmp = tempfile::tempdir().unwrap();
        let (fake, storage) = storage(&tmp);
        let dir = tmp.path().join("session");
        let session = storage.relocated(&dir);
        let service = crate::keychain::service(Some(&dir.to_string_lossy()));
        assert_ne!(service, SERVICE);
        assert_eq!(session.file(), dir.join(".credentials.json"));
        fake.put(&service, r#"{"claudeAiOauth":{"accessToken":"s"}}"#);
        fake.put(SERVICE, "{}");
        assert_eq!(
            session.read().unwrap().unwrap()["claudeAiOauth"]["accessToken"],
            "s"
        );
        session.forget().unwrap();
        assert_eq!(fake.get(&service), None);
        assert_eq!(fake.get(SERVICE).as_deref(), Some("{}"));
    }
}
