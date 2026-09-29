use std::env;
use std::ffi::OsStr;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::{fsx, paths};

/// Default browsers that can be told to open a private window: the
/// desktop-entry name, the executables it ships as, and the flag.
const PRIVATE: &[(&str, &[&str], &str)] = &[
    ("brave", &["brave-browser", "brave"], "--incognito"),
    (
        "google-chrome",
        &["google-chrome-stable", "google-chrome"],
        "--incognito",
    ),
    ("chromium", &["chromium", "chromium-browser"], "--incognito"),
    ("vivaldi", &["vivaldi-stable", "vivaldi"], "--incognito"),
    (
        "microsoft-edge",
        &["microsoft-edge-stable", "microsoft-edge"],
        "--inprivate",
    ),
    ("firefox", &["firefox", "firefox-esr"], "--private-window"),
    ("librewolf", &["librewolf"], "--private-window"),
];

/// A page that sends the browser on to `url`. The browser is handed this
/// file rather than the URL, because a command line is readable by every
/// local user and these URLs carry the page's token or a sign-in's state.
fn redirect_page(url: &str) -> String {
    let attr = url
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;");
    let js = serde_json::to_string(url)
        .unwrap_or_default()
        .replace('<', "\\u003c");
    format!(
        "<!doctype html><meta charset=utf-8><meta name=referrer content=no-referrer>\
         <meta http-equiv=refresh content=\"0;url={attr}\">\
         <script>location.replace({js})</script><a href=\"{attr}\">Continue</a>\n"
    )
}

fn redirect_file(url: &str) -> Option<PathBuf> {
    let page = paths::runtime_dir().join("open.html");
    fsx::write_private(&page, &redirect_page(url)).ok()?;
    Some(page)
}

fn spawn(program: &Path, args: &[&OsStr]) -> bool {
    Command::new(program)
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .is_ok()
}

pub fn open(url: &str) {
    if let Some(page) = redirect_file(url) {
        spawn(Path::new("xdg-open"), &[page.as_os_str()]);
    }
}

/// Opens `url` in a private window of the default browser, so signing in
/// there leaves the browser's own sessions alone. False when there is no
/// display or the default browser is not one it knows how to ask.
pub fn open_private(url: &str) -> bool {
    let shown = ["DISPLAY", "WAYLAND_DISPLAY"]
        .iter()
        .any(|v| env::var_os(v).is_some_and(|v| !v.is_empty()));
    if !shown {
        return false;
    }
    let Some((program, flag)) =
        default_browser().and_then(|desktop| launcher(&desktop, env::var_os("PATH").as_deref()))
    else {
        return false;
    };
    redirect_file(url).is_some_and(|page| spawn(&program, &[OsStr::new(flag), page.as_os_str()]))
}

fn default_browser() -> Option<String> {
    let out = Command::new("xdg-settings")
        .args(["get", "default-web-browser"])
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let id = String::from_utf8(out.stdout).ok()?.trim().to_owned();
    (out.status.success() && !id.is_empty()).then_some(id)
}

fn launcher(desktop: &str, path: Option<&OsStr>) -> Option<(PathBuf, &'static str)> {
    let id = desktop.strip_suffix(".desktop").unwrap_or(desktop);
    let (_, programs, flag) = PRIVATE.iter().find(|(name, ..)| {
        id.strip_prefix(name)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('-'))
    })?;
    let dirs: Vec<PathBuf> = env::split_paths(path?).collect();
    programs
        .iter()
        .flat_map(|p| dirs.iter().map(move |d| d.join(p)))
        .find(|p| {
            p.metadata()
                .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
        .map(|p| (p, *flag))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_redirect_page_quotes_the_url_for_both_of_its_uses() {
        let page = redirect_page("http://127.0.0.1:1/#a&b\"<c");
        assert!(page.contains("url=http://127.0.0.1:1/#a&amp;b&quot;&lt;c\""));
        assert!(page.contains(r#"location.replace("http://127.0.0.1:1/#a&b\"\u003cc")"#));
        assert!(!page.contains("\"<c"));
    }

    fn bin(dir: &Path, name: &str, mode: u32) {
        let p = dir.join(name);
        std::fs::write(&p, "").unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(mode)).unwrap();
    }

    #[test]
    fn a_known_default_browser_is_launched_with_its_private_flag() {
        let tmp = tempfile::tempdir().unwrap();
        bin(tmp.path(), "brave-browser", 0o755);
        bin(tmp.path(), "firefox", 0o755);
        let path = env::join_paths(["/nonexistent".into(), tmp.path().to_owned()]).unwrap();

        assert_eq!(
            launcher("brave-browser.desktop", Some(&path)),
            Some((tmp.path().join("brave-browser"), "--incognito"))
        );
        assert_eq!(
            launcher("firefox.desktop", Some(&path)),
            Some((tmp.path().join("firefox"), "--private-window"))
        );
    }

    #[test]
    fn an_unknown_or_missing_browser_has_no_launcher() {
        let tmp = tempfile::tempdir().unwrap();
        bin(tmp.path(), "chromium", 0o644);
        bin(tmp.path(), "bravery", 0o755);
        let path = tmp.path().as_os_str();

        assert_eq!(launcher("chromium.desktop", Some(path)), None);
        assert_eq!(launcher("google-chrome.desktop", Some(path)), None);
        assert_eq!(launcher("bravery.desktop", Some(path)), None);
        assert_eq!(launcher("com.brave.Browser.desktop", Some(path)), None);
        assert_eq!(launcher("brave-browser.desktop", None), None);
    }
}
