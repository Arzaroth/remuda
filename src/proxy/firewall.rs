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
/// `etc`, else firewalld through the `firewall-cmd` found on `search`, in the
/// zone of `interface`, the one LAN traffic comes in on.
pub fn check(
    etc: &Path,
    search: Option<&OsStr>,
    interface: Option<&str>,
    port: u16,
) -> Option<Verdict> {
    ufw(etc, port).or_else(|| firewalld(search?, interface, port))
}

fn ufw(etc: &Path, port: u16) -> Option<Verdict> {
    let conf = std::fs::read_to_string(etc.join("ufw/ufw.conf")).ok()?;
    if !setting(&conf, "ENABLED").is_some_and(|v| v.eq_ignore_ascii_case("yes")) {
        return None;
    }
    let policy = std::fs::read_to_string(etc.join("default/ufw"))
        .ok()
        .and_then(|d| setting(&d, "DEFAULT_INPUT_POLICY").map(|p| p == "ACCEPT"));
    let open = std::fs::read_to_string(etc.join("ufw/user.rules"))
        .ok()
        .map(|rules| ufw_rules(&rules, port, policy));
    let (open, denied) = match open {
        Some((open, denied)) => (open, denied),
        None => (None, false),
    };
    let allow = if denied { "insert 1 allow" } else { "allow" };
    Some(Verdict {
        name: "ufw",
        open,
        commands: vec![format!("sudo ufw {allow} {port}/tcp")],
    })
}

/// Whether ufw lets any LAN machine in on `port`, `None` when only some
/// sources are let in, and whether a rule of its own refuses the port, which
/// an appended allow would come after.
fn ufw_rules(rules: &str, port: u16, policy: Option<bool>) -> (Option<bool>, bool) {
    let mut some_sources = false;
    let mut decided = None;
    for rule in rules.lines().filter_map(|l| ufw_rule(l, port)) {
        match rule {
            Rule {
                restricted: true,
                accepts,
            } => some_sources |= accepts,
            Rule { accepts, .. } => {
                decided = Some(accepts);
                break;
            }
        }
    }
    let open = decided.or(policy).unwrap_or(false);
    let verdict = if !open && some_sources {
        None
    } else {
        Some(open)
    };
    (verdict, decided == Some(false))
}

fn setting<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    text.lines().find_map(|l| {
        let (k, v) = l.trim().split_once('=')?;
        (k.trim() == key).then(|| v.trim().trim_matches('"'))
    })
}

struct Rule {
    accepts: bool,
    /// Limited to some sources, so it decides nothing for the others.
    restricted: bool,
}

