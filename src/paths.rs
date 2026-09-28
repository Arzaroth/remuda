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
