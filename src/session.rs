use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use crate::fsx::{create_private_dir, now_ms, read_json, update_json, write_json};
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

/// A shared lock on the session, which every process of it holds: remuda,
/// and the CLI it starts, which inherits it. The session ends when the last
/// of them lets go, however each one ends.
pub struct Lease {
    file: File,
}

impl Lease {
    pub fn fd(&self) -> RawFd {
        self.file.as_raw_fd()
    }
}

fn open_lock(path: &Path, create: bool) -> Result<Option<File>> {
    if create && let Some(dir) = path.parent() {
        create_private_dir(dir).with_context(|| format!("failed to create {}", dir.display()))?;
    }
    match OpenOptions::new()
        .read(true)
        .write(create)
        .create(create)
        .truncate(false)
        .mode(0o600)
        .open(path)
    {
        Ok(f) => Ok(Some(f)),
        Err(e) if !create && e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("failed to open {}", path.display())),
    }
}

/// A lock taken on a file another process removed meanwhile guards nothing.
fn still_at(file: &File, path: &Path) -> bool {
    let Ok(held) = file.metadata() else {
        return false;
    };
    fs::metadata(path).is_ok_and(|m| m.ino() == held.ino() && m.dev() == held.dev())
}

/// Whether a `remuda run` session holds the login: one running, or one
/// that ended and could not be finished yet, whose directory then holds its
/// newest tokens.
pub fn in_use(store: &Store, provider: &str, name: &str) -> bool {
    place(store, provider, name).exists()
}

fn running(place: &Path) -> bool {
    let Ok(Some(file)) = open_lock(&place.join("lock"), false) else {
        return false;
    };
    matches!(file.try_lock(), Err(fs::TryLockError::WouldBlock))
}

/// The exclusive lock on a session no process holds any more, for finishing
/// it.
fn idle(path: &Path) -> Result<Option<File>> {
    let Some(file) = open_lock(path, false)? else {
        return Ok(None);
    };
    match file.try_lock() {
        Ok(()) if still_at(&file, path) => Ok(Some(file)),
        Ok(()) | Err(fs::TryLockError::WouldBlock) => Ok(None),
        Err(fs::TryLockError::Error(e)) => {
            Err(e).with_context(|| format!("failed to lock {}", path.display()))
        }
    }
}

/// Joins the session, creating its lock if it has none. Waits only on
/// `in_use`'s probes, which hold the lock for an instant.
fn join(path: &Path) -> Result<Lease> {
    for _ in 0..3 {
        let file = open_lock(path, true)?.context("the lock vanished")?;
        file.lock_shared()
            .with_context(|| format!("failed to lock {}", path.display()))?;
        if still_at(&file, path) {
            return Ok(Lease { file });
        }
    }
    bail!("{} keeps being replaced", path.display())
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

fn own_names(spec: &RunSpec) -> Vec<&'static str> {
    spec.private
        .iter()
        .copied()
        .chain(spec.copied.iter().map(|c| c.name))
        .collect()
}

