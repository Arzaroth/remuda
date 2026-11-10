use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::config;
use crate::ops::{self, LiveState};
use crate::provider::Provider;
use crate::store::{Entry, Store};

pub const FORMAT: u32 = 1;

/// Another machine's logins and settings, as `remuda export` sends them.
/// Holds keys and tokens, so it has no `Debug` that could reach a log.
#[derive(Serialize, Deserialize, Default)]
pub struct Bundle {
    pub remuda_export: u32,
    /// Logins whose files go across as they are: API keys, Cursor's, and the
    /// ones `--move` takes off this machine.
    #[serde(default)]
    pub carried: Vec<Carried>,
    /// Logins whose refresh is single use, signed in again over there.
    #[serde(default)]
    pub sign_in: Vec<ToSignIn>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config: Option<String>,
    /// The proxy's settings, without the keys its clients send.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy: Option<Value>,
    /// What `tokengauge --sync-export` printed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fleet: Option<Value>,
}

#[derive(Serialize, Deserialize)]
pub struct Carried {
    pub provider: String,
    pub name: String,
    pub creds: String,
    pub meta: Value,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct ToSignIn {
    pub provider: String,
    pub name: String,
    pub account_id: String,
    pub email: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

impl ToSignIn {
    pub fn qualified(&self) -> String {
        format!("{}/{}", self.provider, self.name)
    }
}

/// `moved` are logins whose tokens go across and leave this store once the
/// bundle is sent; the one a CLI is using is refused.
pub fn build(
    store: &Store,
    providers: &[&dyn Provider],
    config_path: &Path,
    proxy_path: &Path,
    moved: &[(&dyn Provider, String)],
    fleet: Option<Value>,
) -> Result<Bundle> {
    let mut bundle = Bundle {
        remuda_export: FORMAT,
        fleet,
        ..Bundle::default()
    };
    for p in providers {
        let live = ops::sync_live(store, *p)?;
        let entries = store.list(p.id())?;
        for (_, name) in moved.iter().filter(|(m, _)| m.id() == p.id()) {
            if !entries.iter().any(|e| &e.name == name) {
                bail!("there is no {}/{name} to move", p.id());
            }
            if matches!(&live, LiveState::Stored { name: active, .. } if active == name) {
                bail!(
                    "{}/{name} is the login {} uses; switch it to another or sign it out before moving it",
                    p.id(),
                    p.name()
                );
            }
            if crate::session::in_use(store, p.id(), name) {
                bail!(
                    "{}/{name} is in a `remuda run` session; end it first",
                    p.id()
                );
            }
        }
        for entry in entries {
            let moving = moved
                .iter()
                .any(|(m, name)| m.id() == p.id() && *name == entry.name);
            if moving || p.portable() {
                let (creds, meta) = store
                    .files(p.id(), &entry.name)?
                    .with_context(|| format!("{} vanished", entry.qualified()))?;
                bundle.carried.push(Carried {
                    provider: p.id().to_owned(),
                    name: entry.name,
                    creds,
                    meta,
                });
            } else {
                bundle.sign_in.push(to_sign_in(&entry));
            }
        }
    }
    bundle.config = config::read(config_path)?;
    let proxy = crate::proxy::settings::File::at(proxy_path.to_owned()).read()?;
    if proxy != crate::proxy::settings::Settings::default() {
        let mut proxy = serde_json::to_value(proxy)?;
        proxy["keys"] = Value::Array(Vec::new());
        bundle.proxy = Some(proxy);
    }
    Ok(bundle)
}

/// Takes the moved logins off this store once the bundle has gone, setting
/// their tokens aside first in case the other side cannot restore them. One
/// whose tokens changed after the bundle read them stays, since the copy sent
/// may already be spent.
pub fn drop_moved(
    store: &Store,
    config: &Path,
    bundle: &Bundle,
    moved: &[(&dyn Provider, String)],
    out: &mut dyn Write,
) -> Result<()> {
    let _lock = store.lock()?;
    for (p, name) in moved {
        let at = format!("{}/{name}", p.id());
        let sent = bundle
            .carried
            .iter()
            .find(|c| c.provider == p.id() && c.name == *name)
            .map(|c| c.creds.as_str());
        let dropped = ops::sync_live(store, *p).and_then(|state| {
            let now = store.files(p.id(), name)?;
            if now.as_ref().map(|(creds, _)| creds.as_str()) != sent {
                return Ok(None);
            }
            let entry = store
                .get(p.id(), name)?
                .with_context(|| format!("{at} vanished"))?;
            crate::commands::refuse_in_use(store, *p, name)?;
            let kept = store.set_aside(p.id(), &entry.meta.account_id, &entry.creds)?;
            crate::commands::remove(store, config, *p, &state, name, out)?;
            Ok(Some(kept))
        });
        match dropped {
            Ok(Some(kept)) => writeln!(
                out,
                "  its tokens are set aside in {} until {at} works over there",
                kept.display()
            )?,
            Ok(None) => writeln!(
                out,
                "warning: kept {at}: its tokens changed here after the bundle read them, so the copy sent may be spent; export it again, or sign it in again over there"
            )?,
            Err(e) => writeln!(
                out,
                "warning: {at} went across but is still here ({e:#}); remove it before either machine refreshes it"
            )?,
        }
    }
    Ok(())
}

fn to_sign_in(entry: &Entry) -> ToSignIn {
    ToSignIn {
        provider: entry.provider.clone(),
        name: entry.name.clone(),
        account_id: entry.meta.account_id.clone(),
        email: entry.meta.email.clone(),
        label: entry.meta.label.clone(),
    }
}

/// What `tokengauge --sync-export` prints, or why there is none.
pub fn fleet(tokengauge: Option<&Path>, s3_keys: bool) -> std::result::Result<Value, String> {
    let program = tokengauge.ok_or("TokenGauge is not installed")?;
    let output = Command::new(program)
        .arg("--sync-export")
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("could not run {}: {e}", program.display()))?;
    if !output.status.success() {
        let said = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "TokenGauge printed no fleet ({})",
            said.lines().last().unwrap_or("no reason given").trim()
        ));
    }
    let mut fleet: Value = serde_json::from_slice(&output.stdout)
        .map_err(|_| "TokenGauge's --sync-export printed no fleet bundle".to_owned())?;
    if !s3_keys && let Some(fleet) = fleet.as_object_mut() {
        fleet.remove("s3_credentials");
    }
    Ok(fleet)
}