/// Whether a line of ufw's `user.rules` accepts or refuses TCP on `port`.
fn ufw_rule(line: &str, port: u16) -> Option<Rule> {
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
    let accepts = match after("-j")? {
        "ACCEPT" | "ufw-user-limit-accept" => true,
        "DROP" | "REJECT" => false,
        _ => return None,
    };
    Some(Rule {
        accepts,
        restricted: after("-s").is_some(),
    })
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

fn firewalld(search: &OsStr, interface: Option<&str>, port: u16) -> Option<Verdict> {
    let program = which("firewall-cmd", search)?;
    let ask = |arg: &str| child::run("firewall-cmd", &program, &[arg], ASK_FOR, 4096).ok();
    if !ask("--state")?.ok {
        return None;
    }
    let zone = interface
        .and_then(|i| ask(&format!("--get-zone-of-interface={i}")))
        .filter(|r| r.ok)
        .map(|r| String::from_utf8_lossy(&r.out).trim().to_owned())
        .filter(|z| !z.is_empty());
    let zoned = zone.map(|z| format!(" --zone={z}")).unwrap_or_default();
    let query = format!("{zoned} --query-port={port}/tcp");
    let args: Vec<&str> = query.split_whitespace().collect();
    let open = child::run("firewall-cmd", &program, &args, ASK_FOR, 4096)
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
            format!("sudo firewall-cmd --permanent{zoned} --add-port={port}/tcp"),
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
        check(etc.path(), None, None, port).expect("ufw is on").open
    }

    #[test]
    fn ufw_rules_say_which_ports_are_open() {
        let etc = ufw_at("yes", Some("DROP"), Some(RULES));
        assert_eq!(open(&etc, 22), Some(true));
        assert_eq!(open(&etc, 80), Some(true));
        assert_eq!(open(&etc, 9050), Some(true));
        assert_eq!(open(&etc, 6000), Some(false));
        assert_eq!(open(&etc, 8317), Some(false));
        let v = check(etc.path(), None, None, 8317).unwrap();
        assert_eq!(v.name, "ufw");
        assert_eq!(v.commands, ["sudo ufw allow 8317/tcp"]);
    }

    #[test]
    fn ufw_falls_back_on_its_policy() {
        let etc = ufw_at("yes", Some("ACCEPT"), Some(RULES));
        assert_eq!(open(&etc, 8317), Some(true));
        assert_eq!(open(&etc, 6000), Some(false));
        let etc = ufw_at("yes", Some("ACCEPT"), None);
        assert_eq!(open(&etc, 8317), None);
        let etc = ufw_at("yes", Some("DROP"), None);
        assert_eq!(open(&etc, 8317), None);
        let etc = ufw_at("yes", None, Some(""));
        assert_eq!(open(&etc, 8317), Some(false));
    }

    #[test]
    fn a_source_limited_rule_decides_nothing_for_other_machines() {
        let rules = "-A ufw-user-input -p tcp --dport 8317 -s 192.168.1.9 -j ACCEPT
-A ufw-user-input -p tcp --dport 7000 -s 192.168.1.9 -j DROP
";
        let etc = ufw_at("yes", Some("DROP"), Some(rules));
        assert_eq!(open(&etc, 8317), None);
        let etc = ufw_at("yes", Some("ACCEPT"), Some(rules));
        assert_eq!(open(&etc, 8317), Some(true));
        assert_eq!(open(&etc, 7000), Some(true));
    }

    #[test]
    fn a_deny_rule_is_overtaken_rather_than_appended_after() {
        let etc = ufw_at("yes", Some("DROP"), Some(RULES));
        let v = check(etc.path(), None, None, 6000).unwrap();
        assert_eq!(v.open, Some(false));
        assert_eq!(v.commands, ["sudo ufw insert 1 allow 6000/tcp"]);
    }

    #[test]
    fn no_firewall_when_ufw_is_off_and_firewalld_absent() {
        let etc = ufw_at("no", Some("DROP"), Some(RULES));
        assert_eq!(check(etc.path(), None, None, 8317), None);
        let empty = tempfile::tempdir().unwrap();
        assert_eq!(check(empty.path(), None, None, 8317), None);
        let bin = tempfile::tempdir().unwrap();
        assert_eq!(
            check(empty.path(), Some(bin.path().as_os_str()), None, 8317),
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
        let v = check(empty.path(), Some(dir.as_os_str()), None, 8317).unwrap();
        assert_eq!(v.name, "firewalld");
        assert_eq!(v.open, Some(false));
        assert_eq!(
            v.commands,
            [
                "sudo firewall-cmd --permanent --add-port=8317/tcp",
                "sudo firewall-cmd --reload"
            ]
        );
        let v = check(empty.path(), Some(dir.as_os_str()), None, 9000).unwrap();
        assert_eq!(v.open, Some(true));
    }

    #[test]
    fn a_stopped_firewalld_is_no_firewall() {
        let empty = tempfile::tempdir().unwrap();
        let (_bin, dir) = firewall_cmd("echo 'not running'; exit 252");
        assert_eq!(check(empty.path(), Some(dir.as_os_str()), None, 8317), None);
    }

    #[test]
    fn ufw_wins_over_firewalld() {
        let etc = ufw_at("yes", Some("DROP"), Some(RULES));
        let (_bin, dir) = firewall_cmd("echo running");
        let v = check(etc.path(), Some(dir.as_os_str()), None, 8317).unwrap();
        assert_eq!(v.name, "ufw");
    }
}
