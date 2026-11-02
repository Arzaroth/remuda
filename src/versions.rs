//! The page's Versions view: the remuda and TokenGauge running here, the
//! releases GitHub has, and updating either one.

use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use selvedge::update::version_gt;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::child;
use crate::project::REMUDA;
use crate::served;

pub const GITHUB: &str = "https://api.github.com";

const TOKENGAUGE_REPO: &str = "Arzaroth/TokenGauge";

/// The releases a version newer than the installed one may bring notes for.
const SHOWN: usize = 10;

#[derive(Debug, Clone, Serialize)]
pub struct Release {
    pub version: String,
    pub published: Option<String>,
    pub url: String,
    pub notes: String,
}

#[derive(Deserialize)]
struct Listed {
    tag_name: String,
    published_at: Option<String>,
    html_url: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
}

/// `repo`'s published releases, newest first.
pub fn releases(client: &reqwest::blocking::Client, api: &str, repo: &str) -> Result<Vec<Release>> {
    let resp = client
        .get(format!("{api}/repos/{repo}/releases?per_page=30"))
        .header("Accept", "application/vnd.github+json")
        .send()
        .context("could not reach GitHub")?;
    let status = resp.status();
    if !status.is_success() {
        bail!("GitHub answered {status}");
    }
    let listed: Vec<Listed> = resp.json().context("GitHub's release list is malformed")?;
    Ok(listed
        .into_iter()
        .filter(|r| !r.draft && !r.prerelease)
        .map(|r| Release {
            version: r.tag_name.trim_start_matches('v').to_owned(),
            published: r.published_at,
            url: r.html_url,
            notes: r.body.unwrap_or_default(),
        })
        .collect())
}

/// One program as this machine has it, before GitHub is asked.
pub struct Tool {
    pub id: &'static str,
    pub name: &'static str,
    pub repo: String,
    pub current: Option<String>,
    /// Why the page cannot update it: a package manager owns it, or it is
    /// not installed.
    pub held: Option<String>,
}

impl Tool {
    pub fn remuda() -> Self {
        let held = match selvedge::update::owner(&REMUDA) {
            Ok(owner) => owner.map(|o| o.advice(&REMUDA)),
            Err(e) => Some(format!("{e:#}")),
        };
        Tool {
            id: "remuda",
            name: "remuda",
            repo: REMUDA.repo(),
            current: Some(REMUDA.version.to_owned()),
            held,
        }
    }

    pub fn tokengauge(program: Option<&Path>) -> Self {
        let repo = std::env::var("TOKENGAUGE_REPO").unwrap_or_else(|_| TOKENGAUGE_REPO.into());
        let current = program.and_then(|p| tokengauge_version(p).ok());
        let held = match program {
            None => Some("TokenGauge is not installed".to_owned()),
            Some(_) if current.is_none() => Some("TokenGauge does not answer `--version`".to_owned()),
            Some(p) if packaged(p) => Some(
                "TokenGauge was installed by the system's package manager - upgrade it through that instead"
                    .to_owned(),
            ),
            Some(_) => None,
        };
        Tool {
            id: "tokengauge",
            name: "TokenGauge",
            repo,
            current,
            held,
        }
    }

    /// What the page shows: the notes of every release newer than the one
    /// installed, or of the latest when there is none.
    pub fn report(&self, listed: Result<Vec<Release>>) -> Value {
        let mut out = json!({
            "id": self.id,
            "name": self.name,
            "url": format!("https://github.com/{}/releases", self.repo),
            "current": self.current,
            "held": self.held,
            "latest": null,
            "available": false,
            "releases": [],
        });
        match listed {
            Ok(all) => {
                let newer: Vec<&Release> = match &self.current {
                    Some(current) => all
                        .iter()
                        .filter(|r| version_gt(&r.version, current))
                        .take(SHOWN)
                        .collect(),
                    None => Vec::new(),
                };
                out["latest"] = json!(all.first().map(|r| &r.version));
                out["available"] = json!(!newer.is_empty());
                out["releases"] = if newer.is_empty() {
                    json!(all.first().into_iter().collect::<Vec<_>>())
                } else {
                    json!(newer)
                };
            }
            Err(e) => out["error"] = json!(format!("{e:#}")),
        }
        out
    }
}

