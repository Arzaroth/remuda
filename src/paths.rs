use std::env;
use std::path::PathBuf;

fn home() -> PathBuf {
    env::var_os("HOME").map(PathBuf::from).unwrap_or_default()
}

fn claude_config_dir() -> Option<PathBuf> {
    env::var_os("CLAUDE_CONFIG_DIR")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
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
    env::var_os("CODEX_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".codex"))
        .join("auth.json")
}

pub fn store_root() -> PathBuf {
    if let Some(dir) = env::var_os("REMUDA_STORE").filter(|v| !v.is_empty()) {
        return PathBuf::from(dir);
    }
    env::var_os("XDG_DATA_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".local/share"))
        .join("remuda/credentials")
}
