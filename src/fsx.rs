use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde_json::Value;

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    DirBuilder::new().recursive(true).mode(0o700).create(dir)
}

/// UTC, second precision: `2026-09-27T14:05:09Z`.
pub fn rfc3339(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

pub fn read_json(path: &Path) -> Result<Option<Value>> {
    match fs::read_to_string(path) {
        Ok(text) => {
            Ok(Some(serde_json::from_str(&text).with_context(|| {
                format!("{} is not valid JSON", path.display())
            })?))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("failed to read {}", path.display())),
    }
}

pub fn write_json(path: &Path, value: &Value) -> Result<()> {
    let mut body = serde_json::to_string_pretty(value)?;
    body.push('\n');
    write_private(path, &body)
}

pub fn read_record<T: serde::de::DeserializeOwned>(path: &Path) -> Option<T> {
    read_json(path)
        .ok()
        .flatten()
        .and_then(|v| serde_json::from_value(v).ok())
}

/// Best effort: a record that cannot be written never fails the command that
/// keeps it.
pub fn write_record<T: serde::Serialize>(path: &Path, record: &T, what: &str) {
    let written = serde_json::to_value(record)
        .map_err(anyhow::Error::from)
        .and_then(|v| write_json(path, &v));
    if let Err(e) = written {
        eprintln!("warning: could not note {what}: {e:#}");
    }
}

/// Writes through a sibling temp file and a rename, keeping the target's mode
/// when it exists and 0600 otherwise. A symlinked target is written where the
/// link points, so the link survives.
pub fn write_private(path: &Path, body: &str) -> Result<()> {
    write_private_if(path, body, || true).map(|_| ())
}

/// The live login changed between the read a switch acted on and its write.
#[derive(Debug)]
pub struct Changed;

impl std::fmt::Display for Changed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the file changed while remuda was writing it")
    }
}

impl std::error::Error for Changed {}

fn read_text(path: &Path) -> Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("failed to read {}", path.display())),
    }
}

/// Rewrites a JSON object another program also writes, and takes no lock on:
/// read, edit, and rename into place only if the file still holds what was
/// read, starting over otherwise. A read caught halfway through the other
/// program's write is retried too. `edit` returning [`Changed`] stops it.
pub fn update_json(path: &Path, edit: impl Fn(&mut Value) -> Result<()>) -> Result<()> {
    update_json_pacing(path, edit, || {
        std::thread::sleep(std::time::Duration::from_millis(20))
    })
}

fn update_json_pacing(
    path: &Path,
    edit: impl Fn(&mut Value) -> Result<()>,
    mut pause: impl FnMut(),
) -> Result<()> {
    for _ in 0..10 {
        let before = read_text(path)?;
        let mut value = match &before {
            None => serde_json::json!({}),
            Some(text) => match serde_json::from_str(text) {
                Ok(value) => value,
                Err(_) => {
                    pause();
                    continue;
                }
            },
        };
        if !value.is_object() {
            anyhow::bail!("{} is not a JSON object", path.display());
        }
        edit(&mut value)?;
        let mut body = serde_json::to_string_pretty(&value)?;
        body.push('\n');
        if write_private_if(path, &body, || {
            read_text(path).ok().as_ref() == Some(&before)
        })? {
            return Ok(());
        }
        pause();
    }
    anyhow::bail!("{} kept changing while remuda wrote it", path.display())
}

/// Returns false, writing nothing, when `still` says no just before the
/// rename.
fn write_private_if(path: &Path, body: &str, still: impl Fn() -> bool) -> Result<bool> {
    let resolved;
    let path = match fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_symlink() => {
            resolved = fs::canonicalize(path)
                .with_context(|| format!("{} is a dangling link", path.display()))?;
            resolved.as_path()
        }
        _ => path,
    };
    let dir = path.parent().context("path has no parent")?;
    create_private_dir(dir).with_context(|| format!("failed to create {}", dir.display()))?;
    let mode = fs::metadata(path)
        .map(|m| m.permissions().mode() & 0o777)
        .unwrap_or(0o600);
    let name = path
        .file_name()
        .context("path has no file name")?
        .to_string_lossy();
    let tmp = dir.join(format!(".{name}.remuda-{}", std::process::id()));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(mode)
            .open(&tmp)?;
        file.set_permissions(fs::Permissions::from_mode(mode))?;
        file.write_all(body.as_bytes())?;
        file.sync_all()?;
        if !still() {
            return Ok(false);
        }
        fs::rename(&tmp, path).map(|()| true)
    })();
    if !matches!(result, Ok(true)) {
        let _ = fs::remove_file(&tmp);
    }
    result.with_context(|| format!("failed to write {}", path.display()))
}