/// `/usr` outside `/usr/local` is the distribution's, as selvedge reads it.
fn packaged(program: &Path) -> bool {
    let resolved = std::fs::canonicalize(program).unwrap_or_else(|_| program.to_owned());
    resolved.starts_with("/usr") && !resolved.starts_with("/usr/local")
}

fn tokengauge_version(program: &Path) -> Result<String> {
    let ran = child::run(
        "TokenGauge",
        program,
        &["--version"],
        Duration::from_secs(5),
        4096,
    )?;
    let out = String::from_utf8_lossy(&ran.out);
    let version = out.split_whitespace().last().unwrap_or_default();
    if !ran.ok || !version.starts_with(|c: char| c.is_ascii_digit()) {
        bail!("TokenGauge did not print its version");
    }
    Ok(version.to_owned())
}

/// Every tool's report, asking GitHub once per tool.
pub fn check(client: &reqwest::blocking::Client, api: &str, tokengauge: Option<&Path>) -> Value {
    let tools: Vec<Value> = [Tool::remuda(), Tool::tokengauge(tokengauge)]
        .iter()
        .map(|t| t.report(releases(client, api, &t.repo)))
        .collect();
    json!({ "tools": tools, "checkedAt": crate::fsx::now_ms() })
}

/// `tokengauge --update`, which refreshes its own frontends and daemon.
pub fn update_tokengauge(program: Option<&Path>) -> Result<Value> {
    let tool = Tool::tokengauge(program);
    if let Some(held) = tool.held {
        bail!("{held}");
    }
    let program = program.context("TokenGauge is not installed")?;
    let ran = child::run(
        "TokenGauge",
        program,
        &["--update"],
        Duration::from_secs(300),
        64 << 10,
    )?;
    if !ran.ok {
        bail!("TokenGauge did not update: {}", ran.said());
    }
    let out = String::from_utf8_lossy(&ran.out);
    let said = out
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("");
    Ok(json!({ "message": said.trim(), "restarting": false }))
}

