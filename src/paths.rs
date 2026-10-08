use std::env;
use std::path::{Path, PathBuf};

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

/// `GROK_AUTH_PATH` names the file itself; `GROK_HOME` its directory.
pub fn grok_auth() -> PathBuf {
    if let Some(file) = var_dir("GROK_AUTH_PATH") {
        return file;
    }
    var_dir("GROK_HOME")
        .unwrap_or_else(|| home().join(".grok"))
        .join("auth.json")
}

/// The Kimi Code CLI's home: `KIMI_CODE_HOME`, else `~/.kimi-code`.
pub fn kimi_home() -> PathBuf {
    var_dir("KIMI_CODE_HOME").unwrap_or_else(|| home().join(".kimi-code"))
}

/// cursor-agent's login: `CURSOR_CONFIG_DIR`, else `~/.config/cursor`.
pub fn cursor_auth() -> PathBuf {
    var_dir("CURSOR_CONFIG_DIR")
        .unwrap_or_else(|| config_home().join("cursor"))
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

/// Where `install.sh` puts the units.
pub fn installed_units() -> PathBuf {
    config_home().join("systemd/user")
}

pub fn systemd_user_units() -> Vec<PathBuf> {
    let data_dirs = env::var("XDG_DATA_DIRS").ok().filter(|v| !v.is_empty());
    unit_dirs(
        &config_home(),
        var_dir("XDG_RUNTIME_DIR").as_deref(),
        &var_dir("XDG_DATA_HOME").unwrap_or_else(|| home().join(".local/share")),
        data_dirs
            .as_deref()
            .unwrap_or("/usr/local/share:/usr/share"),
    )
}

fn unit_dirs(config: &Path, runtime: Option<&Path>, data: &Path, data_dirs: &str) -> Vec<PathBuf> {
    let mut dirs = vec![config.join("systemd/user"), "/etc/systemd/user".into()];
    dirs.extend(runtime.map(|r| r.join("systemd/user")));
    dirs.push("/run/systemd/user".into());
    dirs.push(data.join("systemd/user"));
    dirs.extend(
        data_dirs
            .split(':')
            .filter(|d| d.starts_with('/'))
            .map(|d| Path::new(d).join("systemd/user")),
    );
    dirs.extend(["/usr/local/lib/systemd/user", "/usr/lib/systemd/user"].map(PathBuf::from));
    dirs
}

fn cache_file(config: &str) -> Option<PathBuf> {
    config
        .lines()
        .map(str::trim)
        .take_while(|l| !l.starts_with('['))
        .find_map(|l| {
            let value = l
                .strip_prefix("cache_file")?
                .trim_start()
                .strip_prefix('=')?
                .trim();
            let quote = value.chars().next().filter(|c| *c == '"' || *c == '\'')?;
            let value = value[1..].split(quote).next()?;
            (!value.is_empty()).then(|| PathBuf::from(value))
        })
        .filter(|p| *p != env::temp_dir().join("tokengauge-usage.json"))
}

pub fn tokengauge_snapshot() -> PathBuf {
    let config = var_dir("TOKENGAUGE_CONFIG")
        .unwrap_or_else(|| config_home().join("tokengauge/config.toml"));
    std::fs::read_to_string(config)
        .ok()
        .and_then(|text| cache_file(&text))
        .unwrap_or_else(|| {
            var_dir("XDG_STATE_HOME")
                .unwrap_or_else(|| home().join(".local/state"))
                .join("tokengauge/tokengauge-usage.json")
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_units_are_searched_where_systemd_loads_them() {
        let dirs = unit_dirs(
            Path::new("/h/.config"),
            Some(Path::new("/run/user/1000")),
            Path::new("/h/.local/share"),
            "/opt/share:relative:/usr/share",
        );
        let dirs: Vec<_> = dirs.iter().map(|d| d.to_str().unwrap()).collect();
        assert_eq!(
            dirs,
            [
                "/h/.config/systemd/user",
                "/etc/systemd/user",
                "/run/user/1000/systemd/user",
                "/run/systemd/user",
                "/h/.local/share/systemd/user",
                "/opt/share/systemd/user",
                "/usr/share/systemd/user",
                "/usr/local/lib/systemd/user",
                "/usr/lib/systemd/user",
            ]
        );
    }

    #[test]
    fn tokengauge_cache_file_is_read_like_tokengauge_reads_it() {
        let at = |text: &str| cache_file(text).map(|p| p.display().to_string());
        assert_eq!(
            at("cache_file = \"/data/tg.json\"").as_deref(),
            Some("/data/tg.json")
        );
        assert_eq!(
            at("  cache_file='/data/tg.json'  ").as_deref(),
            Some("/data/tg.json")
        );
        assert_eq!(at("# cache_file = \"/x\""), None);
        assert_eq!(at("cache_file = \"\""), None);
        assert_eq!(at("[waybar]\ncache_file = \"/x\""), None);
        assert_eq!(
            at("refresh_secs = 600\ncache_file = \"/x\"\n[a]").as_deref(),
            Some("/x")
        );
        let legacy = env::temp_dir().join("tokengauge-usage.json");
        assert_eq!(at(&format!("cache_file = \"{}\"", legacy.display())), None);
    }
}
