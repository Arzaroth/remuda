use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use crate::fsx::{create_private_dir, read_json, update_json, write_json};
use crate::logbook::{self, Event, Kind};
use crate::ops;
use crate::provider::{Provider, RunSpec};
use crate::store::{Entry, Store};

fn runs(store: &Store) -> PathBuf {
    store.root().join(".runs")
}

/// `<store>/.runs/<provider>/<name>/`: the session's `lock`, the CLI's
/// directory as the session sees it (`home`), and what its copied files held
/// when it started (`base`).
fn place(store: &Store, provider: &str, name: &str) -> PathBuf {
    runs(store).join(provider).join(name)
}

pub fn home(store: &Store, provider: &str, name: &str) -> PathBuf {
    place(store, provider, name).join("home")
}

fn base(store: &Store, provider: &str, name: &str) -> PathBuf {
    place(store, provider, name).join("base")
}

/// Held for as long as a session runs on a login, and released by the kernel
/// when its process dies, however it dies.
pub struct Lease {
    _file: File,
}

/// The lock on `path` if nothing else holds it and it is still the file at
/// `path`: a lock taken on a file another process removed meanwhile guards
/// nothing.
fn grab(path: &Path, create: bool) -> Result<Option<File>> {
    if create && let Some(dir) = path.parent() {
        create_private_dir(dir).with_context(|| format!("failed to create {}", dir.display()))?;
    }
    let file = match OpenOptions::new()
        .read(true)
        .write(create)
        .create(create)
        .truncate(false)
        .mode(0o600)
        .open(path)
    {
        Ok(f) => f,
        Err(e) if !create && e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("failed to open {}", path.display())),
    };
    match file.try_lock() {
        Ok(()) => {}
        Err(fs::TryLockError::WouldBlock) => return Ok(None),
        Err(fs::TryLockError::Error(e)) => {
            return Err(e).with_context(|| format!("failed to lock {}", path.display()));
        }
    }
    let held = file.metadata()?;
    let still = fs::metadata(path).is_ok_and(|m| m.ino() == held.ino() && m.dev() == held.dev());
    Ok(still.then_some(file))
}

/// Whether a `remuda run` session holds the login now.
pub fn in_use(store: &Store, provider: &str, name: &str) -> bool {
    let path = place(store, provider, name).join("lock");
    path.exists() && !matches!(grab(&path, false), Ok(Some(_)))
}

pub fn lease(store: &Store, provider: &str, name: &str) -> Result<Lease> {
    match grab(&place(store, provider, name).join("lock"), true)? {
        Some(file) => Ok(Lease { _file: file }),
        None => bail!("{provider}/{name} is already in a `remuda run` session"),
    }
}

fn without(value: Option<Value>, keys: &[&str]) -> Value {
    let mut value = value.filter(Value::is_object).unwrap_or_else(|| json!({}));
    if let Some(map) = value.as_object_mut() {
        for key in keys {
            map.remove(*key);
        }
    }
    value
}

/// Builds the session's directory: every entry of the CLI's own linked to,
/// the copied files without the login's keys, then the login installed into
/// them as a switch would.
pub fn prepare(store: &Store, p: &dyn Provider, spec: &RunSpec, entry: &Entry) -> Result<PathBuf> {
    let home = home(store, p.id(), &entry.name);
    let base = base(store, p.id(), &entry.name);
    for dir in [&home, &base] {
        if dir.exists() {
            fs::remove_dir_all(dir)
                .with_context(|| format!("failed to clear {}", dir.display()))?;
        }
        create_private_dir(dir).with_context(|| format!("failed to create {}", dir.display()))?;
    }
    let real = p.home();
    let own: Vec<&str> = spec
        .private
        .iter()
        .copied()
        .chain(spec.copied.iter().map(|c| c.name))
        .collect();
    for item in fs::read_dir(&real).into_iter().flatten().flatten() {
        let name = item.file_name();
        if own.iter().any(|o| name == *o) {
            continue;
        }
        std::os::unix::fs::symlink(item.path(), home.join(&name))
            .with_context(|| format!("failed to link {}", item.path().display()))?;
    }
    for c in &spec.copied {
        let start = without(read_json(&c.from)?, c.login_keys);
        write_json(&base.join(c.name), &start)?;
        write_json(&home.join(c.name), &start)?;
    }
    let session = p
        .relocated(&home)
        .with_context(|| format!("{} cannot run in a directory of its own", p.name()))?;
    session.install(entry, None)?;
    Ok(home)
}