/// What a bundle carries, in a line, without a secret in it.
pub fn describe(bundle: &Bundle) -> String {
    let mut parts = vec![
        format!("{} login(s) as they are", bundle.carried.len()),
        format!("{} to sign in again", bundle.sign_in.len()),
    ];
    if bundle.config.is_some() {
        parts.push("config.toml".into());
    }
    if bundle.proxy.is_some() {
        parts.push("the proxy's settings".into());
    }
    if let Some(fleet) = &bundle.fleet {
        parts.push(if fleet.get("s3_credentials").is_some() {
            "the TokenGauge fleet with its S3 keys".into()
        } else {
            "the TokenGauge fleet".into()
        });
    }
    format!("It carries {}.", parts.join(", "))
}

/// Refuses a file already there rather than replacing what might be another
/// export.
pub fn write_new_private(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .with_context(|| format!("could not create {}", path.display()))?;
    file.write_all(bytes)?;
    Ok(())
}

/// Whether `bytes` are a bundle as it travels over a wormhole, rather than
/// one sealed into a file.
pub fn unsealed(bytes: &[u8]) -> bool {
    bytes.trim_ascii_start().starts_with(b"{")
}

/// Keeps a received bundle that could not be restored, since for the logins
/// moved here it may be the only copy left.
pub fn keep_received(store: &Store, bytes: &[u8]) -> Result<PathBuf> {
    crate::fsx::create_private_dir(store.root())
        .with_context(|| format!("could not create {}", store.root().display()))?;
    let path = store
        .root()
        .join(format!(".received-{}.json", crate::fsx::now_ms()));
    write_new_private(&path, bytes)?;
    Ok(path)
}

