use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Result, bail};

use crate::fsx;
use crate::units::{Refreshed, Unit};

pub const SERVE: &str = "com.github.arzaroth.remuda.serve";
pub const REFRESH: &str = "com.github.arzaroth.remuda.refresh";

/// The variables the shell may set that move what remuda reads, copied into
/// the agents so they look where it does.
const PASSED: [&str; 9] = [
    "CLAUDE_CONFIG_DIR",
    "CODEX_HOME",
    "GROK_HOME",
    "GROK_AUTH_PATH",
    "KIMI_CODE_HOME",
    "CURSOR_CONFIG_DIR",
    "AGENT_CLI_CREDENTIAL_STORE",
    "REMUDA_STORE",
    "REMUDA_CONFIG",
];

pub fn label(unit: Unit) -> &'static str {
    match unit {
        Unit::Serve => SERVE,
        Unit::Timer => REFRESH,
    }
}

pub fn agents_dir(home: &Path) -> PathBuf {
    home.join("Library/LaunchAgents")
}

pub fn plist_path(dir: &Path, unit: Unit) -> PathBuf {
    dir.join(format!("{}.plist", label(unit)))
}

pub fn logs(home: &Path, unit: Unit) -> PathBuf {
    let name = match unit {
        Unit::Serve => "serve.log",
        Unit::Timer => "refresh.log",
    };
    home.join("Library/Logs/remuda").join(name)
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// The agent's plist: `~/.local/bin/remuda`, as install.sh puts it, with
/// the directories `env` gives.
pub fn plist(unit: Unit, home: &Path, env: &dyn Fn(&str) -> Option<String>) -> String {
    let exe = home.join(".local/bin/remuda");
    let args: &[&str] = match unit {
        Unit::Serve => &["serve"],
        Unit::Timer => &["refresh", "--scheduled"],
    };
    let mut vars = vec![
        ("REMUDA_SERVICE".to_owned(), "1".to_owned()),
        (
            "PATH".to_owned(),
            format!(
                "{}:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin",
                home.join(".local/bin").display()
            ),
        ),
    ];
    vars.extend(PASSED.iter().filter_map(|v| {
        env(v)
            .filter(|x| !x.is_empty())
            .map(|x| ((*v).to_owned(), x))
    }));
    let mut out = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\">\n<dict>\n",
    );
    let line = |out: &mut String, key: &str, value: &str| {
        out.push_str(&format!(
            "\t<key>{key}</key>\n\t<string>{}</string>\n",
            escape(value)
        ));
    };
    line(&mut out, "Label", label(unit));
    out.push_str("\t<key>ProgramArguments</key>\n\t<array>\n");
    for arg in std::iter::once(exe.to_string_lossy().into_owned())
        .chain(args.iter().map(|a| (*a).to_owned()))
    {
        out.push_str(&format!("\t\t<string>{}</string>\n", escape(&arg)));
    }
    out.push_str("\t</array>\n\t<key>EnvironmentVariables</key>\n\t<dict>\n");
    for (k, v) in &vars {
        out.push_str(&format!(
            "\t\t<key>{k}</key>\n\t\t<string>{}</string>\n",
            escape(v)
        ));
    }
    out.push_str("\t</dict>\n");
    match unit {
        Unit::Serve => out.push_str(
            "\t<key>RunAtLoad</key>\n\t<true/>\n\t<key>KeepAlive</key>\n\t<dict>\n\t\t<key>Crashed</key>\n\t\t<true/>\n\t</dict>\n",
        ),
        Unit::Timer => out.push_str("\t<key>StartInterval</key>\n\t<integer>1800</integer>\n"),
    }
    let log = logs(home, unit).to_string_lossy().into_owned();
    line(&mut out, "StandardOutPath", &log);
    line(&mut out, "StandardErrorPath", &log);
    out.push_str("</dict>\n</plist>\n");
    out
}

