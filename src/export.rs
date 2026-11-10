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
        for carried in &bundle.carried {
            let at = format!("{}/{}", carried.provider, carried.name);
            if known(&carried.provider).is_none() {
                writeln!(
                    out,
                    "skipped {at}: this remuda does not know {}",
                    carried.provider
                )?;
                continue;
            }
            if !force && store.get(&carried.provider, &carried.name)?.is_some() {
                writeln!(out, "kept {at}, already stored here (--force replaces it)")?;
                continue;
            }
            let account = carried.meta.get("accountId").and_then(Value::as_str);
            if let Some(other) = store
                .list(&carried.provider)?
                .into_iter()
                .find(|e| Some(e.meta.account_id.as_str()) == account && e.name != carried.name)
            {
                writeln!(
                    out,
                    "skipped {at}: its account is stored here as {}",
                    other.qualified()
                )?;
                continue;
            }
            store.put_files(
                &carried.provider,
                &carried.name,
                &carried.creds,
                &carried.meta,
            )?;
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
            let settings: crate::proxy::settings::Settings = serde_json::from_value(proxy.clone())
                .context("the proxy settings are malformed")?;
            crate::fsx::write_json(places.proxy, &serde_json::to_value(settings)?)?;
            writeln!(out, "set the proxy up as it was, with no client keys yet")?;
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
        left.push(login);
    }
    Ok(Restored { sign_in: left })
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