/// Writes into `theirs` what the session changed from `base` and the CLI's
/// own copy did not, key by key and one level down (`projects.<dir>`,
/// `mcpServers.<name>`); where both changed a key, the CLI's copy wins.
fn merge(base: &Value, ours: &Value, theirs: &mut Value, skip: &[&str], deep: bool) -> bool {
    let (Some(b), Some(o)) = (base.as_object(), ours.as_object()) else {
        return false;
    };
    if !theirs.is_object() {
        return false;
    }
    let keys: BTreeSet<&String> = b.keys().chain(o.keys()).collect();
    let mut changed = false;
    for key in keys {
        if skip.contains(&key.as_str()) {
            continue;
        }
        let (was, now) = (b.get(key), o.get(key));
        if was == now {
            continue;
        }
        let map = theirs.as_object_mut().expect("checked above");
        if map.get(key) == was {
            match now {
                Some(v) => map.insert(key.clone(), v.clone()),
                None => map.remove(key),
            };
            changed = true;
        } else if deep && let (Some(was), Some(now), Some(theirs)) = (was, now, map.get_mut(key)) {
            changed |= merge(was, now, theirs, &[], false);
        }
    }
    changed
}

fn merge_back(spec: &RunSpec, home: &Path, base: &Path) -> Result<()> {
    for c in &spec.copied {
        let Some(ours) = read_json(&home.join(c.name))? else {
            continue;
        };
        let was = read_json(&base.join(c.name))?.unwrap_or_else(|| json!({}));
        let mut probe = without(read_json(&c.from)?, &[]);
        if !merge(&was, &ours, &mut probe, c.login_keys, true) {
            continue;
        }
        update_json(&c.from, |theirs| {
            merge(&was, &ours, theirs, c.login_keys, true);
            Ok(())
        })?;
    }
    Ok(())
}

/// Files the tokens the session's CLI ends with under the login it started
/// on, once they are known to be that account's: the same refresh token, or
/// the account the provider names for them. Anything else is set aside, and
/// said why.
fn sync_back(
    store: &Store,
    p: &dyn Provider,
    session: &dyn Provider,
    name: &str,
) -> Result<Option<String>> {
    let Some(creds) = session.live()? else {
        return Ok(None);
    };
    let Some(mut entry) = store.get(p.id(), name)? else {
        let kept = store.set_aside(p.id(), "unknown", &creds)?;
        return Ok(Some(format!(
            "{}/{name} was removed during the session; its tokens are kept in {}",
            p.id(),
            kept.display()
        )));
    };
    if entry.creds == creds {
        return Ok(None);
    }
    let refresh = p.refresh_token(&creds);
    let same_line = refresh.is_some() && refresh == p.refresh_token(&entry.creds);
    if !same_line {
        match session.identify(&creds) {
            Ok(id) if id.account_id == entry.meta.account_id => {}
            Ok(id) => {
                let kept = store.set_aside(p.id(), &id.account_id, &creds)?;
                return Ok(Some(format!(
                    "the session ended signed in to another account ({}), kept in {}",
                    if id.email.is_empty() {
                        &id.account_id
                    } else {
                        &id.email
                    },
                    kept.display()
                )));
            }
            Err(e) => {
                let kept = store.set_aside(p.id(), &entry.meta.account_id, &creds)?;
                return Ok(Some(format!(
                    "could not confirm whose tokens the session ended with ({e:#}); they are kept in {}",
                    kept.display()
                )));
            }
        }
    }
    let older = matches!(
        (p.expires_at(&creds), p.expires_at(&entry.creds)),
        (Some(ended), Some(stored)) if ended < stored
    );
    if !older {
        entry.creds = creds;
        store.save(&entry)?;
    }
    Ok(None)
}

/// Ends a session: its tokens back into the store, its edits to the copied
/// files back into the CLI's, and its directory gone. Called under the store
/// lock with the session's lease held. A directory whose tokens could not be
/// read is kept, for the next command to try again.
pub fn finish(store: &Store, p: &dyn Provider, spec: &RunSpec, name: &str) -> Result<()> {
    let home = home(store, p.id(), name);
    let base = base(store, p.id(), name);
    let synced = match p.relocated(&home) {
        Some(session) if home.exists() => sync_back(store, p, session.as_ref(), name)?,
        _ => None,
    };
    let merged = if home.exists() {
        merge_back(spec, &home, &base)
    } else {
        Ok(())
    };
    let place = place(store, p.id(), name);
    for dir in [&home, &base] {
        if dir.exists() {
            fs::remove_dir_all(dir)
                .with_context(|| format!("failed to remove {}", dir.display()))?;
        }
    }
    let _ = fs::remove_file(place.join("lock"));
    let _ = fs::remove_dir(&place);
    if let Some(why) = synced {
        logbook::note(
            store,
            Event::new(Kind::Run, false, &why).about(p.id(), Some(name)),
        );
        bail!("{why}");
    }
    merged
}

