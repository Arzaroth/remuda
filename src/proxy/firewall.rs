use std::ffi::OsStr;
use std::path::Path;
use std::time::Duration;

use serde::Serialize;

use crate::child;
use crate::doctor::which;

const ASK_FOR: Duration = Duration::from_secs(5);

/// A firewall that may keep other machines from the proxy, as far as it can
/// be read without root.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Verdict {
    pub name: &'static str,
    /// Whether it lets the port in, or `None` when its rules cannot be read.
    pub open: Option<bool>,
    /// What opens the port.
    pub commands: Vec<String>,
}

/// The firewall in front of `port`, if one is on: ufw from its files under
/// `etc`, else firewalld through the `firewall-cmd` found on `search`.
pub fn check(etc: &Path, search: Option<&OsStr>, port: u16) -> Option<Verdict> {
    ufw(etc, port).or_else(|| firewalld(search?, port))
}

fn ufw(etc: &Path, port: u16) -> Option<Verdict> {
    let conf = std::fs::read_to_string(etc.join("ufw/ufw.conf")).ok()?;
    if !setting(&conf, "ENABLED").is_some_and(|v| v.eq_ignore_ascii_case("yes")) {
        return None;
    }
    let policy = std::fs::read_to_string(etc.join("default/ufw"))
        .ok()
        .and_then(|d| setting(&d, "DEFAULT_INPUT_POLICY").map(|p| p == "ACCEPT"));
    let ruled = std::fs::read_to_string(etc.join("ufw/user.rules"))
        .ok()
        .map(|rules| rules.lines().find_map(|l| ufw_rule(l, port)));
    let open = match ruled {
        Some(Some(accepts)) => Some(accepts),
        Some(None) => policy.or(Some(false)),
        None => policy.filter(|accepts| *accepts),
    };
    Some(Verdict {
        name: "ufw",
        open,
        commands: vec![format!("sudo ufw allow {port}/tcp")],
    })
}

fn setting<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    text.lines().find_map(|l| {
        let (k, v) = l.trim().split_once('=')?;
        (k.trim() == key).then(|| v.trim().trim_matches('"'))
    })
}

/// Whether a line of ufw's `user.rules` accepts or refuses TCP on `port`.
fn ufw_rule(line: &str, port: u16) -> Option<bool> {
    let words: Vec<&str> = line.split_whitespace().collect();
    if words.get(..2) != Some(&["-A", "ufw-user-input"]) {
        return None;
    }
    let after = |flag: &str| {
        words
            .iter()
            .position(|w| *w == flag)
            .and_then(|i| words.get(i + 1).copied())
    };
    if after("-p").is_some_and(|p| p != "tcp" && p != "all") {
        return None;
    }
    if let Some(ports) = after("--dport").or_else(|| after("--dports"))
        && !ports.split(',').any(|range| covers(range, port))
    {
        return None;
    }
    match after("-j")? {
        "ACCEPT" | "ufw-user-limit-accept" => Some(true),
        "DROP" | "REJECT" => Some(false),
        _ => None,
    }
}

fn covers(range: &str, port: u16) -> bool {
    let bound = |s: &str| s.parse::<u16>().ok();
    match range.split_once(':') {
        Some((low, high)) => {
            bound(low).is_some_and(|l| l <= port) && bound(high).is_some_and(|h| port <= h)
        }
        None => bound(range) == Some(port),
    }
}