/// Installs the latest remuda over this binary, as `remuda --update` does.
/// Under the service, the restart that loads it waits for this answer to
/// leave and keeps the page's token.
pub fn update_remuda(runtime: &Path, port: u16) -> Result<Value> {
    if let Some(held) = Tool::remuda().held {
        bail!("{held}");
    }
    let exe = std::env::current_exe().context("cannot find this binary")?;
    selvedge::update::silence_progress();
    let applied = selvedge::update::apply(&REMUDA, &selvedge::state::update_cache_file(&REMUDA))?;
    if applied.version == REMUDA.version {
        return Ok(json!({
            "message": format!("remuda {} is the latest release", applied.version),
            "restarting": false,
        }));
    }
    let restarting = crate::dirs::is_service(false) && served::hand_off(runtime, port).is_ok();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(1));
        let _ = std::process::Command::new(exe)
            .arg("sync-units")
            .stdin(std::process::Stdio::null())
            .status();
    });
    let message = if restarting {
        format!(
            "remuda {} is installed; the page restarts on it",
            applied.version
        )
    } else {
        format!(
            "remuda {} is installed; restart `remuda serve` to run it",
            applied.version
        )
    };
    Ok(json!({ "message": message, "restarting": restarting }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listed(versions: &[(&str, bool, bool)]) -> String {
        let all: Vec<Value> = versions
            .iter()
            .map(|(tag, draft, prerelease)| {
                json!({
                    "tag_name": tag,
                    "published_at": "2026-11-02T11:13:16Z",
                    "html_url": format!("https://github.com/x/y/releases/tag/{tag}"),
                    "body": format!("### Added\n\n- {tag}"),
                    "draft": draft,
                    "prerelease": prerelease,
                })
            })
            .collect();
        json!(all).to_string()
    }

    fn tool(current: Option<&str>) -> Tool {
        Tool {
            id: "remuda",
            name: "remuda",
            repo: "x/y".into(),
            current: current.map(str::to_owned),
            held: None,
        }
    }

    fn release(version: &str) -> Release {
        Release {
            version: version.into(),
            published: None,
            url: String::new(),
            notes: String::new(),
        }
    }

    #[test]
    fn the_list_skips_drafts_and_prereleases_and_drops_the_v() {
        let mut server = mockito::Server::new();
        server
            .mock("GET", "/repos/x/y/releases?per_page=30")
            .match_header("accept", "application/vnd.github+json")
            .with_body(listed(&[
                ("v0.31.0", true, false),
                ("v0.31.0-rc1", false, true),
                ("v0.30.0", false, false),
            ]))
            .create();
        let all = releases(&crate::oauth::client().unwrap(), &server.url(), "x/y").unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].version, "0.30.0");
        assert_eq!(all[0].notes, "### Added\n\n- v0.30.0");
        assert_eq!(all[0].url, "https://github.com/x/y/releases/tag/v0.30.0");
    }

    #[test]
    fn a_github_that_refuses_is_an_error() {
        let mut server = mockito::Server::new();
        server
            .mock("GET", mockito::Matcher::Any)
            .with_status(403)
            .create();
        let err = releases(&crate::oauth::client().unwrap(), &server.url(), "x/y").unwrap_err();
        assert!(format!("{err:#}").contains("403"));
    }

    #[test]
    fn an_older_install_gets_the_notes_of_every_newer_release() {
        let all = vec![release("0.31.0"), release("0.30.1"), release("0.30.0")];
        let r = tool(Some("0.30.0")).report(Ok(all));
        assert_eq!(r["latest"], "0.31.0");
        assert_eq!(r["available"], true);
        let shown: Vec<&str> = r["releases"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["version"].as_str().unwrap())
            .collect();
        assert_eq!(shown, ["0.31.0", "0.30.1"]);
    }

    #[test]
    fn an_install_up_to_date_or_missing_gets_the_latest_notes() {
        let all = || vec![release("0.31.0"), release("0.30.0")];
        for current in [Some("0.31.0"), Some("0.32.0"), None] {
            let r = tool(current).report(Ok(all()));
            assert_eq!(r["available"], false, "{current:?}");
            assert_eq!(r["releases"].as_array().unwrap().len(), 1);
            assert_eq!(r["releases"][0]["version"], "0.31.0");
        }
    }

    #[test]
    fn github_out_of_reach_still_reports_what_is_installed() {
        let r = tool(Some("0.30.0")).report(Err(anyhow::anyhow!("could not reach GitHub")));
        assert_eq!(r["current"], "0.30.0");
        assert_eq!(r["latest"], Value::Null);
        assert_eq!(r["error"], "could not reach GitHub");
        assert_eq!(r["url"], "https://github.com/x/y/releases");
    }

    fn script(dir: &Path, body: &str) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join("tokengauge");
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[test]
    fn tokengauge_is_read_from_its_version_flag() {
        let tmp = tempfile::tempdir().unwrap();
        let program = script(
            tmp.path(),
            r#"[ "$1" = --version ] && echo "tokengauge 0.53.0""#,
        );
        let t = Tool::tokengauge(Some(&program));
        assert_eq!(t.current.as_deref(), Some("0.53.0"));
        assert_eq!(t.held, None);

        let t = Tool::tokengauge(None);
        assert_eq!(t.current, None);
        assert_eq!(t.held.as_deref(), Some("TokenGauge is not installed"));

        let mute = script(tmp.path(), "exit 2");
        assert!(Tool::tokengauge(Some(&mute)).held.is_some());
    }

    #[test]
    fn tokengauge_updates_itself_and_says_what_it_did() {
        let tmp = tempfile::tempdir().unwrap();
        let program = script(
            tmp.path(),
            r#"case "$1" in
--version) echo "tokengauge 0.52.0" ;;
--update) echo "downloading"; echo "updated tokengauge 0.52.0 -> 0.53.0" ;;
esac"#,
        );
        let r = update_tokengauge(Some(&program)).unwrap();
        assert_eq!(r["message"], "updated tokengauge 0.52.0 -> 0.53.0");

        let failing = script(
            tmp.path(),
            r#"case "$1" in
--version) echo "tokengauge 0.52.0" ;;
*) echo "an update is already running" >&2; exit 1 ;;
esac"#,
        );
        let err = update_tokengauge(Some(&failing)).unwrap_err();
        assert_eq!(
            format!("{err:#}"),
            "TokenGauge did not update: an update is already running"
        );
        assert!(update_tokengauge(None).is_err());
    }

    #[test]
    fn a_binary_under_usr_belongs_to_the_package_manager() {
        assert!(packaged(Path::new("/usr/bin/nonexistent-tokengauge")));
        assert!(!packaged(Path::new(
            "/usr/local/bin/nonexistent-tokengauge"
        )));
        assert!(!packaged(Path::new("/home/me/.local/bin/tokengauge")));
    }
}