/// Builds the session's directory: every entry of the CLI's own linked to,
/// the copied files without the login's keys, then the login installed into
/// them as a switch would.
fn prepare(store: &Store, p: &dyn Provider, spec: &RunSpec, entry: &Entry) -> Result<()> {
    let home = home(store, p.id(), &entry.name);
    let base = base(store, p.id(), &entry.name);
    for dir in [&home, &base] {
        create_private_dir(dir).with_context(|| format!("failed to create {}", dir.display()))?;
    }
    fs::write(base.join(".started"), now_ms().to_string())?;
    let own = own_names(spec);
    for item in fs::read_dir(p.home()).into_iter().flatten().flatten() {
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
    session.install(entry, None)
}

fn expires_at(v: Option<&Value>) -> Option<i64> {
    v?.get("expiresAt")?.as_i64()
}

/// Writes into `theirs` what the session changed from `base` and the CLI's
/// own copy did not, key by key and one level down (`projects.<dir>`,
/// `mcpOAuth.<server>`). Where both changed a key, the CLI's copy wins, but
/// for tokens: the one that expires later is the one a refresh rotated to.
fn merge(base: &Value, ours: &Value, theirs: &mut Value, skip: &[&str], deep: bool) -> bool {
    let empty = serde_json::Map::new();
    let b = base.as_object().unwrap_or(&empty);
    let Some(o) = ours.as_object() else {
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
        let Some(map) = theirs.as_object_mut() else {
            return changed;
        };
        let current = map.get(key);
        let rotated = expires_at(now).is_some_and(|n| expires_at(current).is_none_or(|c| n > c));
        if current == was || rotated {
            match now {
                Some(v) => map.insert(key.clone(), v.clone()),
                None => map.remove(key),
            };
            changed = true;
        } else if deep && let (Some(now), Some(theirs)) = (now, map.get_mut(key)) {
            changed |= merge(was.unwrap_or(&Value::Null), now, theirs, &[], false);
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

fn move_file(from: &Path, to: &Path) -> Result<()> {
    if fs::rename(from, to).is_ok() {
        return Ok(());
    }
    if !fs::symlink_metadata(from)?.is_file() {
        bail!("cannot move {} to {}", from.display(), to.display());
    }
    fs::copy(from, to).with_context(|| format!("failed to copy {}", from.display()))?;
    fs::remove_file(from)?;
    Ok(())
}

/// What the CLI made in the session's directory beside the links: entries
/// the CLI's own directory did not have, and files it rewrote by rename,
/// which replaced their link. They go into the CLI's own directory, a
/// rewritten file only if nothing changed it there since the session
/// started, and beside it as `<name>.remuda-run` otherwise.
fn move_back(p: &dyn Provider, spec: &RunSpec, home: &Path, base: &Path) -> Result<Vec<PathBuf>> {
    let started = fs::read_to_string(base.join(".started"))
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .map_or(UNIX_EPOCH, |ms| UNIX_EPOCH + Duration::from_millis(ms));
    let real = p.home();
    let own = own_names(spec);
    let mut kept = Vec::new();
    for item in fs::read_dir(home)?.flatten() {
        let name = item.file_name();
        if own.iter().any(|o| name == *o) || item.file_type()?.is_symlink() {
            continue;
        }
        create_private_dir(&real)
            .with_context(|| format!("failed to create {}", real.display()))?;
        let target = real.join(&name);
        match fs::symlink_metadata(&target) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                move_file(&item.path(), &target)?;
            }
            Ok(m) if m.is_file() && m.modified().is_ok_and(|t| t <= started) => {
                move_file(&item.path(), &target)?;
            }
            _ => {
                let aside = real.join(format!("{}.remuda-run", name.to_string_lossy()));
                move_file(&item.path(), &aside)?;
                kept.push(aside);
            }
        }
    }
    Ok(kept)
}

/// Files the tokens the session's CLI ends with under the login it started
/// on, once they are known to be that account's: the same refresh token, or
/// the account the provider names for them. Another account's, or ones the
/// store has newer than, are set aside, and said. Ones that cannot be
/// identified yet are an error, which keeps the directory for the next
/// command to try.
fn sync_back(
    store: &Store,
    p: &dyn Provider,
    session: &dyn Provider,
    name: &str,
) -> Result<Option<String>> {
    let Some(creds) = session.live()? else {
        return Ok(Some(format!(
            "the session ended signed out; if its {} refreshed first, sign {}/{name} in again",
            p.name(),
            p.id()
        )));
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
        let id = session.identify(&creds).with_context(|| {
            format!(
                "could not confirm whose tokens the session on {}/{name} ended with; the next command tries again",
                p.id()
            )
        })?;
        if id.account_id != entry.meta.account_id {
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
    }
    let older = matches!(
        (p.expires_at(&creds), p.expires_at(&entry.creds)),
        (Some(ended), Some(stored)) if ended < stored
    );
    if older && same_line {
        return Ok(None);
    }
    if older {
        let kept = store.set_aside(p.id(), &entry.meta.account_id, &creds)?;
        return Ok(Some(format!(
            "the store holds newer tokens for {}/{name} than the session ended with; the session's are kept in {}",
            p.id(),
            kept.display()
        )));
    }
    entry.creds = creds;
    store.save(&entry)?;
    Ok(None)
}

/// Ends a session no process holds any more: its tokens back into the store,
/// what it made back into the CLI's directory, its edits to the copied files
/// back into the CLI's, and its directory gone. Called under the store lock
/// with the session's exclusive lock held. Anything that fails keeps the
/// directory, for the next command to try again.
fn finish(store: &Store, p: &dyn Provider, spec: &RunSpec, name: &str) -> Result<()> {
    let home = home(store, p.id(), name);
    let base = base(store, p.id(), name);
    let mut said = Vec::new();
    if home.exists() {
        let session = p
            .relocated(&home)
            .with_context(|| format!("{} cannot run in a directory of its own", p.name()))?;
        said.extend(sync_back(store, p, session.as_ref(), name)?);
        for aside in move_back(p, spec, &home, &base)? {
            said.push(format!(
                "the session's copy of a file changed meanwhile is kept as {}",
                aside.display()
            ));
        }
        merge_back(spec, &home, &base)?;
    }
    for dir in [&home, &base] {
        if dir.exists() {
            fs::remove_dir_all(dir)
                .with_context(|| format!("failed to remove {}", dir.display()))?;
        }
    }
    let place = place(store, p.id(), name);
    let _ = fs::remove_file(place.join("lock"));
    let _ = fs::remove_dir(&place);
    for why in &said {
        eprintln!("warning: {why}");
        logbook::note(
            store,
            Event::new(Kind::Run, false, why).about(p.id(), Some(name)),
        );
    }
    Ok(())
}

/// Finishes the provider's sessions no process holds any more. Called under
/// the store lock.
pub fn recover(store: &Store, providers: &[&dyn Provider]) {
    for p in providers {
        let Some(spec) = p.run_spec() else {
            continue;
        };
        for item in fs::read_dir(runs(store).join(p.id()))
            .into_iter()
            .flatten()
            .flatten()
        {
            let Some(name) = item.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            let Ok(Some(_last)) = idle(&item.path().join("lock")) else {
                continue;
            };
            match finish(store, *p, &spec, &name) {
                Ok(()) => logbook::note(
                    store,
                    Event::new(Kind::Run, true, "the session ended").about(p.id(), Some(&name)),
                ),
                Err(e) => {
                    let why = format!(
                        "the session on {}/{name} could not be finished: {e:#}",
                        p.id()
                    );
                    eprintln!("warning: {why}");
                    logbook::note_once(
                        store,
                        Event::new(Kind::Run, false, why).about(p.id(), Some(&name)),
                    );
                }
            }
        }
    }
}

/// Starts a session on the login, or joins the one running on it. Called
/// under the store lock, after `ops::sync_live` finished the sessions whose
/// processes had all ended.
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
    let place = place(store, p.id(), name);
    let home = home(store, p.id(), name);
    if place.exists() {
        if !running(&place) {
            bail!(
                "the last session on {}/{name} could not be finished, and its newest tokens are in {}; `remuda ls` tries again and says why",
                p.id(),
                place.display()
            );
        }
        let lease = join(&place.join("lock"))?;
        return Ok((lease, spec, home));
    }
    let lease = join(&place.join("lock"))?;
    if p.renews() && ops::expires_in_ms(p, &entry).is_some_and(|ms| ms < ops::REFRESH_BEFORE_MS) {
        ops::refresh_saved(store, p, &mut entry)?;
    }
    if let Err(e) = prepare(store, p, &spec, &entry) {
        let _ = fs::remove_dir_all(&place);
        return Err(e);
    }
    logbook::note(
        store,
        Event::new(Kind::Run, true, "started a session").about(p.id(), Some(name)),
    );
    Ok((lease, spec, home))
}

/// Lets go of the session, and finishes it if nothing else holds it. Called
/// under the store lock, once the CLI has exited.
pub fn leave(store: &Store, p: &dyn Provider, lease: Lease) {
    drop(lease);
    recover(store, &[p]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::claude::{Api, Claude, OAUTH_KEY};
    use crate::ops::testing::*;

    fn read(path: &Path) -> Value {
        read_json(path).unwrap().unwrap()
    }

    fn rotate(home: &Path, refresh: &str, access: &str) {
        let mut creds = read(&home.join(".credentials.json"));
        creds[OAUTH_KEY]["refreshToken"] = json!(refresh);
        creds[OAUTH_KEY]["accessToken"] = json!(access);
        creds[OAUTH_KEY]["expiresAt"] = json!(now_ms() + 2 * HOUR);
        write_json(&home.join(".credentials.json"), &creds).unwrap();
    }

    fn profile(server: &mut mockito::ServerGuard, access: &str, uuid: &str) {
        server
            .mock("GET", "/api/oauth/profile")
            .match_header("authorization", format!("Bearer {access}").as_str())
            .with_body(profile_body(uuid))
            .create();
    }

    fn set_aside_count(e: &Env) -> usize {
        fs::read_dir(e.store.root().join("claude"))
            .unwrap()
            .flatten()
            .filter(|f| f.file_name().to_string_lossy().starts_with(".set-aside-"))
            .count()
    }

    #[test]
    fn a_session_is_held_by_every_process_in_it_and_ends_with_the_last() {
        let e = env(&OFFLINE);
        e.stored("work", "u-work", HOUR);
        assert!(!in_use(&e.store, "claude", "work"));
        let (first, _, home) = start(&e.store, &e.claude, "work").unwrap();
        assert!(in_use(&e.store, "claude", "work"));
        assert!(running(&place(&e.store, "claude", "work")));
        assert!(!in_use(&e.store, "claude", "perso"));
        let (second, _, joined) = start(&e.store, &e.claude, "work").unwrap();
        assert_eq!(joined, home, "a second terminal joins the session");

        leave(&e.store, &e.claude, first);
        assert!(home.exists(), "the other process still runs in it");
        assert!(in_use(&e.store, "claude", "work"));
        leave(&e.store, &e.claude, second);
        assert!(!home.exists());
        assert!(!in_use(&e.store, "claude", "work"));
        assert!(start(&e.store, &e.claude, "ghost").is_err());
    }

    #[test]
    fn a_session_links_the_cli_s_files_and_keeps_only_the_login_its_own() {
        let e = env(&OFFLINE);
        e.stored("work", "u-work", HOUR);
        e.sign_in(oauth("a-live", "r-live", HOUR), account("u-live"));
        let mut creds = read(&e.tmp.path().join(".credentials.json"));
        creds["mcpOAuth"] = json!({"srv": {"accessToken": "mcp"}});
        write_json(&e.tmp.path().join(".credentials.json"), &creds).unwrap();
        let mut config = read(&e.tmp.path().join(".claude.json"));
        config["primaryApiKey"] = json!("sk-console");
        write_json(&e.tmp.path().join(".claude.json"), &config).unwrap();
        fs::write(e.tmp.path().join("settings.json"), "{}").unwrap();
        fs::create_dir(e.tmp.path().join("projects")).unwrap();

        let (lease, _, home) = start(&e.store, &e.claude, "work").unwrap();

        assert!(home.join("settings.json").is_symlink());
        assert!(home.join("projects").is_symlink());
        assert!(!home.join(".credentials.json").is_symlink());
        let session = read(&home.join(".credentials.json"));
        assert_eq!(session[OAUTH_KEY]["refreshToken"], "r-work");
        assert_eq!(session["mcpOAuth"]["srv"]["accessToken"], "mcp");
        let session_config = read(&home.join(".claude.json"));
        assert_eq!(session_config["oauthAccount"]["accountUuid"], "u-work");
        assert!(session_config.get("primaryApiKey").is_none());
        let base = read(&base(&e.store, "claude", "work").join(".credentials.json"));
        assert!(
            base.get(OAUTH_KEY).is_none(),
            "no other login's tokens are copied"
        );
        assert_eq!(e.live_refresh_token().as_deref(), Some("r-live"));
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(&home).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);

        fs::write(home.join("todos.json"), "made in the session").unwrap();
        leave(&e.store, &e.claude, lease);
        assert!(!place(&e.store, "claude", "work").exists());
        assert!(
            e.tmp.path().join("settings.json").exists(),
            "links, not files, are removed"
        );
        assert_eq!(
            fs::read_to_string(e.tmp.path().join("todos.json")).unwrap(),
            "made in the session"
        );
        assert_eq!(e.live_refresh_token().as_deref(), Some("r-live"));
        assert_eq!(
            read(&e.tmp.path().join(".claude.json"))["primaryApiKey"],
            "sk-console"
        );
    }

    #[test]
    fn a_file_the_cli_rewrote_by_rename_goes_back_unless_it_changed_outside() {
        let e = env(&OFFLINE);
        e.stored("work", "u-work", HOUR);
        fs::write(e.tmp.path().join("config.toml"), "outside").unwrap();
        fs::write(e.tmp.path().join("other.toml"), "outside").unwrap();
        std::thread::sleep(Duration::from_millis(20));
        let (lease, _, home) = start(&e.store, &e.claude, "work").unwrap();
        std::thread::sleep(Duration::from_millis(20));
        for name in ["config.toml", "other.toml"] {
            fs::remove_file(home.join(name)).unwrap();
            fs::write(home.join(name), "session").unwrap();
        }
        fs::write(e.tmp.path().join("other.toml"), "changed outside").unwrap();

        leave(&e.store, &e.claude, lease);
        let real = |n: &str| fs::read_to_string(e.tmp.path().join(n)).unwrap();
        assert_eq!(real("config.toml"), "session");
        assert_eq!(real("other.toml"), "changed outside");
        assert_eq!(real("other.toml.remuda-run"), "session");
    }

    #[test]
    fn tokens_the_session_rotated_go_back_to_the_store() {
        let mut server = mockito::Server::new();
        profile(&mut server, "a-rotated", "u-work");
        let e = env(&server.url());
        e.stored("work", "u-work", HOUR);
        let claude = Claude::at(e.tmp.path(), Api::local(&server.url()));
        let (lease, _, home) = start(&e.store, &claude, "work").unwrap();
        rotate(&home, "r-rotated", "a-rotated");
        leave(&e.store, &claude, lease);
        assert_eq!(e.stored_refresh_token("work").as_deref(), Some("r-rotated"));
    }

    #[test]
    fn another_account_s_tokens_are_set_aside_and_unconfirmed_ones_wait() {
        let mut server = mockito::Server::new();
        profile(&mut server, "a-other", "u-other");
        let e = env(&server.url());
        e.stored("work", "u-work", HOUR);
        let claude = Claude::at(e.tmp.path(), Api::local(&server.url()));

        let (lease, _, home) = start(&e.store, &claude, "work").unwrap();
        rotate(&home, "r-other", "a-other");
        leave(&e.store, &claude, lease);
        assert_eq!(e.stored_refresh_token("work").as_deref(), Some("r-work"));
        assert_eq!(set_aside_count(&e), 1);
        assert!(!home.exists());

        let (lease, _, home) = start(&e.store, &claude, "work").unwrap();
        rotate(&home, "r-unknown", "a-unknown");
        leave(&e.store, &claude, lease);
        assert!(
            home.exists(),
            "tokens nobody could identify yet stay where they are"
        );
        assert!(
            in_use(&e.store, "claude", "work"),
            "and keep the login from being used"
        );
        assert_eq!(set_aside_count(&e), 1);
        let err = start(&e.store, &claude, "work").err().unwrap();
        assert!(err.to_string().contains("could not be finished"), "{err}");

        profile(&mut server, "a-unknown", "u-work");
        recover(&e.store, &[&claude]);
        assert!(!home.exists());
        assert_eq!(e.stored_refresh_token("work").as_deref(), Some("r-unknown"));
    }

    #[test]
    fn tokens_older_than_the_store_s_are_set_aside_not_dropped() {
        let mut server = mockito::Server::new();
        profile(&mut server, "a-session", "u-work");
        let e = env(&server.url());
        e.stored("work", "u-work", 3 * HOUR);
        let claude = Claude::at(e.tmp.path(), Api::local(&server.url()));
        let (lease, _, home) = start(&e.store, &claude, "work").unwrap();
        rotate(&home, "r-session", "a-session");
        leave(&e.store, &claude, lease);
        assert_eq!(e.stored_refresh_token("work").as_deref(), Some("r-work"));
        assert_eq!(set_aside_count(&e), 1);
    }

    #[test]
    fn what_the_session_changed_in_the_copied_files_is_merged_back_but_the_login() {
        let e = env(&OFFLINE);
        e.stored("work", "u-work", HOUR);
        e.sign_in(oauth("a-live", "r-live", HOUR), account("u-live"));
        let config = e.tmp.path().join(".claude.json");
        let mut real = read(&config);
        real["projects"] = json!({"/a": {"trusted": false}, "/b": {"n": 1}});
        real["tips"] = json!(1);
        write_json(&config, &real).unwrap();
        let creds = e.tmp.path().join(".credentials.json");
        let mut real_creds = read(&creds);
        real_creds["mcpOAuth"] = json!({"srv": {"refreshToken": "m1", "expiresAt": 1}});
        write_json(&creds, &real_creds).unwrap();
        let (lease, _, home) = start(&e.store, &e.claude, "work").unwrap();

        let mut ours = read(&home.join(".claude.json"));
        ours["projects"]["/a"] = json!({"trusted": true});
        ours["projects"]["/c"] = json!({"new": true});
        ours["tips"] = json!(2);
        ours["mcpServers"] = json!({"x": {}});
        ours["primaryApiKey"] = json!("sk-session");
        write_json(&home.join(".claude.json"), &ours).unwrap();
        let mut ours_creds = read(&home.join(".credentials.json"));
        ours_creds["mcpOAuth"]["srv"] = json!({"refreshToken": "m3", "expiresAt": 3});
        write_json(&home.join(".credentials.json"), &ours_creds).unwrap();
        let mut theirs = read(&config);
        theirs["tips"] = json!(5);
        theirs["projects"]["/b"] = json!({"n": 2});
        write_json(&config, &theirs).unwrap();
        let mut theirs_creds = read(&creds);
        theirs_creds["mcpOAuth"]["srv"] = json!({"refreshToken": "m2", "expiresAt": 2});
        write_json(&creds, &theirs_creds).unwrap();

        leave(&e.store, &e.claude, lease);
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
        assert!(merged.get("primaryApiKey").is_none());
        let merged_creds = read(&creds);
        assert_eq!(merged_creds[OAUTH_KEY]["refreshToken"], "r-live");
        assert_eq!(
            merged_creds["mcpOAuth"]["srv"]["refreshToken"], "m3",
            "the token that expires later is the one a refresh rotated to"
        );
    }

    #[test]
    fn a_codex_session_has_a_codex_home_of_its_own() {
        use crate::codex::{Api as CodexApi, Codex};
        let e = env(&OFFLINE);
        let codex_dir = e.tmp.path().join("codex");
        fs::create_dir(&codex_dir).unwrap();
        fs::write(codex_dir.join("config.toml"), "model = \"x\"").unwrap();
        fs::write(codex_dir.join("auth.json"), "{\"live\": true}").unwrap();
        let codex = Codex::at(&codex_dir, CodexApi::local(&OFFLINE), 0);
        let creds = json!({"tokens": {"access_token": "a", "refresh_token": "r-codex", "account_id": "acct-1"}});
        e.store
            .save(&Entry::new(
                "codex",
                "work",
                creds.clone(),
                identity("acct-1"),
                0,
            ))
            .unwrap();

        let (lease, spec, home) = start(&e.store, &codex, "work").unwrap();
        assert_eq!(spec.var, "CODEX_HOME");
        assert!(home.join("config.toml").is_symlink());
        assert_eq!(read(&home.join("auth.json")), creds);
        assert_eq!(
            fs::read_to_string(codex_dir.join("auth.json")).unwrap(),
            "{\"live\": true}"
        );
        leave(&e.store, &codex, lease);
        assert!(!home.exists());
    }
}
