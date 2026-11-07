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