/// Whether `file` is one [`keep_received`] wrote, which a restore that
/// succeeds removes.
pub fn was_kept(store: &Store, file: &Path) -> bool {
    file.parent()
        .and_then(|dir| dir.canonicalize().ok())
        .is_some_and(|dir| store.root().canonicalize().is_ok_and(|root| root == dir))
        && file
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with(".received-") && n.ends_with(".json"))
}

pub fn parse(bytes: &[u8]) -> Result<Bundle> {
    let bundle: Bundle =
        serde_json::from_slice(bytes).context("this is not something `remuda export` sent")?;
    if bundle.remuda_export != FORMAT {
        bail!(
            "this was exported in format {}, and this remuda reads format {FORMAT}; update the older side",
            bundle.remuda_export
        );
    }
    Ok(bundle)
}

/// Where restore leaves things, for the sign-ins that follow.
#[derive(Default)]
pub struct Restored {
    pub sign_in: Vec<ToSignIn>,
}

pub struct Places<'a> {
    pub config: &'a Path,
    pub proxy: &'a Path,
    pub tokengauge: Option<&'a Path>,
}

/// Writes what the bundle carries that this machine lacks, and returns the
/// logins left to sign in. What is already here is kept unless `force`.
pub fn restore(
    store: &Store,
    providers: &[&dyn Provider],
    places: &Places,
    bundle: Bundle,
    force: bool,
    out: &mut dyn Write,
) -> Result<Restored> {
    let known = |id: &str| providers.iter().find(|p| p.id() == id).copied();
    {
        let _lock = store.lock()?;
        let mut lives: BTreeMap<&str, std::result::Result<LiveState, String>> = BTreeMap::new();
        for carried in &bundle.carried {
            let at = format!("{}/{}", carried.provider, carried.name);
            let Some(p) = known(&carried.provider) else {
                writeln!(
                    out,
                    "skipped {at}: this remuda does not know {}",
                    carried.provider
                )?;
                continue;
            };
            if force {
                lives
                    .entry(p.id())
                    .or_insert_with(|| ops::sync_live(store, p).map_err(|e| format!("{e:#}")));
            }
            let here = store.files(p.id(), &carried.name)?;
            if !force && let Some((creds, _)) = &here {
                writeln!(out, "kept {at}, already stored here (--force replaces it)")?;
                if *creds != carried.creds {
                    set_aside_carried(store, p, carried, out)?;
                }
                continue;
            }
            if let Some((creds, _)) = &here
                && let Some(live) = lives.get(p.id())
                && let Some(why) = refuse_replacing(store, p, &carried.name, live)
            {
                writeln!(out, "kept {at}: {why}")?;
                if *creds != carried.creds {
                    set_aside_carried(store, p, carried, out)?;
                }
                continue;
            }
            let account = carried.meta.get("accountId").and_then(Value::as_str);
            if let Some(other) = store
                .list(p.id())?
                .into_iter()
                .find(|e| Some(e.meta.account_id.as_str()) == account && e.name != carried.name)
            {
                writeln!(
                    out,
                    "skipped {at}: its account is stored here as {}",
                    other.qualified()
                )?;
                if store
                    .files(p.id(), &other.name)?
                    .is_none_or(|(creds, _)| creds != carried.creds)
                {
                    set_aside_carried(store, p, carried, out)?;
                }
                continue;
            }
            if p.renews()
                && let Some(old) = store.get(p.id(), &carried.name)?
                && here.is_some_and(|(creds, _)| creds != carried.creds)
            {
                let kept = store.set_aside(p.id(), &old.meta.account_id, &old.creds)?;
                writeln!(
                    out,
                    "set the tokens {at} held here aside in {}",
                    kept.display()
                )?;
            }
            store.put_files(p.id(), &carried.name, &carried.creds, &carried.meta)?;
            writeln!(out, "stored {at}")?;
        }
    }

    let mut names: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for p in providers {
        for entry in store.list(p.id())? {
            names.entry(p.id().to_owned()).or_default().push(entry.name);
        }
    }
    for login in &bundle.sign_in {
        names
            .entry(login.provider.clone())
            .or_default()
            .push(login.name.clone());
    }
    if let Some(text) = &bundle.config {
        restore_config(places.config, text, &names, providers, force, out)?;
    }
    if let Some(proxy) = &bundle.proxy {
        if force || !places.proxy.exists() {
            let mut settings: crate::proxy::settings::Settings =
                serde_json::from_value(proxy.clone())
                    .context("the proxy settings are malformed")?;
            settings.keys = crate::proxy::settings::File::at(places.proxy.to_owned())
                .read()
                .map(|here| here.keys)
                .unwrap_or_default();
            crate::fsx::write_json(places.proxy, &serde_json::to_value(&settings)?)?;
            if settings.keys.is_empty() {
                writeln!(out, "set the proxy up as it was, with no client keys yet")?;
            } else {
                writeln!(
                    out,
                    "set the proxy up as it was, keeping its client keys here"
                )?;
            }
        } else {
            writeln!(
                out,
                "kept the proxy's settings here (--force replaces them)"
            )?;
        }
    }
    if let Some(fleet) = &bundle.fleet {
        join_fleet(places.tokengauge, fleet, force, out)?;
    }

    let mut left = Vec::new();
    for login in bundle.sign_in {
        let Some(p) = known(&login.provider) else {
            writeln!(
                out,
                "skipped {}: this remuda does not know {}",
                login.qualified(),
                login.provider
            )?;
            continue;
        };
        if store.get(p.id(), &login.name)?.is_some() {
            continue;
        }
        if let Some(other) = store
            .list(p.id())?
            .into_iter()
            .find(|e| e.meta.account_id == login.account_id)
        {
            writeln!(
                out,
                "skipped {}: its account is stored here as {}",
                login.qualified(),
                other.qualified()
            )?;
            continue;
        }
        left.push(login);
    }
    Ok(Restored { sign_in: left })
}

