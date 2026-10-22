use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use toml_edit::{DocumentMut, value};

use crate::store::validate_name;

pub const FILE: &str = ".remuda.toml";

#[derive(Debug, Clone, PartialEq)]
pub struct Pin {
    pub name: String,
    pub file: PathBuf,
}

fn parse(path: &Path) -> Result<DocumentMut> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    text.parse()
        .with_context(|| format!("{} is not valid TOML", path.display()))
}

/// A file someone else owns could steer this user's logins from a checkout
/// they cloned, so it is not read, as git does with `safe.directory`.
fn ours(path: &Path) -> bool {
    // SAFETY: getuid cannot fail.
    let me = unsafe { libc::getuid() };
    std::fs::metadata(path).is_ok_and(|m| m.uid() == me)
}

/// The login the nearest `.remuda.toml` from `dir` up pins `provider` to.
pub fn find(dir: &Path, provider: &str) -> Result<Option<Pin>> {
    for dir in dir.ancestors() {
        let path = dir.join(FILE);
        if !path.is_file() {
            continue;
        }
        if !ours(&path) {
            eprintln!(
                "warning: ignoring {}, which another user owns",
                path.display()
            );
            continue;
        }
        let doc = parse(&path)?;
        let Some(item) = doc.get(provider) else {
            continue;
        };
        let name = item.as_str().with_context(|| {
            format!("{} pins {provider} to something not a name", path.display())
        })?;
        validate_name(name).with_context(|| format!("in {}", path.display()))?;
        return Ok(Some(Pin {
            name: name.to_owned(),
            file: path,
        }));
    }
    Ok(None)
}

/// Pins `provider` to `name` in `dir`'s `.remuda.toml`, or unpins it, keeping
/// the rest of the file as written.
pub fn set(dir: &Path, provider: &str, name: Option<&str>) -> Result<PathBuf> {
    let path = dir.join(FILE);
    let mut doc = if path.exists() {
        parse(&path)?
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
    std::fs::write(&path, doc.to_string())
        .with_context(|| format!("failed to write {}", path.display()))?;
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

    #[test]
    fn the_nearest_file_that_names_the_cli_pins_it() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        let sub = repo.join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(
            tmp.path().join(FILE),
            "claude = \"perso\"\ncodex = \"perso\"\n",
        )
        .unwrap();
        std::fs::write(repo.join(FILE), "# the job's\nclaude = \"work\"\n").unwrap();

        let claude = find(&sub, "claude").unwrap().unwrap();
        assert_eq!(claude.name, "work");
        assert_eq!(claude.file, repo.join(FILE));
        assert_eq!(find(&sub, "codex").unwrap().unwrap().name, "perso");
        assert_eq!(find(&sub, "grok").unwrap(), None);

        std::fs::write(repo.join(FILE), "claude = \"../x\"\n").unwrap();
        assert!(find(&sub, "claude").is_err());
        std::fs::write(repo.join(FILE), "claude = [\n").unwrap();
        assert!(find(&sub, "claude").is_err());
    }

    #[test]
    fn setting_a_pin_keeps_the_rest_of_the_file() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join(FILE), "# mine\ncodex = \"perso\"\n").unwrap();
        let path = set(tmp.path(), "claude", Some("work")).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text, "# mine\ncodex = \"perso\"\nclaude = \"work\"\n");
        set(tmp.path(), "claude", None).unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "# mine\ncodex = \"perso\"\n"
        );
        assert!(set(tmp.path(), "claude", None).is_err());
        assert!(set(tmp.path(), "claude", Some("a b")).is_err());
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
