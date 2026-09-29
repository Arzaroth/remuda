use std::env;
use std::ffi::OsStr;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime};

use crate::{fsx, paths, pkce};

/// Long enough for a browser to have read its redirect page.
const REDIRECT_KEPT: Duration = Duration::from_secs(600);

/// `xdg-settings` asks the desktop; a sign-in does not wait on it for longer.
const ASK_DESKTOP: Duration = Duration::from_secs(2);

/// Default browsers that can be told to open a private window: their
/// desktop entries, the executables those run, and the flag.
const PRIVATE: &[(&[&str], &[&str], &str)] = &[
    (
        &["brave-browser", "brave"],
        &["brave-browser", "brave"],
        "--incognito",
    ),
    (
        &["brave-browser-beta"],
        &["brave-browser-beta"],
        "--incognito",
    ),
    (
        &["brave-browser-nightly"],
        &["brave-browser-nightly"],
        "--incognito",
    ),
    (
        &["google-chrome"],
        &["google-chrome-stable", "google-chrome"],
        "--incognito",
    ),
    (
        &["google-chrome-beta"],
        &["google-chrome-beta"],
        "--incognito",
    ),
    (
        &["google-chrome-unstable"],
        &["google-chrome-unstable"],
        "--incognito",
    ),
    (
        &["chromium", "chromium-browser"],
        &["chromium", "chromium-browser"],
        "--incognito",
    ),
    (
        &["vivaldi-stable"],
        &["vivaldi-stable", "vivaldi"],
        "--incognito",
    ),
    (&["vivaldi-snapshot"], &["vivaldi-snapshot"], "--incognito"),
    (
        &["microsoft-edge"],
        &["microsoft-edge-stable", "microsoft-edge"],
        "--inprivate",
    ),
    (
        &["microsoft-edge-beta"],
        &["microsoft-edge-beta"],
        "--inprivate",
    ),
    (
        &["microsoft-edge-dev"],
        &["microsoft-edge-dev"],
        "--inprivate",
    ),
    (&["firefox"], &["firefox"], "--private-window"),
    (&["firefox-esr"], &["firefox-esr"], "--private-window"),
    (
        &["firefox-developer-edition"],
        &["firefox-developer-edition"],
        "--private-window",
    ),
    (&["librewolf"], &["librewolf"], "--private-window"),
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
    redirect_file_in(&paths::runtime_dir(), url, SystemTime::now())
}

/// One file per open, so two sign-ins started together each reach their own
/// URL; the ones a browser has long since read are removed.
fn redirect_file_in(dir: &Path, url: &str, now: SystemTime) -> Option<PathBuf> {
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let stale = entry
            .metadata()
            .and_then(|m| m.modified())
            .is_ok_and(|t| now.duration_since(t).is_ok_and(|age| age > REDIRECT_KEPT));
        if name.starts_with("open-") && name.ends_with(".html") && stale {
            let _ = std::fs::remove_file(entry.path());
        }
    }
    let page = dir.join(format!("open-{}.html", pkce::random().ok()?));
    fsx::write_private(&page, &redirect_page(url)).ok()?;
    Some(page)
}

fn spawn(program: &Path, args: &[&OsStr]) -> bool {
    let child = Command::new(program)
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    let Ok(mut child) = child else {
        return false;
    };
    std::thread::spawn(move || child.wait());
    true
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
    let mut ask = Command::new("xdg-settings");
    ask.args(["get", "default-web-browser"]);
    let id = output_within(ask, ASK_DESKTOP)?.trim().to_owned();
    (!id.is_empty()).then_some(id)
}

/// The standard output of a command that succeeded within `limit`.
fn output_within(mut cmd: Command, limit: Duration) -> Option<String> {
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = std::time::Instant::now() + limit;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    };
    let mut out = String::new();
    std::io::Read::read_to_string(&mut child.stdout.take()?, &mut out).ok()?;
    status.success().then_some(out)
}

fn launcher(desktop: &str, path: Option<&OsStr>) -> Option<(PathBuf, &'static str)> {
    let id = desktop.strip_suffix(".desktop").unwrap_or(desktop);
    let (_, programs, flag) = PRIVATE.iter().find(|(ids, ..)| ids.contains(&id))?;
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
        bin(tmp.path(), "google-chrome-stable", 0o755);
        let path = env::join_paths(["/nonexistent".into(), tmp.path().to_owned()]).unwrap();

        assert_eq!(
            launcher("brave-browser.desktop", Some(&path)),
            Some((tmp.path().join("brave-browser"), "--incognito"))
        );
        assert_eq!(
            launcher("firefox.desktop", Some(&path)),
            Some((tmp.path().join("firefox"), "--private-window"))
        );
        assert_eq!(launcher("google-chrome-beta.desktop", Some(&path)), None);
        assert_eq!(launcher("firefox-esr.desktop", Some(&path)), None);
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
        assert_eq!(launcher("firefox_firefox.desktop", Some(path)), None);
        assert_eq!(launcher("brave-browser.desktop", None), None);
    }

    #[test]
    fn each_open_gets_its_own_redirect_file_and_old_ones_go() {
        let tmp = tempfile::tempdir().unwrap();
        let now = SystemTime::now();
        let old = tmp.path().join("open-old.html");
        std::fs::write(&old, "").unwrap();
        std::fs::File::options()
            .write(true)
            .open(&old)
            .unwrap()
            .set_modified(now - REDIRECT_KEPT - Duration::from_secs(1))
            .unwrap();
        let other = tmp.path().join("keep.html");
        std::fs::write(&other, "").unwrap();

        let a = redirect_file_in(tmp.path(), "https://a.example/", now).unwrap();
        let b = redirect_file_in(tmp.path(), "https://b.example/", now).unwrap();

        assert_ne!(a, b);
        assert!(
            std::fs::read_to_string(&a)
                .unwrap()
                .contains("https://a.example/")
        );
        assert!(
            std::fs::read_to_string(&b)
                .unwrap()
                .contains("https://b.example/")
        );
        assert!(!old.exists());
        assert!(other.exists());
    }

    #[test]
    fn asking_the_desktop_gives_up_after_its_limit() {
        let mut quick = Command::new("sh");
        quick.args(["-c", "echo brave-browser.desktop"]);
        assert_eq!(
            output_within(quick, Duration::from_secs(5)).as_deref(),
            Some("brave-browser.desktop\n")
        );

        let mut failing = Command::new("sh");
        failing.args(["-c", "echo x; exit 1"]);
        assert_eq!(output_within(failing, Duration::from_secs(5)), None);

        let mut stuck = Command::new("sleep");
        stuck.arg("30");
        let started = std::time::Instant::now();
        assert_eq!(output_within(stuck, Duration::from_millis(100)), None);
        assert!(started.elapsed() < Duration::from_secs(5));
    }
}
