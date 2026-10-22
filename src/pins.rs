use std::collections::BTreeMap;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use toml_edit::{DocumentMut, value};

use crate::fsx::{read_record, write_record};
use crate::store::{Store, digest, validate_name};

pub const FILE: &str = ".remuda.toml";

#[derive(Debug, Clone, PartialEq)]
pub struct Pin {
    pub name: String,
    pub file: PathBuf,
}

/// `<store>/.pins.json`: each `.remuda.toml` remuda may read, by path, with
/// the digest of what it held when it was allowed. A file that changed since
/// is not read until allowed again, as direnv does with `.envrc`.
fn allowed_path(store: &Store) -> PathBuf {
    store.root().join(".pins.json")
}

fn allowed(store: &Store) -> BTreeMap<String, String> {
    read_record(&allowed_path(store)).unwrap_or_default()
}

fn allow(store: &Store, path: &Path, text: &str) {
    let mut all = allowed(store);
    all.insert(path.display().to_string(), digest(text));
    write_record(&allowed_path(store), &all, "which pins are allowed");
}

/// Another user's file, or a link to one's own, could steer this user's
/// logins, so only a regular file they own is read.
fn ours(path: &Path) -> bool {
    // SAFETY: getuid cannot fail.
    let me = unsafe { libc::getuid() };
    std::fs::symlink_metadata(path).is_ok_and(|m| m.is_file() && m.uid() == me)
}

fn parse(path: &Path, text: &str) -> Result<DocumentMut> {
    text.parse()
        .with_context(|| format!("{} is not valid TOML", path.display()))
}

/// The login the nearest allowed `.remuda.toml` from `dir` up pins
/// `provider` to. A file that cannot be read for it is passed over with a
/// warning, so a stray one never stops the CLI from starting.
pub fn find(store: &Store, dir: &Path, provider: &str) -> Option<Pin> {
    for dir in dir.ancestors() {
        let path = dir.join(FILE);
        if std::fs::symlink_metadata(&path).is_err() {
            continue;
        }
        match read_pin(store, &path, provider) {
            Ok(Some(name)) => return Some(Pin { name, file: path }),
            Ok(None) => {}
            Err(e) => eprintln!("warning: remuda: {e:#}"),
        }
    }
    None
}

fn read_pin(store: &Store, path: &Path, provider: &str) -> Result<Option<String>> {
    if !ours(path) {
        bail!("ignoring {}: not a file of yours", path.display());
    }
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    let Some(item) = parse(path, &text)?.get(provider).cloned() else {
        return Ok(None);
    };
    let name = item
        .as_str()
        .with_context(|| format!("{} pins {provider} to something not a name", path.display()))?
        .to_owned();
    validate_name(&name).with_context(|| format!("in {}", path.display()))?;
    if allowed(store).get(&path.display().to_string()) != Some(&digest(&text)) {
        bail!(
            "ignoring {}, which pins {provider} to {name}: `remuda pin --allow` there trusts it",
            path.display()
        );
    }
    Ok(Some(name))
}

/// Pins `provider` to `name` in `dir`'s `.remuda.toml`, or unpins it, keeping
/// the rest of the file as written, and allows what it then holds.
pub fn set(store: &Store, dir: &Path, provider: &str, name: Option<&str>) -> Result<PathBuf> {
    let path = dir.join(FILE);
    let mut doc = if path.exists() {
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        parse(&path, &text)?
    } else {
        DocumentMut::new()
    };
    match name {
        Some(name) => {
            validate_name(name)?;
            doc[provider] = value(name);
        }
        None => {
            if doc.remove(provider).is_none() {
                bail!("{} does not pin {provider}", path.display());
            }
        }
    }
    let text = doc.to_string();
    std::fs::write(&path, &text).with_context(|| format!("failed to write {}", path.display()))?;
    allow(store, &path, &text);
    Ok(path)
}