/// Why `--force` may not replace the stored `name`: the checks a switch and a
/// remove make, since the CLI or a `remuda run` session may be spending it.
fn refuse_replacing(
    store: &Store,
    p: &dyn Provider,
    name: &str,
    live: &std::result::Result<LiveState, String>,
) -> Option<String> {
    match live {
        Err(e) => Some(format!("{} could not be read back first: {e}", p.name())),
        Ok(LiveState::Unreadable { error }) => {
            Some(format!("which login {} uses is unknown: {error}", p.name()))
        }
        Ok(live) if live.active_name() == Some(name) => Some(format!(
            "it is the login {} uses here; switch it to another first",
            p.name()
        )),
        Ok(_) if crate::session::in_use(store, p.id(), name) => {
            Some("it is in a `remuda run` session here; end it first".to_owned())
        }
        Ok(_) => None,
    }
}

/// Keeps the tokens of a carried login that is not stored, when its provider
/// renews them: they may be the only copy, the sender's having been moved.
fn set_aside_carried(
    store: &Store,
    p: &dyn Provider,
    carried: &Carried,
    out: &mut dyn Write,
) -> Result<()> {
    if !p.renews() {
        return Ok(());
    }
    let creds = serde_json::from_str(&carried.creds)
        .unwrap_or_else(|_| Value::String(carried.creds.clone()));
    let account = carried
        .meta
        .get("accountId")
        .and_then(Value::as_str)
        .unwrap_or(&carried.name);
    let kept = store.set_aside(p.id(), account, &creds)?;
    writeln!(
        out,
        "  the tokens it carried are set aside in {}",
        kept.display()
    )?;
    Ok(())
}