/// Rewrites the agents installed in `dir` whose plist differs from this
/// binary's, as [`crate::units::refresh`] does the systemd units.
pub fn refresh(
    dir: &Path,
    home: &Path,
    env: &dyn Fn(&str) -> Option<String>,
) -> (Refreshed, Vec<Unit>) {
    let mut done = Refreshed::default();
    let mut changed = Vec::new();
    for unit in [Unit::Timer, Unit::Serve] {
        let path = plist_path(dir, unit);
        let body = plist(unit, home, env);
        let installed = std::fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_file());
        if !installed || std::fs::read_to_string(&path).is_ok_and(|text| text == body) {
            continue;
        }
        match fsx::write_private(&path, &body) {
            Ok(()) => {
                done.changed.push(label(unit));
                changed.push(unit);
            }
            Err(e) => done
                .failed
                .push(format!("cannot update {}: {e:#}", label(unit))),
        }
    }
    (done, changed)
}

pub fn domain() -> String {
    // SAFETY: getuid cannot fail.
    format!("gui/{}", unsafe { libc::getuid() })
}

fn launchctl(args: &[&str]) -> Option<(bool, String)> {
    #[cfg(debug_assertions)]
    let program =
        std::env::var_os("REMUDA_TEST_LAUNCHCTL").unwrap_or_else(|| "/bin/launchctl".into());
    #[cfg(not(debug_assertions))]
    let program = "/bin/launchctl";
    let out = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    Some((
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
    ))
}

fn target(unit: Unit) -> String {
    format!("{}/{}", domain(), label(unit))
}

/// The pid launchd runs the agent as, from `launchctl print`'s `pid = N`.
pub fn pid_in(printed: &str) -> Option<u32> {
    printed
        .lines()
        .find_map(|l| l.trim().strip_prefix("pid = "))
        .and_then(|p| p.trim().parse().ok())
}

/// Whether `print-disabled` lists the agent as disabled.
pub fn disabled_in(printed: &str, unit: Unit) -> bool {
    let quoted = format!("\"{}\"", label(unit));
    printed.lines().any(|l| {
        let l = l.trim();
        l.starts_with(&quoted) && (l.ends_with("disabled") || l.ends_with("true"))
    })
}

pub fn loaded(unit: Unit) -> Option<String> {
    match launchctl(&["print", &target(unit)])? {
        (true, printed) => Some(printed),
        (false, _) => None,
    }
}

pub fn state(dir: &Path, unit: Unit) -> &'static str {
    if !plist_path(dir, unit).exists() {
        return "absent";
    }
    let disabled = launchctl(&["print-disabled", &domain()])
        .is_some_and(|(_, printed)| disabled_in(&printed, unit));
    match (disabled, loaded(unit).is_some()) {
        (true, _) => "masked",
        (false, true) => "enabled",
        (false, false) => "disabled",
    }
}

/// Whether launchd runs `pid` as the agent.
pub fn runs(unit: Unit, pid: u32) -> bool {
    loaded(unit).and_then(|p| pid_in(&p)) == Some(pid)
}

/// Loads the agent, or restarts it when it is loaded already.
pub fn start(dir: &Path, unit: Unit) -> bool {
    if loaded(unit).is_some() {
        return launchctl(&["kickstart", "-k", &target(unit)]).is_some_and(|(ok, _)| ok);
    }
    let plist = plist_path(dir, unit);
    launchctl(&["bootstrap", &domain(), &plist.to_string_lossy()]).is_some_and(|(ok, _)| ok)
}