/// Finishes the sessions whose process died before it could, which the
/// lock they no longer hold gives away. Called under the store lock.
pub fn recover(store: &Store, providers: &[&dyn Provider]) {
    for p in providers {
        let Some(spec) = p.run_spec() else {
            continue;
        };
        let dir = runs(store).join(p.id());
        for item in fs::read_dir(&dir).into_iter().flatten().flatten() {
            let Some(name) = item.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            let Ok(Some(_lease)) = grab(&item.path().join("lock"), false) else {
                continue;
            };
            match finish(store, *p, &spec, &name) {
                Ok(()) => logbook::note(
                    store,
                    Event::new(
                        Kind::Run,
                        true,
                        "finished a session that ended without remuda",
                    )
                    .about(p.id(), Some(&name)),
                ),
                Err(e) => eprintln!(
                    "warning: a session on {}/{name} ended without remuda: {e:#}",
                    p.id()
                ),
            }
        }
    }
}

/// Starts a session: the lease, a refresh if the login is about to expire,
/// and the directory. Called under the store lock.
pub fn start(store: &Store, p: &dyn Provider, name: &str) -> Result<(Lease, RunSpec, PathBuf)> {
    let spec = p
        .run_spec()
        .with_context(|| format!("remuda run cannot start {} yet", p.name()))?;
    let mut entry = store
        .get(p.id(), name)?
        .with_context(|| format!("no credential named {}/{name}", p.id()))?;
    if !entry.verified {
        bail!(
            "{} does not match its sidecar; run `remuda ls` online to identify it first",
            entry.qualified()
        );
    }
    let lease = lease(store, p.id(), name)?;
    if p.renews() && ops::expires_in_ms(p, &entry).is_some_and(|ms| ms < 5 * 60 * 1000) {
        ops::refresh_saved(store, p, &mut entry)?;
    }
    let home = match prepare(store, p, &spec, &entry) {
        Ok(home) => home,
        Err(e) => {
            let _ = finish(store, p, &spec, name);
            return Err(e);
        }
    };
    logbook::note(
        store,
        Event::new(Kind::Run, true, "started a session").about(p.id(), Some(name)),
    );
    Ok((lease, spec, home))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::claude::{Api, Claude, OAUTH_KEY};
    use crate::ops::testing::*;

    fn read(path: &Path) -> Value {
        read_json(path).unwrap().unwrap()
    }

    #[test]
    fn a_lease_is_held_until_dropped_and_seen_from_elsewhere() {
        let e = env(&OFFLINE);
        assert!(!in_use(&e.store, "claude", "work"));
        let lease = lease(&e.store, "claude", "work").unwrap();
        assert!(in_use(&e.store, "claude", "work"));
        assert!(!in_use(&e.store, "claude", "perso"));
        let err = super::lease(&e.store, "claude", "work").err().unwrap();
        assert!(
            err.to_string()
                .contains("already in a `remuda run` session")
        );
        drop(lease);
        assert!(!in_use(&e.store, "claude", "work"));
    }

    #[test]
    fn a_session_links_the_cli_s_files_and_keeps_only_the_login_its_own() {
        let e = env(&OFFLINE);
        e.stored("work", "u-work", HOUR);
        e.sign_in(oauth("a-live", "r-live", HOUR), account("u-live"));
        let mut creds = read(&e.tmp.path().join(".credentials.json"));
        creds["mcpOAuth"] = json!({"srv": {"accessToken": "mcp"}});
        write_json(&e.tmp.path().join(".credentials.json"), &creds).unwrap();
        fs::write(e.tmp.path().join("settings.json"), "{}").unwrap();
        fs::create_dir(e.tmp.path().join("projects")).unwrap();

        let (lease, spec, home) = start(&e.store, &e.claude, "work").unwrap();

        assert!(home.join("settings.json").is_symlink());
        assert!(home.join("projects").is_symlink());
        assert!(!home.join(".credentials.json").is_symlink());
        let session = read(&home.join(".credentials.json"));
        assert_eq!(session[OAUTH_KEY]["refreshToken"], "r-work");
        assert_eq!(session["mcpOAuth"]["srv"]["accessToken"], "mcp");
        assert_eq!(
            read(&home.join(".claude.json"))["oauthAccount"]["accountUuid"],
            "u-work"
        );
        let base = read(&base(&e.store, "claude", "work").join(".credentials.json"));
        assert!(
            base.get(OAUTH_KEY).is_none(),
            "no other login's tokens are copied"
        );
        assert_eq!(e.live_refresh_token().as_deref(), Some("r-live"));
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(&home).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);

        finish(&e.store, &e.claude, &spec, "work").unwrap();
        drop(lease);
        assert!(!place(&e.store, "claude", "work").exists());
        assert!(
            e.tmp.path().join("settings.json").exists(),
            "links, not files, are removed"
        );
        assert_eq!(e.live_refresh_token().as_deref(), Some("r-live"));
    }

    #[test]
    fn tokens_the_session_rotated_go_back_to_the_store() {
        let mut server = mockito::Server::new();
        server
            .mock("GET", "/api/oauth/profile")
            .with_body(profile_body("u-work"))
            .create();
        let e = env(&server.url());
        e.stored("work", "u-work", HOUR);
        let claude = Claude::at(e.tmp.path(), Api::local(&server.url()));
        let (_lease, spec, home) = start(&e.store, &claude, "work").unwrap();

        let session = Claude::at(&home, Api::local(&server.url()));
        let mut creds = read(&home.join(".credentials.json"));
        creds[OAUTH_KEY]["refreshToken"] = json!("r-rotated");
        creds[OAUTH_KEY]["accessToken"] = json!("a-rotated");
        creds[OAUTH_KEY]["expiresAt"] = json!(crate::fsx::now_ms() + 2 * HOUR);
        write_json(&home.join(".credentials.json"), &creds).unwrap();
        assert!(session.live().unwrap().is_some());

        finish(&e.store, &claude, &spec, "work").unwrap();
        assert_eq!(e.stored_refresh_token("work").as_deref(), Some("r-rotated"));
    }

    #[test]
    fn tokens_of_another_account_or_unconfirmed_ones_are_set_aside() {
        let e = env(&OFFLINE);
        e.stored("work", "u-work", HOUR);
        let (_lease, spec, home) = start(&e.store, &e.claude, "work").unwrap();
        let mut creds = read(&home.join(".credentials.json"));
        creds[OAUTH_KEY]["refreshToken"] = json!("r-unknown");
        write_json(&home.join(".credentials.json"), &creds).unwrap();

        let err = finish(&e.store, &e.claude, &spec, "work").unwrap_err();
        assert!(err.to_string().contains("kept in"), "{err}");
        assert_eq!(e.stored_refresh_token("work").as_deref(), Some("r-work"));
        let aside = fs::read_dir(e.store.root().join("claude"))
            .unwrap()
            .flatten()
            .any(|f| f.file_name().to_string_lossy().starts_with(".set-aside-"));
        assert!(aside);
        assert!(
            !home.exists(),
            "tokens set aside are safe, so the directory goes"
        );
    }

    #[test]
    fn what_the_session_changed_in_claude_json_is_merged_back_but_its_account() {
        let e = env(&OFFLINE);
        e.stored("work", "u-work", HOUR);
        e.sign_in(oauth("a-live", "r-live", HOUR), account("u-live"));
        let config = e.tmp.path().join(".claude.json");
        let mut real = read(&config);
        real["projects"] = json!({"/a": {"trusted": false}, "/b": {"n": 1}});
        real["tips"] = json!(1);
        write_json(&config, &real).unwrap();
        let (_lease, spec, home) = start(&e.store, &e.claude, "work").unwrap();

        let mut ours = read(&home.join(".claude.json"));
        ours["projects"]["/a"] = json!({"trusted": true});
        ours["projects"]["/c"] = json!({"new": true});
        ours["tips"] = json!(2);
        ours["mcpServers"] = json!({"x": {}});
        write_json(&home.join(".claude.json"), &ours).unwrap();
        let mut theirs = read(&config);
        theirs["tips"] = json!(5);
        theirs["projects"]["/b"] = json!({"n": 2});
        write_json(&config, &theirs).unwrap();

        finish(&e.store, &e.claude, &spec, "work").unwrap();
        let merged = read(&config);
        assert_eq!(merged["oauthAccount"]["accountUuid"], "u-live");
        assert_eq!(merged["projects"]["/a"]["trusted"], true);
        assert_eq!(merged["projects"]["/b"]["n"], 2);
        assert_eq!(merged["projects"]["/c"]["new"], true);
        assert_eq!(
            merged["tips"], 5,
            "where both changed a key, the CLI's copy wins"
        );
        assert!(merged["mcpServers"].get("x").is_some());
    }

    #[test]
    fn a_session_whose_process_died_is_finished_by_the_next_command() {
        let e = env(&OFFLINE);
        e.stored("work", "u-work", HOUR);
        let (lease, _spec, home) = start(&e.store, &e.claude, "work").unwrap();
        recover(&e.store, &[&e.claude]);
        assert!(home.exists(), "a live session is left alone");
        drop(lease);
        recover(&e.store, &[&e.claude]);
        assert!(!home.exists());
        let logged = logbook::read(&e.store);
        assert_eq!(
            logged.last().unwrap().message,
            "finished a session that ended without remuda"
        );
    }

    #[test]
    fn a_login_already_in_a_session_or_unknown_cannot_start_another() {
        let e = env(&OFFLINE);
        e.stored("work", "u-work", HOUR);
        let _held = start(&e.store, &e.claude, "work").unwrap();
        assert!(start(&e.store, &e.claude, "work").is_err());
        assert!(start(&e.store, &e.claude, "ghost").is_err());
    }
}