fn restore_config(
    path: &Path,
    text: &str,
    names: &BTreeMap<String, Vec<String>>,
    providers: &[&dyn Provider],
    force: bool,
    out: &mut dyn Write,
) -> Result<()> {
    if !force && config::read(path)?.is_some() {
        writeln!(out, "kept config.toml here (--force replaces it)")?;
        return Ok(());
    }
    let parsed = config::parse(path, text)?;
    let problems = parsed.check(&|id| {
        providers
            .iter()
            .any(|p| p.id() == id)
            .then(|| names.get(id).cloned().unwrap_or_default())
    });
    if !problems.is_empty() {
        writeln!(
            out,
            "kept config.toml as it is here, since the one sent has problems:"
        )?;
        for problem in problems {
            writeln!(out, "  {problem}")?;
        }
        return Ok(());
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    crate::fsx::write_private(path, text)?;
    writeln!(out, "wrote {}", path.display())?;
    Ok(())
}

fn join_fleet(
    tokengauge: Option<&Path>,
    fleet: &Value,
    force: bool,
    out: &mut dyn Write,
) -> Result<()> {
    let Some(program) = tokengauge else {
        writeln!(
            out,
            "the TokenGauge fleet was sent, but TokenGauge is not installed here; install it and export again"
        )?;
        return Ok(());
    };
    let mut join = Command::new(program);
    join.args(["--sync-join", "-"]);
    if force {
        join.arg("--sync-force");
    }
    let mut child = join
        .stdin(Stdio::piped())
        .spawn()
        .with_context(|| format!("could not run {}", program.display()))?;
    child
        .stdin
        .take()
        .context("no stdin to hand TokenGauge the fleet on")?
        .write_all(serde_json::to_string(fleet)?.as_bytes())?;
    if !child.wait()?.success() {
        writeln!(
            out,
            "TokenGauge did not join the fleet; the rest is restored"
        )?;
        return Ok(());
    }
    let tested = Command::new(program).arg("--sync-test").status()?;
    if !tested.success() {
        writeln!(
            out,
            "TokenGauge joined the fleet, but its round trip failed"
        )?;
    }
    Ok(())
}

/// A wormhole client remuda hands the bundle to rather than carrying its own.
pub enum Via {
    Wyrm(PathBuf),
    Wormhole(PathBuf),
}

impl Via {
    pub fn find(search: &std::ffi::OsStr) -> Option<Via> {
        crate::doctor::which("wyrm", search)
            .map(Via::Wyrm)
            .or_else(|| crate::doctor::which("wormhole", search).map(Via::Wormhole))
    }

    pub fn name(&self) -> &'static str {
        match self {
            Via::Wyrm(_) => "wyrmyon",
            Via::Wormhole(_) => "magic-wormhole",
        }
    }

    fn program(&self) -> &Path {
        match self {
            Via::Wyrm(p) | Via::Wormhole(p) => p,
        }
    }

    /// Prints the code on the terminal and waits for the other side.
    pub fn send(&self, file: &Path) -> Result<()> {
        let status = Command::new(self.program())
            .args(["send", "--hide-progress"])
            .arg(file)
            .status()
            .with_context(|| format!("could not run {}", self.program().display()))?;
        if !status.success() {
            bail!("{} did not send it", self.name());
        }
        Ok(())
    }

    /// Asks for the code on the terminal itself, so it never lands on a
    /// command line.
    pub fn receive(&self, into: &Path) -> Result<()> {
        let status = Command::new(self.program())
            .args(["receive", "--accept-file", "--hide-progress", "-o"])
            .arg(into)
            .status()
            .with_context(|| format!("could not run {}", self.program().display()))?;
        if !status.success() || !into.exists() {
            bail!("{} received nothing", self.name());
        }
        Ok(())
    }
}

/// A directory only this user can read, removed when dropped.
pub struct Private(PathBuf);

