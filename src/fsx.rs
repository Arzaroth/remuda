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

/// Writes through a sibling temp file and a rename, keeping the target's mode
/// when it exists and 0600 otherwise. A symlinked target is written where the
/// link points, so the link survives.
pub fn write_private(path: &Path, body: &str) -> Result<()> {
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
        fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result.with_context(|| format!("failed to write {}", path.display()))
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
    fn timestamps_are_utc_to_the_second() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339(951_782_400_000), "2000-02-29T00:00:00Z");
        assert_eq!(rfc3339(1_790_503_509_999), "2026-09-27T10:05:09Z");
    }
}