/// Allows the nearest `.remuda.toml` from `dir` up, as it is now.
pub fn allow_nearest(store: &Store, dir: &Path) -> Result<PathBuf> {
    let path = dir
        .ancestors()
        .map(|d| d.join(FILE))
        .find(|p| std::fs::symlink_metadata(p).is_ok())
        .with_context(|| format!("no {FILE} here or above"))?;
    if !ours(&path) {
        bail!("{} is not a file of yours", path.display());
    }
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    parse(&path, &text)?;
    allow(store, &path, &text);
    Ok(path)
}

/// Shell functions that send `claude`, `codex` and the like through
/// `remuda run`, so a pinned directory starts its CLI on its login.
pub fn shell_init(shell: &str, commands: &[&str]) -> Result<String> {
    let line = |cmd: &str| match shell {
        "bash" | "zsh" => Ok(format!(
            "{cmd}() {{ command remuda run {cmd} -- \"$@\"; }}\n"
        )),
        "fish" => Ok(format!(
            "function {cmd} --wraps {cmd}; command remuda run {cmd} -- $argv; end\n"
        )),
        other => bail!("no shell functions for {other}: bash, zsh or fish"),
    };
    commands.iter().map(|c| line(c)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(tmp: &Path) -> Store {
        let store = Store::open(&tmp.join("store"));
        std::fs::create_dir_all(store.root()).unwrap();
        store
    }

    #[test]
    fn the_nearest_allowed_file_that_names_the_cli_pins_it() {
        let tmp = tempfile::tempdir().unwrap();
        let store = store(tmp.path());
        let repo = tmp.path().join("repo");
        let sub = repo.join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        set(&store, tmp.path(), "claude", Some("perso")).unwrap();
        set(&store, tmp.path(), "codex", Some("perso")).unwrap();
        set(&store, &repo, "claude", Some("work")).unwrap();

        let claude = find(&store, &sub, "claude").unwrap();
        assert_eq!(claude.name, "work");
        assert_eq!(claude.file, repo.join(FILE));
        assert_eq!(find(&store, &sub, "codex").unwrap().name, "perso");
        assert_eq!(find(&store, &sub, "grok"), None);

        std::fs::write(repo.join(FILE), "claude = \"job\"\n").unwrap();
        assert_eq!(
            find(&store, &sub, "claude").unwrap().name,
            "perso",
            "a file changed since it was allowed is passed over"
        );
        allow_nearest(&store, &sub).unwrap();
        assert_eq!(find(&store, &sub, "claude").unwrap().name, "job");

        std::fs::write(repo.join(FILE), "claude = [\n").unwrap();
        assert_eq!(find(&store, &sub, "claude").unwrap().name, "perso");
        assert!(allow_nearest(&store, &sub).is_err());
        std::fs::remove_file(repo.join(FILE)).unwrap();
        std::os::unix::fs::symlink(tmp.path().join(FILE), repo.join(FILE)).unwrap();
        assert!(!ours(&repo.join(FILE)), "a link is not read");
    }

    #[test]
    fn setting_a_pin_keeps_the_rest_of_the_file() {
        let tmp = tempfile::tempdir().unwrap();
        let store = store(tmp.path());
        std::fs::write(tmp.path().join(FILE), "# mine\ncodex = \"perso\"\n").unwrap();
        let path = set(&store, tmp.path(), "claude", Some("work")).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text, "# mine\ncodex = \"perso\"\nclaude = \"work\"\n");
        set(&store, tmp.path(), "claude", None).unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "# mine\ncodex = \"perso\"\n"
        );
        assert_eq!(find(&store, tmp.path(), "codex").unwrap().name, "perso");
        assert!(set(&store, tmp.path(), "claude", None).is_err());
        assert!(set(&store, tmp.path(), "claude", Some("a b")).is_err());
    }

    #[test]
    fn shell_functions_go_through_remuda_run() {
        assert_eq!(
            shell_init("zsh", &["claude"]).unwrap(),
            "claude() { command remuda run claude -- \"$@\"; }\n"
        );
        assert!(shell_init("fish", &["codex"]).unwrap().contains("$argv"));
        assert!(shell_init("tcsh", &["claude"]).is_err());
    }
}