fn firewalld(search: &OsStr, port: u16) -> Option<Verdict> {
    let program = which("firewall-cmd", search)?;
    let running = child::run("firewall-cmd", &program, &["--state"], ASK_FOR, 4096).ok()?;
    if !running.ok {
        return None;
    }
    let query = format!("--query-port={port}/tcp");
    let open = child::run("firewall-cmd", &program, &[&query], ASK_FOR, 4096)
        .ok()
        .and_then(|r| match String::from_utf8_lossy(&r.out).trim() {
            "yes" => Some(true),
            "no" => Some(false),
            _ => None,
        });
    Some(Verdict {
        name: "firewalld",
        open,
        commands: vec![
            format!("sudo firewall-cmd --permanent --add-port={port}/tcp"),
            "sudo firewall-cmd --reload".to_owned(),
        ],
    })
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    use super::*;

    const RULES: &str = "*filter
:ufw-user-input - [0:0]
-A ufw-user-input -p udp --dport 8317 -j ACCEPT
-A ufw-user-input -p tcp --dport 22 -j ACCEPT
-A ufw-user-input -p tcp --dport 22 -m conntrack --ctstate NEW -m recent --set
-A ufw-user-input -p tcp --dport 22 -j ufw-user-limit
-A ufw-user-input -p tcp -m multiport --dports 80,9000:9100 -j ACCEPT
-A ufw-user-input -p tcp --dport 6000 -j DROP
COMMIT
";

    fn ufw_at(enabled: &str, policy: Option<&str>, rules: Option<&str>) -> tempfile::TempDir {
        let etc = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(etc.path().join("ufw")).unwrap();
        std::fs::create_dir_all(etc.path().join("default")).unwrap();
        std::fs::write(
            etc.path().join("ufw/ufw.conf"),
            format!("# comment\nENABLED={enabled}\nLOGLEVEL=low\n"),
        )
        .unwrap();
        if let Some(policy) = policy {
            std::fs::write(
                etc.path().join("default/ufw"),
                format!("IPV6=yes\nDEFAULT_INPUT_POLICY=\"{policy}\"\n"),
            )
            .unwrap();
        }
        if let Some(rules) = rules {
            std::fs::write(etc.path().join("ufw/user.rules"), rules).unwrap();
        }
        etc
    }

    fn open(etc: &tempfile::TempDir, port: u16) -> Option<bool> {
        check(etc.path(), None, port).expect("ufw is on").open
    }

    #[test]
    fn ufw_rules_say_which_ports_are_open() {
        let etc = ufw_at("yes", Some("DROP"), Some(RULES));
        assert_eq!(open(&etc, 22), Some(true));
        assert_eq!(open(&etc, 80), Some(true));
        assert_eq!(open(&etc, 9050), Some(true));
        assert_eq!(open(&etc, 6000), Some(false));
        assert_eq!(open(&etc, 8317), Some(false));
        let v = check(etc.path(), None, 8317).unwrap();
        assert_eq!(v.name, "ufw");
        assert_eq!(v.commands, ["sudo ufw allow 8317/tcp"]);
    }

    #[test]
    fn ufw_falls_back_on_its_policy() {
        let etc = ufw_at("yes", Some("ACCEPT"), Some(RULES));
        assert_eq!(open(&etc, 8317), Some(true));
        assert_eq!(open(&etc, 6000), Some(false));
        let etc = ufw_at("yes", Some("ACCEPT"), None);
        assert_eq!(open(&etc, 8317), Some(true));
        let etc = ufw_at("yes", Some("DROP"), None);
        assert_eq!(open(&etc, 8317), None);
        let etc = ufw_at("yes", None, Some(""));
        assert_eq!(open(&etc, 8317), Some(false));
    }

    #[test]
    fn no_firewall_when_ufw_is_off_and_firewalld_absent() {
        let etc = ufw_at("no", Some("DROP"), Some(RULES));
        assert_eq!(check(etc.path(), None, 8317), None);
        let empty = tempfile::tempdir().unwrap();
        assert_eq!(check(empty.path(), None, 8317), None);
        let bin = tempfile::tempdir().unwrap();
        assert_eq!(
            check(empty.path(), Some(bin.path().as_os_str()), 8317),
            None
        );
    }

    fn firewall_cmd(script: &str) -> (tempfile::TempDir, PathBuf) {
        let bin = tempfile::tempdir().unwrap();
        let path = bin.path().join("firewall-cmd");
        std::fs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        let dir = bin.path().to_owned();
        (bin, dir)
    }

    #[test]
    fn firewalld_is_asked_about_the_port() {
        let empty = tempfile::tempdir().unwrap();
        let (_bin, dir) = firewall_cmd(
            r#"case "$1" in
--state) echo running ;;
--query-port=8317/tcp) echo no; exit 1 ;;
--query-port=*) echo yes ;;
esac"#,
        );
        let v = check(empty.path(), Some(dir.as_os_str()), 8317).unwrap();
        assert_eq!(v.name, "firewalld");
        assert_eq!(v.open, Some(false));
        assert_eq!(
            v.commands,
            [
                "sudo firewall-cmd --permanent --add-port=8317/tcp",
                "sudo firewall-cmd --reload"
            ]
        );
        let v = check(empty.path(), Some(dir.as_os_str()), 9000).unwrap();
        assert_eq!(v.open, Some(true));
    }

    #[test]
    fn a_stopped_firewalld_is_no_firewall() {
        let empty = tempfile::tempdir().unwrap();
        let (_bin, dir) = firewall_cmd("echo 'not running'; exit 252");
        assert_eq!(check(empty.path(), Some(dir.as_os_str()), 8317), None);
    }

    #[test]
    fn ufw_wins_over_firewalld() {
        let etc = ufw_at("yes", Some("DROP"), Some(RULES));
        let (_bin, dir) = firewall_cmd("echo running");
        let v = check(etc.path(), Some(dir.as_os_str()), 8317).unwrap();
        assert_eq!(v.name, "ufw");
    }
}
