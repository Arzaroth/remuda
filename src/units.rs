use std::path::Path;

use crate::fsx;

pub const UNITS: [(&str, &str); 3] = [
    (
        "remuda-refresh.service",
        include_str!("../systemd/remuda-refresh.service"),
    ),
    (
        "remuda-refresh.timer",
        include_str!("../systemd/remuda-refresh.timer"),
    ),
    (
        "remuda-serve.service",
        include_str!("../systemd/remuda-serve.service"),
    ),
];

#[derive(Debug, Default)]
pub struct Refreshed {
    pub changed: Vec<&'static str>,
    pub failed: Vec<String>,
}

/// Rewrites the units already installed in `dir` that differ from this
/// binary's. A unit that is not there was not asked for, and a link (a masked
/// unit, or one the user points elsewhere) is theirs. One that cannot be
/// written does not stop the others, which still need reloading.
pub fn refresh(dir: &Path) -> Refreshed {
    let mut done = Refreshed::default();
    for (name, body) in UNITS {
        let path = dir.join(name);
        let installed = std::fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_file());
        if !installed || std::fs::read_to_string(&path).is_ok_and(|text| text == body) {
            continue;
        }
        match fsx::write_private(&path, body) {
            Ok(()) => done.changed.push(name),
            Err(e) => done.failed.push(format!("cannot update {name}: {e:#}")),
        }
    }
    done
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn only_installed_units_that_differ_are_rewritten() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        std::fs::write(dir.join("remuda-refresh.service"), "old").unwrap();
        std::fs::set_permissions(
            dir.join("remuda-refresh.service"),
            std::fs::Permissions::from_mode(0o644),
        )
        .unwrap();
        std::fs::write(dir.join("remuda-refresh.timer"), UNITS[1].1).unwrap();
        std::os::unix::fs::symlink("/dev/null", dir.join("remuda-serve.service")).unwrap();

        assert_eq!(refresh(dir).changed, ["remuda-refresh.service"]);
        assert_eq!(
            std::fs::read_to_string(dir.join("remuda-refresh.service")).unwrap(),
            UNITS[0].1
        );
        let mode = std::fs::metadata(dir.join("remuda-refresh.service"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o644);
        assert!(
            std::fs::symlink_metadata(dir.join("remuda-serve.service"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert!(refresh(dir).changed.is_empty());
    }

    #[test]
    fn a_unit_that_cannot_be_written_does_not_stop_the_others() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        std::fs::write(dir.join("remuda-refresh.service"), "old").unwrap();
        std::fs::write(dir.join("remuda-refresh.timer"), "old").unwrap();
        let blocker = dir.join(format!(
            ".remuda-refresh.service.remuda-{}",
            std::process::id()
        ));
        std::fs::create_dir(&blocker).unwrap();

        let done = refresh(dir);

        assert_eq!(done.changed, ["remuda-refresh.timer"]);
        assert_eq!(done.failed.len(), 1);
        assert!(
            done.failed[0].contains("remuda-refresh.service"),
            "{:?}",
            done.failed
        );
    }

    #[test]
    fn units_nobody_installed_are_left_out() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(refresh(tmp.path()).changed.is_empty());
        assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), 0);
        let missing = refresh(&tmp.path().join("missing"));
        assert!(missing.changed.is_empty() && missing.failed.is_empty());
    }

    #[test]
    fn every_unit_the_release_ships_is_carried() {
        let mut shipped: Vec<String> =
            std::fs::read_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/systemd"))
                .unwrap()
                .map(|e| e.unwrap().file_name().into_string().unwrap())
                .collect();
        shipped.sort();
        let carried: Vec<&str> = UNITS.iter().map(|(name, _)| *name).collect();
        assert_eq!(shipped, carried);
    }
}