/// Writes the agents asked for and loads them, replacing what runs.
pub fn install(
    dir: &Path,
    home: &Path,
    units: &[Unit],
    env: &dyn Fn(&str) -> Option<String>,
) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    for unit in units {
        if let Some(parent) = logs(home, *unit).parent() {
            std::fs::create_dir_all(parent)?;
        }
        let path = plist_path(dir, *unit);
        fsx::write_private(&path, &plist(*unit, home, env))?;
        let _ = launchctl(&["bootout", &target(*unit)]);
        let _ = launchctl(&["enable", &target(*unit)]);
        if !launchctl(&["bootstrap", &domain(), &path.to_string_lossy()]).is_some_and(|(ok, _)| ok)
        {
            bail!(
                "launchctl could not load {}; is anyone logged in to the Mac's desktop?",
                path.display()
            );
        }
        println!("Loaded {}", label(*unit));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::units::{Action, Manager};

    fn env(vars: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |v| {
            vars.iter()
                .find(|(k, _)| *k == v)
                .map(|(_, x)| (*x).to_owned())
        }
    }

    #[test]
    fn serve_s_plist_runs_the_installed_binary_with_the_shell_s_directories() {
        let home = Path::new("/Users/me");
        let plist = plist(
            Unit::Serve,
            home,
            &env(&[
                ("REMUDA_STORE", "/Users/me/a&b"),
                ("CODEX_HOME", ""),
                ("TERM", "x"),
            ]),
        );
        assert!(plist.contains("<string>com.github.arzaroth.remuda.serve</string>"));
        assert!(
            plist.contains(
                "<string>/Users/me/.local/bin/remuda</string>\n\t\t<string>serve</string>"
            )
        );
        assert!(plist.contains("<key>REMUDA_SERVICE</key>\n\t\t<string>1</string>"));
        assert!(plist.contains("<key>REMUDA_STORE</key>\n\t\t<string>/Users/me/a&amp;b</string>"));
        assert!(!plist.contains("CODEX_HOME") && !plist.contains("TERM"));
        assert!(plist.contains("<string>/Users/me/.local/bin:/opt/homebrew/bin:"));
        assert!(plist.contains("<key>Crashed</key>"));
        assert!(plist.contains("/Users/me/Library/Logs/remuda/serve.log"));
        assert!(!plist.contains("StartInterval"));
    }

    #[test]
    fn the_refresh_runs_every_half_hour() {
        let plist = plist(Unit::Timer, Path::new("/Users/me"), &env(&[]));
        assert!(plist.contains("<string>refresh</string>\n\t\t<string>--scheduled</string>"));
        assert!(plist.contains("<key>StartInterval</key>\n\t<integer>1800</integer>"));
        assert!(!plist.contains("KeepAlive"));
        assert!(plist.contains("refresh.log"));
    }

    #[test]
    fn only_installed_agents_that_differ_are_rewritten() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("LaunchAgents");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(plist_path(&dir, Unit::Timer), "old").unwrap();
        let (done, changed) = refresh(&dir, tmp.path(), &env(&[]));
        assert_eq!(done.changed, [REFRESH]);
        assert_eq!(changed, [Unit::Timer]);
        assert!(!plist_path(&dir, Unit::Serve).exists());
        assert_eq!(
            std::fs::read_to_string(plist_path(&dir, Unit::Timer)).unwrap(),
            plist(Unit::Timer, tmp.path(), &env(&[]))
        );
        let (done, _) = refresh(&dir, tmp.path(), &env(&[]));
        assert!(done.changed.is_empty());
    }

    #[test]
    fn launchctl_s_answers_are_read() {
        let printed = "gui/501/com.github.arzaroth.remuda.serve = {\n\tactive count = 1\n\tpid = 4242\n\tstate = running\n}\n";
        assert_eq!(pid_in(printed), Some(4242));
        assert_eq!(pid_in("state = not running\n"), None);
        let disabled = "disabled services = {\n\t\"com.github.arzaroth.remuda.refresh\" => disabled\n\t\"com.github.arzaroth.remuda.serve\" => enabled\n}\n";
        assert!(disabled_in(disabled, Unit::Timer));
        assert!(!disabled_in(disabled, Unit::Serve));
    }

    #[test]
    fn the_fixes_name_launchctl_and_the_agent() {
        let m = Manager::Launchd {
            dir: PathBuf::from("/Users/me/Library/LaunchAgents"),
            home: PathBuf::from("/Users/me"),
        };
        assert_eq!(m.name(Unit::Serve), SERVE);
        assert_eq!(
            m.fix(Unit::Timer, Action::Enable),
            format!("`launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/{REFRESH}.plist`")
        );
        assert_eq!(
            m.fix(Unit::Serve, Action::Restart),
            format!("`launchctl kickstart -k gui/$(id -u)/{SERVE}`")
        );
        assert_eq!(
            m.logs(Unit::Serve),
            "/Users/me/Library/Logs/remuda/serve.log"
        );
    }
}
