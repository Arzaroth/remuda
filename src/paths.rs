use std::env;
use std::path::PathBuf;

fn home() -> PathBuf {
    env::var_os("HOME").map(PathBuf::from).unwrap_or_default()
}

/// A directory named relative to where remuda was started means something
/// else from anywhere else, the refresh timer included.
fn var_dir(name: &str) -> Option<PathBuf> {
    let dir = PathBuf::from(env::var_os(name).filter(|v| !v.is_empty())?);
    Some(std::path::absolute(&dir).unwrap_or(dir))
}

fn claude_config_dir() -> Option<PathBuf> {
    var_dir("CLAUDE_CONFIG_DIR")
}

pub fn claude_credentials() -> PathBuf {
    claude_config_dir()
        .unwrap_or_else(|| home().join(".claude"))
        .join(".credentials.json")
}

pub fn claude_config() -> PathBuf {
    match claude_config_dir() {
        Some(dir) => dir.join(".claude.json"),
        None => home().join(".claude.json"),
    }
}

pub fn codex_auth() -> PathBuf {
    var_dir("CODEX_HOME")
        .unwrap_or_else(|| home().join(".codex"))
        .join("auth.json")
}

/// Private per-user scratch space: `$XDG_RUNTIME_DIR/remuda`, else the store.
pub fn runtime_dir() -> PathBuf {
    env::var_os("XDG_RUNTIME_DIR")
        .filter(|v| !v.is_empty())
        .map(|d| PathBuf::from(d).join("remuda"))
        .unwrap_or_else(store_root)
}

pub fn store_root() -> PathBuf {
    if let Some(dir) = var_dir("REMUDA_STORE") {
        return dir;
    }
    env::var_os("XDG_DATA_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".local/share"))
        .join("remuda/credentials")
}

fn config_home() -> PathBuf {
    var_dir("XDG_CONFIG_HOME").unwrap_or_else(|| home().join(".config"))
}

pub fn systemd_user_units() -> PathBuf {
    config_home().join("systemd/user")
}

/// TokenGauge's snapshot: the top-level `cache_file` of its config when set,
/// else `$XDG_STATE_HOME/tokengauge/tokengauge-usage.json`.
pub fn tokengauge_snapshot() -> PathBuf {
    let config = std::fs::read_to_string(config_home().join("tokengauge/config.toml"));
    let configured = config.ok().and_then(|text| {
        text.lines()
            .map(str::trim)
            .take_while(|l| !l.starts_with('['))
            .find_map(|l| {
                let value = l
                    .strip_prefix("cache_file")?
                    .trim_start()
                    .strip_prefix('=')?;
                let value = value.trim().strip_prefix('"')?.split('"').next()?;
                (!value.is_empty()).then(|| PathBuf::from(value))
            })
    });
    configured.unwrap_or_else(|| {
        var_dir("XDG_STATE_HOME")
            .unwrap_or_else(|| home().join(".local/state"))
            .join("tokengauge/tokengauge-usage.json")
    })
}