/// Takes an exclusive `flock` on `path`, creating it (and its directory) if
/// needed: a lock file another tool shares, held until the file is dropped.
pub fn lock_file(path: &Path) -> Result<File> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).with_context(|| format!("failed to create {}", dir.display()))?;
    }
    let file = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)
        .with_context(|| format!("failed to open {}", path.display()))?;
    file.lock()
        .with_context(|| format!("failed to lock {}", path.display()))?;
    Ok(file)
}

pub fn lock(dir: &Path) -> Result<File> {
    create_private_dir(dir).with_context(|| format!("failed to create {}", dir.display()))?;
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(0o600)
        .open(dir.join(".lock"))
        .context("failed to open the store lock")?;
    file.lock().context("failed to lock the store")?;
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_symlinked_file_is_written_through_its_link() {
        let tmp = tempfile::tempdir().unwrap();
        let real = tmp.path().join("real.json");
        let link = tmp.path().join("link.json");
        write_json(&real, &serde_json::json!({"v": 1})).unwrap();
        std::os::unix::fs::symlink(&real, &link).unwrap();

        write_json(&link, &serde_json::json!({"v": 2})).unwrap();

        assert!(
            fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(read_json(&real).unwrap().unwrap()["v"], 2);
    }

    #[test]
    fn an_update_never_reverts_what_another_writer_just_saved() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.json");
        fs::write(&path, r#"{"numStartups": 1}"#).unwrap();
        let runs = std::cell::Cell::new(0);

        update_json(&path, |v| {
            runs.set(runs.get() + 1);
            match runs.get() {
                // Same length, same inode, same instant: only the content
                // says it moved.
                1 => fs::write(&path, r#"{"numStartups": 2}"#).unwrap(),
                2 => fs::write(&path, r#"{"numStartups": 2, "projects": {}}"#).unwrap(),
                _ => {}
            }
            v["oauthAccount"] = serde_json::json!({"accountUuid": "u"});
            Ok(())
        })
        .unwrap();

        assert_eq!(runs.get(), 3);
        let v = read_json(&path).unwrap().unwrap();
        assert_eq!(v["numStartups"], 2);
        assert!(v.get("projects").is_some());
        assert_eq!(v["oauthAccount"]["accountUuid"], "u");
        let leftovers = fs::read_dir(tmp.path()).unwrap().count();
        assert_eq!(leftovers, 1, "a temp file was left behind");
    }

    #[test]
    fn a_read_caught_halfway_through_another_write_is_retried() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.json");
        fs::write(&path, r#"{"numStartups": "#).unwrap();
        let mut pauses = 0;
        update_json_pacing(
            &path,
            |v| {
                v["k"] = serde_json::json!(1);
                Ok(())
            },
            || {
                pauses += 1;
                fs::write(&path, r#"{"numStartups": 3}"#).unwrap();
            },
        )
        .unwrap();
        assert_eq!(pauses, 1);
        let v = read_json(&path).unwrap().unwrap();
        assert_eq!(
            (v["numStartups"].clone(), v["k"].clone()),
            (3.into(), 1.into())
        );
    }

    #[test]
    fn an_update_gives_up_on_a_file_that_never_settles_and_leaves_it_be() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.json");
        fs::write(&path, r#"{"n": 0}"#).unwrap();
        let n = std::cell::Cell::new(0);
        let err = update_json(&path, |v| {
            n.set(n.get() + 1);
            fs::write(&path, format!(r#"{{"n": {}}}"#, n.get())).unwrap();
            v["mine"] = serde_json::json!(true);
            Ok(())
        })
        .unwrap_err();
        assert!(err.to_string().contains("kept changing"), "{err}");
        assert_eq!(
            read_json(&path).unwrap().unwrap(),
            serde_json::json!({"n": 10})
        );
        assert_eq!(fs::read_dir(tmp.path()).unwrap().count(), 1);

        fs::write(&path, "[1]").unwrap();
        let err = update_json(&path, |_| Ok(())).unwrap_err();
        assert!(err.to_string().contains("not a JSON object"), "{err}");
    }

    #[test]
    fn an_edit_that_refuses_writes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("auth.json");
        fs::write(&path, "{}").unwrap();
        let err = update_json(&path, |_| Err(Changed.into())).unwrap_err();
        assert!(err.is::<Changed>());
        assert_eq!(fs::read_to_string(&path).unwrap(), "{}");
        assert_eq!(fs::read_dir(tmp.path()).unwrap().count(), 1);
    }

    #[test]
    fn timestamps_are_utc_to_the_second() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339(951_782_400_000), "2000-02-29T00:00:00Z");
        assert_eq!(rfc3339(1_790_503_509_999), "2026-09-27T10:05:09Z");
    }
}