impl Private {
    pub fn new() -> Result<Self> {
        let dir = std::env::temp_dir().join(format!("remuda-{}", crate::pkce::random()?));
        crate::fsx::create_private_dir(&dir)
            .with_context(|| format!("could not create {}", dir.display()))?;
        Ok(Private(dir))
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Private {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apikey::ApiKey;
    use crate::ops::testing::*;
    use crate::provider::Identity;
    use serde_json::json;
    use std::os::unix::fs::PermissionsExt;

    fn glm() -> ApiKey {
        ApiKey::glm().with(None, |_| None)
    }

    fn keyed(e: &Env, name: &str, key: &str) {
        let identity = Identity {
            account_id: format!("key-{key}"),
            email: format!("key ending {key}"),
            oauth_account: None,
        };
        e.store
            .save(&Entry::new("glm", name, json!({"key": key}), identity, 1))
            .unwrap();
    }

    fn places(e: &Env) -> (PathBuf, PathBuf) {
        (
            e.tmp.path().join("config.toml"),
            e.store.root().join(".proxy.json"),
        )
    }

    fn bundle_of(e: &Env, moved: &[(&dyn Provider, String)]) -> Result<Bundle> {
        let g = glm();
        let (config, proxy) = places(e);
        build(&e.store, &[&e.claude, &g], &config, &proxy, moved, None)
    }

    fn restore_into(e: &Env, bundle: Bundle, force: bool) -> (Restored, String) {
        let g = glm();
        let (config, proxy) = places(e);
        let places = Places {
            config: &config,
            proxy: &proxy,
            tokengauge: None,
        };
        let mut out = Vec::new();
        let restored =
            restore(&e.store, &[&e.claude, &g], &places, bundle, force, &mut out).unwrap();
        (restored, String::from_utf8(out).unwrap())
    }

    fn round_trip(bundle: &Bundle) -> Bundle {
        parse(&serde_json::to_vec(bundle).unwrap()).unwrap()
    }

    fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[test]
    fn keys_go_as_their_files_and_single_use_logins_as_a_list() {
        let e = env(&OFFLINE);
        e.stored("work", "u-1", HOUR);
        keyed(&e, "k", "sk-1");

        let bundle = bundle_of(&e, &[]).unwrap();

        assert_eq!(bundle.carried.len(), 1);
        let carried = &bundle.carried[0];
        assert_eq!(
            (carried.provider.as_str(), carried.name.as_str()),
            ("glm", "k")
        );
        let (creds, meta) = e.store.files("glm", "k").unwrap().unwrap();
        assert_eq!(carried.creds, creds);
        assert_eq!(carried.meta, meta);
        assert_eq!(bundle.sign_in.len(), 1);
        assert_eq!(bundle.sign_in[0].qualified(), "claude/work");
        assert_eq!(bundle.sign_in[0].account_id, "u-1");
        let json = serde_json::to_string(&bundle.sign_in).unwrap();
        assert!(!json.contains("r-work"), "{json}");
        assert!(bundle.config.is_none() && bundle.proxy.is_none());
        assert_eq!(
            describe(&bundle),
            "It carries 1 login(s) as they are, 1 to sign in again."
        );
    }

    #[test]
    fn a_moved_login_carries_its_tokens_and_the_one_in_use_is_refused() {
        let e = env(&OFFLINE);
        e.stored("work", "u-1", HOUR);
        e.stored("perso", "u-2", HOUR);
        e.sign_in(oauth("a-work", "r-work", HOUR), account("u-1"));

        let bundle = bundle_of(&e, &[(&e.claude, "perso".into())]).unwrap();
        assert_eq!(bundle.carried.len(), 1);
        assert_eq!(bundle.carried[0].name, "perso");
        assert!(bundle.carried[0].creds.contains("r-perso"));
        assert_eq!(bundle.sign_in.len(), 1);

        let error = bundle_of(&e, &[(&e.claude, "work".into())]).err().unwrap();
        assert!(
            error.to_string().contains("is the login Claude Code uses"),
            "{error}"
        );
        let error = bundle_of(&e, &[(&e.claude, "ghost".into())]).err().unwrap();
        assert!(error.to_string().contains("no claude/ghost"), "{error}");
    }

    #[test]
    fn restore_files_carried_logins_byte_for_byte_and_keeps_what_is_here() {
        let from = env(&OFFLINE);
        keyed(&from, "k", "sk-1");
        from.stored("work", "u-1", HOUR);
        let bundle = bundle_of(&from, &[]).unwrap();
        let to = env(&OFFLINE);

        let (restored, out) = restore_into(&to, round_trip(&bundle), false);

        assert!(out.contains("stored glm/k"), "{out}");
        let entry = to.store.get("glm", "k").unwrap().unwrap();
        assert!(entry.verified);
        assert_eq!(
            std::fs::read(to.store.root().join("glm/k.json")).unwrap(),
            std::fs::read(from.store.root().join("glm/k.json")).unwrap()
        );
        assert_eq!(restored.sign_in.len(), 1);
        assert_eq!(restored.sign_in[0].qualified(), "claude/work");

        keyed(&to, "k", "sk-here");
        let (_, out) = restore_into(&to, round_trip(&bundle), false);
        assert!(out.contains("kept glm/k"), "{out}");
        assert_eq!(
            to.store.get("glm", "k").unwrap().unwrap().creds["key"],
            "sk-here"
        );
        restore_into(&to, round_trip(&bundle), true);
        assert_eq!(
            to.store.get("glm", "k").unwrap().unwrap().creds["key"],
            "sk-1"
        );

        let other = env(&OFFLINE);
        keyed(&other, "mine", "sk-1");
        let (_, out) = restore_into(&other, round_trip(&bundle), false);
        assert!(
            out.contains("skipped glm/k: its account is stored here as glm/mine"),
            "{out}"
        );
        assert!(other.store.get("glm", "k").unwrap().is_none());
    }

    #[test]
    fn a_login_already_signed_in_here_is_not_asked_for_again() {
        let from = env(&OFFLINE);
        from.stored("work", "u-1", HOUR);
        from.stored("perso", "u-2", HOUR);
        let bundle = bundle_of(&from, &[]).unwrap();
        let to = env(&OFFLINE);
        to.stored("work", "u-1", HOUR);

        let (restored, _) = restore_into(&to, bundle, false);

        let left: Vec<String> = restored.sign_in.iter().map(ToSignIn::qualified).collect();
        assert_eq!(left, ["claude/perso"]);
    }

    #[test]
    fn the_config_is_written_when_its_names_resolve_and_kept_otherwise() {
        let from = env(&OFFLINE);
        from.stored("work", "u-1", HOUR);
        let (config, _) = places(&from);
        std::fs::write(&config, "[warm]\nclaude = [\"work\"]\n").unwrap();
        let bundle = bundle_of(&from, &[]).unwrap();
        assert!(bundle.config.is_some());

        let to = env(&OFFLINE);
        let (_, out) = restore_into(&to, round_trip(&bundle), false);
        assert!(out.contains("wrote"), "{out}");
        assert_eq!(
            std::fs::read_to_string(places(&to).0).unwrap(),
            "[warm]\nclaude = [\"work\"]\n"
        );
        let (_, out) = restore_into(&to, round_trip(&bundle), false);
        assert!(out.contains("kept config.toml here"), "{out}");

        let mut ghostly = round_trip(&bundle);
        ghostly.config = Some("[warm]\nclaude = [\"ghost\"]\n".into());
        let fresh = env(&OFFLINE);
        let (_, out) = restore_into(&fresh, ghostly, false);
        assert!(out.contains("no stored login claude/ghost"), "{out}");
        assert!(!places(&fresh).0.exists());
    }

    #[test]
    fn the_proxy_goes_across_without_its_client_keys() {
        let from = env(&OFFLINE);
        let (_, proxy) = places(&from);
        crate::fsx::write_json(
            &proxy,
            &json!({"enabled": true, "port": 9999, "lan": false, "keys": [{"id": "1", "name": "laptop", "key": "rmd-secret", "created": 1}]}),
        )
        .unwrap();

        let bundle = bundle_of(&from, &[]).unwrap();
        assert!(
            !serde_json::to_string(&bundle)
                .unwrap()
                .contains("rmd-secret")
        );

        let to = env(&OFFLINE);
        let (_, out) = restore_into(&to, round_trip(&bundle), false);
        assert!(out.contains("set the proxy up"), "{out}");
        let restored = crate::proxy::settings::File::at(places(&to).1)
            .read()
            .unwrap();
        assert!(restored.enabled);
        assert_eq!(restored.port, 9999);
        assert!(restored.keys.is_empty());
    }

    #[test]
    fn the_fleet_is_what_tokengauge_exports_less_the_s3_keys_on_request() {
        let tmp = tempfile::tempdir().unwrap();
        let tokengauge = script(
            tmp.path(),
            "tokengauge",
            r#"[ "$1" = --sync-export ] && echo '{"tokengauge_sync":1,"key":"k","s3_credentials":{"access_key_id":"a","secret_access_key":"s"}}'"#,
        );

        let fleet_with = fleet(Some(&tokengauge), true).unwrap();
        assert_eq!(fleet_with["s3_credentials"]["access_key_id"], "a");
        let fleet_without = fleet(Some(&tokengauge), false).unwrap();
        assert!(fleet_without.get("s3_credentials").is_none());
        assert_eq!(fleet_without["key"], "k");

        let failing = script(
            tmp.path(),
            "failing",
            "echo 'no fleet key on this machine' >&2; exit 1",
        );
        let why = fleet(Some(&failing), true).unwrap_err();
        assert!(why.contains("no fleet key on this machine"), "{why}");
        assert_eq!(
            fleet(None, true).unwrap_err(),
            "TokenGauge is not installed"
        );
    }

    #[test]
    fn restore_hands_the_fleet_to_tokengauge_on_stdin_then_tests_it() {
        let tmp = tempfile::tempdir().unwrap();
        let log = tmp.path().join("calls");
        let tokengauge = script(
            tmp.path(),
            "tokengauge",
            &format!(
                "echo \"$@\" >> {log}\n[ \"$1\" = --sync-join ] && cat > {stdin}\nexit 0",
                log = log.display(),
                stdin = tmp.path().join("stdin").display()
            ),
        );
        let to = env(&OFFLINE);
        let (config, proxy) = places(&to);
        let g = glm();
        let bundle = Bundle {
            remuda_export: FORMAT,
            fleet: Some(json!({"tokengauge_sync": 1, "key": "k"})),
            ..Bundle::default()
        };
        let places = Places {
            config: &config,
            proxy: &proxy,
            tokengauge: Some(&tokengauge),
        };
        restore(
            &to.store,
            &[&to.claude, &g],
            &places,
            bundle,
            true,
            &mut Vec::new(),
        )
        .unwrap();

        assert_eq!(
            std::fs::read_to_string(&log).unwrap(),
            "--sync-join - --sync-force\n--sync-test\n"
        );
        let handed: Value =
            serde_json::from_str(&std::fs::read_to_string(tmp.path().join("stdin")).unwrap())
                .unwrap();
        assert_eq!(handed["key"], "k");
    }

    #[test]
    fn a_wormhole_client_sends_the_file_and_receives_into_the_place_given() {
        let tmp = tempfile::tempdir().unwrap();
        let wire = tmp.path().join("wire");
        script(
            tmp.path(),
            "wormhole",
            &format!(
                "case \"$1\" in send) [ \"$2\" = --hide-progress ] && cp \"$3\" {wire};; receive) [ \"$2 $3 $4\" = '--accept-file --hide-progress -o' ] && cp {wire} \"$5\";; esac",
                wire = wire.display()
            ),
        );
        let via = Via::find(tmp.path().as_os_str()).unwrap();
        assert_eq!(via.name(), "magic-wormhole");
        script(tmp.path(), "wyrm", "exit 1");
        let wyrm = Via::find(tmp.path().as_os_str()).unwrap();
        assert_eq!(wyrm.name(), "wyrmyon");
        assert!(wyrm.send(&wire).is_err());

        let dir = Private::new().unwrap();
        let mode = std::fs::metadata(dir.path()).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
        let sent = dir.path().join("out.json");
        std::fs::write(&sent, "{}").unwrap();
        via.send(&sent).unwrap();
        let received = dir.path().join("in.json");
        via.receive(&received).unwrap();
        assert_eq!(std::fs::read_to_string(&received).unwrap(), "{}");
        let gone = dir.path().to_owned();
        drop(dir);
        assert!(!gone.exists());
    }

    #[test]
    fn another_format_or_something_else_is_refused() {
        let error = parse(br#"{"remuda_export": 2}"#).err().unwrap();
        assert!(error.to_string().contains("format 2"), "{error}");
        assert!(parse(b"hello").is_err());
    }

    #[test]
    fn a_sealed_file_is_never_written_over() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("out.age");
        write_new_private(&path, b"one").unwrap();
        assert!(write_new_private(&path, b"two").is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"one");
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}
