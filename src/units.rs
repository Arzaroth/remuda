use std::path::{Path, PathBuf};

use crate::fsx;

pub const TIMER: &str = "remuda-refresh.timer";
pub const SERVE: &str = "remuda-serve.service";

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unit {
    Serve,
    Timer,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Enable,
    Unmask,
    Restart,
    Status,
}

/// What runs `serve` and the scheduled refresh: systemd's user units, found
/// in these directories, or on macOS launchd's agents in this one.
pub enum Manager {
    Systemd(Vec<PathBuf>),
    Launchd { dir: PathBuf, home: PathBuf },
}

/// Where a service's settings tell it where the CLIs keep their logins.
pub fn env_hint() -> &'static str {
    match cfg!(target_os = "macos") {
        true => "the EnvironmentVariables of the agents in ~/Library/LaunchAgents",
        false => "~/.config/environment.d/60-remuda.conf",
    }
}

fn systemctl(args: &[&str]) -> bool {
    std::process::Command::new("systemctl")
        .arg("--user")
        .args(args)
        .stdin(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

impl Manager {
    pub fn from_env() -> Self {
        if cfg!(target_os = "macos") {
            let home = crate::paths::home();
            return Manager::Launchd {
                dir: crate::launchd::agents_dir(&home),
                home,
            };
        }
        Manager::Systemd(crate::paths::systemd_user_units())
    }

    pub fn name(&self, unit: Unit) -> String {
        match (self, unit) {
            (Manager::Systemd(_), Unit::Serve) => SERVE.to_owned(),
            (Manager::Systemd(_), Unit::Timer) => TIMER.to_owned(),
            (Manager::Launchd { .. }, unit) => crate::launchd::label(unit).to_owned(),
        }
    }

    /// Where to read what the service printed.
    pub fn logs(&self, unit: Unit) -> String {
        match self {
            Manager::Systemd(_) => format!(
                "`journalctl --user -u {}`",
                self.name(unit).trim_end_matches(".service")
            ),
            Manager::Launchd { home, .. } => crate::launchd::logs(home, unit).display().to_string(),
        }
    }

    /// Whether this service runs the process `pid`.
    pub fn runs(&self, unit: Unit, pid: u32) -> bool {
        match self {
            Manager::Systemd(_) => {
                let cgroup =
                    std::fs::read_to_string(format!("/proc/{pid}/cgroup")).unwrap_or_default();
                crate::dirs::in_remuda_unit(&cgroup)
            }
            Manager::Launchd { .. } => crate::launchd::runs(unit, pid),
        }
    }

    /// Starts the service when it is installed.
    pub fn start(&self, unit: Unit) -> bool {
        match self {
            Manager::Systemd(dirs) => {
                let name = self.name(unit);
                dirs.iter().any(|dir| dir.join(&name).exists()) && systemctl(&["start", &name])
            }
            Manager::Launchd { dir, .. } => {
                crate::launchd::plist_path(dir, unit).exists() && crate::launchd::start(dir, unit)
            }
        }
    }

    /// Brings the installed services up to this binary and restarts a
    /// running `serve` on it.
    pub fn sync(&self) -> anyhow::Result<()> {
        let env = |v: &str| std::env::var(v).ok();
        let done = match self {
            Manager::Systemd(_) => {
                let done = refresh(&crate::paths::installed_units());
                if !done.changed.is_empty() {
                    println!("updated {}", done.changed.join(", "));
                    if !systemctl(&["daemon-reload"]) {
                        eprintln!("warning: run `systemctl --user daemon-reload`");
                    }
                }
                if systemctl(&["is-active", "--quiet", SERVE]) && !systemctl(&["restart", SERVE]) {
                    eprintln!(
                        "warning: {SERVE} still runs the old binary; run {}",
                        self.fix(Unit::Serve, Action::Restart)
                    );
                }
                done
            }
            Manager::Launchd { dir, home } => {
                let (done, changed) = crate::launchd::refresh(dir, home, &env);
                if !done.changed.is_empty() {
                    println!("updated {}", done.changed.join(", "));
                }
                for unit in [Unit::Timer, Unit::Serve] {
                    let loaded = crate::launchd::loaded(unit).is_some();
                    let restart = unit == Unit::Serve || changed.contains(&unit);
                    if loaded && restart && !crate::launchd::start(dir, unit) {
                        eprintln!(
                            "warning: {} still runs the old binary; run {}",
                            self.name(unit),
                            self.fix(unit, Action::Restart)
                        );
                    }
                }
                done
            }
        };
        if !done.failed.is_empty() {
            anyhow::bail!("{}", done.failed.join("\n"));
        }
        Ok(())
    }

    /// `enabled`, `disabled`, `masked` or `absent`.
    pub fn state(&self, unit: Unit) -> &'static str {
        match self {
            Manager::Launchd { dir, .. } => crate::launchd::state(dir, unit),
            Manager::Systemd(dirs) => {
                let wants = match unit {
                    Unit::Serve => "default.target.wants",
                    Unit::Timer => "timers.target.wants",
                };
                state(dirs, &self.name(unit), wants)
            }
        }
    }

    /// The command that does `action` to `unit`, quoted for a fix line.
    pub fn fix(&self, unit: Unit, action: Action) -> String {
        let name = self.name(unit);
        match self {
            Manager::Systemd(_) => {
                let verb = match action {
                    Action::Enable => "enable --now",
                    Action::Unmask => "unmask",
                    Action::Restart => "restart",
                    Action::Status => "status",
                };
                format!("`systemctl --user {verb} {name}`")
            }
            Manager::Launchd { .. } => match action {
                Action::Enable => format!(
                    "`launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/{name}.plist`"
                ),
                Action::Unmask => format!("`launchctl enable gui/$(id -u)/{name}`"),
                Action::Restart => format!("`launchctl kickstart -k gui/$(id -u)/{name}`"),
                Action::Status => format!("`launchctl print gui/$(id -u)/{name}`"),
            },
        }
    }
}

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

/// `enabled`, `disabled`, `masked` or `absent`, as systemd would find `unit`
/// in `dirs` and pulled in by `wants`.
pub fn state(dirs: &[PathBuf], unit: &str, wants: &str) -> &'static str {
    let found = dirs
        .iter()
        .map(|d| d.join(unit))
        .find(|p| p.symlink_metadata().is_ok());
    let masked = found
        .as_ref()
        .is_some_and(|p| std::fs::canonicalize(p).is_ok_and(|t| t == Path::new("/dev/null")));
    let wanted = dirs
        .iter()
        .any(|d| d.join(wants).join(unit).metadata().is_ok());
    match (found, masked, wanted) {
        (_, true, _) => "masked",
        (Some(_), false, true) => "enabled",
        (Some(_), false, false) => "disabled",
        (None, ..) => "absent",
    }
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
    fn every_service_says_it_runs_as_one() {
        for (name, body) in UNITS.iter().filter(|(name, _)| name.ends_with(".service")) {
            assert!(
                body.contains("Environment=REMUDA_SERVICE=1") || body.contains("--scheduled"),
                "{name}"
            );
        }
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
