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

#[derive(Debug, Clone, Copy, PartialEq)]
enum Family {
    Chromium,
    Edge,
    Firefox,
}

impl Family {
    fn private_flag(self) -> &'static str {
        match self {
            Family::Chromium => "--incognito",
            Family::Edge => "--inprivate",
            Family::Firefox => "--private-window",
        }
    }
}

/// A browser remuda knows how to ask for a private window: its desktop
/// entries and the executables those run. Each release channel runs its own
/// executable and never the stable one.
#[derive(Debug, PartialEq)]
pub struct Browser {
    pub id: &'static str,
    pub name: &'static str,
    desktop: &'static [&'static str],
    programs: &'static [&'static str],
    family: Family,
}

const fn known(
    id: &'static str,
    name: &'static str,
    desktop: &'static [&'static str],
    programs: &'static [&'static str],
    family: Family,
) -> Browser {
    Browser {
        id,
        name,
        desktop,
        programs,
        family,
    }
}

pub const BROWSERS: &[Browser] = &[
    known(
        "brave",
        "Brave",
        &["brave-browser", "brave"],
        &["brave-browser", "brave"],
        Family::Chromium,
    ),
    known(
        "brave-beta",
        "Brave Beta",
        &["brave-browser-beta"],
        &["brave-browser-beta"],
        Family::Chromium,
    ),
    known(
        "brave-nightly",
        "Brave Nightly",
        &["brave-browser-nightly"],
        &["brave-browser-nightly"],
        Family::Chromium,
    ),
    known(
        "chrome",
        "Google Chrome",
        &["google-chrome"],
        &["google-chrome-stable", "google-chrome"],
        Family::Chromium,
    ),
    known(
        "chrome-beta",
        "Google Chrome Beta",
        &["google-chrome-beta"],
        &["google-chrome-beta"],
        Family::Chromium,
    ),
    known(
        "chrome-unstable",
        "Google Chrome Dev",
        &["google-chrome-unstable"],
        &["google-chrome-unstable"],
        Family::Chromium,
    ),
    known(
        "chromium",
        "Chromium",
        &["chromium", "chromium-browser"],
        &["chromium", "chromium-browser"],
        Family::Chromium,
    ),
    known(
        "vivaldi",
        "Vivaldi",
        &["vivaldi-stable"],
        &["vivaldi-stable", "vivaldi"],
        Family::Chromium,
    ),
    known(
        "vivaldi-snapshot",
        "Vivaldi Snapshot",
        &["vivaldi-snapshot"],
        &["vivaldi-snapshot"],
        Family::Chromium,
    ),
    known(
        "edge",
        "Microsoft Edge",
        &["microsoft-edge"],
        &["microsoft-edge-stable", "microsoft-edge"],
        Family::Edge,
    ),
    known(
        "edge-beta",
        "Microsoft Edge Beta",
        &["microsoft-edge-beta"],
        &["microsoft-edge-beta"],
        Family::Edge,
    ),
    known(
        "edge-dev",
        "Microsoft Edge Dev",
        &["microsoft-edge-dev"],
        &["microsoft-edge-dev"],
        Family::Edge,
    ),
    known(
        "firefox",
        "Firefox",
        &["firefox"],
        &["firefox"],
        Family::Firefox,
    ),
    known(
        "firefox-esr",
        "Firefox ESR",
        &["firefox-esr"],
        &["firefox-esr"],
        Family::Firefox,
    ),
    known(
        "firefox-developer-edition",
        "Firefox Developer Edition",
        &["firefox-developer-edition"],
        &["firefox-developer-edition"],
        Family::Firefox,
    ),
    known(
        "librewolf",
        "LibreWolf",
        &["librewolf"],
        &["librewolf"],
        Family::Firefox,
    ),
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

/// The desktop's own opener, which hands a file to the default browser.
const OPENER: &str = if cfg!(target_os = "macos") {
    "/usr/bin/open"
} else {
    "xdg-open"
};

pub fn open(url: &str) {
    if let Some(page) = redirect_file(url) {
        spawn(Path::new(OPENER), &[page.as_os_str()]);
    }
}

/// Where a sign-in opens: a private window (the default) leaves the
/// browser's own sessions alone, a profile kept for the login stays signed in
/// for the next time, and a normal window uses the browser as it is.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Window {
    #[default]
    Private,
    Profile,
    Normal,
}

impl Window {
    pub fn is_private(&self) -> bool {
        *self == Window::Private
    }
}

impl std::str::FromStr for Window {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "private" => Ok(Window::Private),
            "profile" => Ok(Window::Profile),
            "normal" => Ok(Window::Normal),
            _ => Err(format!("{s} is not private, profile or normal")),
        }
    }
}

/// The browser a sign-in opens in, and how; no browser is the desktop's
/// own opener, which cannot be asked for a window.
#[derive(Debug, PartialEq)]
pub struct Choice {
    browser: Option<(&'static Browser, PathBuf)>,
    window: Window,
}

/// `named` when it is installed, else the desktop's default browser when
/// remuda knows it.
pub fn choose(named: Option<&str>, window: Window) -> Choice {
    let path = env::var_os("PATH");
    let browser = named
        .and_then(|id| installed_as(id, path.as_deref()))
        .or_else(|| default_browser().and_then(|desktop| launcher(&desktop, path.as_deref())));
    Choice { browser, window }
}

fn installed_as(id: &str, path: Option<&OsStr>) -> Option<(&'static Browser, PathBuf)> {
    let browser = BROWSERS.iter().find(|b| b.id == id)?;
    Some((browser, program(browser, path)?))
}

/// The browsers on `PATH`, for a choice to offer.
pub fn installed() -> Vec<&'static Browser> {
    let path = env::var_os("PATH");
    BROWSERS
        .iter()
        .filter(|b| program(b, path.as_deref()).is_some())
        .collect()
}

/// Whether the session evidently has no browser of its own, so a browser on
/// another machine would have to sign in: an SSH login, or on Linux no
/// display. `set` says whether an environment variable is set and not empty.
pub fn remote(set: &dyn Fn(&str) -> bool) -> bool {
    set("SSH_CONNECTION")
        || set("SSH_TTY")
        || (cfg!(target_os = "linux") && !set("DISPLAY") && !set("WAYLAND_DISPLAY"))
}

/// Whether a window can be shown here: on Linux `DISPLAY` or
/// `WAYLAND_DISPLAY` set, on macOS anything but an SSH login.
pub fn displayed() -> bool {
    shows(&|v| env::var_os(v).is_some_and(|v| !v.is_empty()))
}

fn shows(set: &dyn Fn(&str) -> bool) -> bool {
    match cfg!(target_os = "macos") {
        true => !set("SSH_CONNECTION") && !set("SSH_TTY"),
        false => set("DISPLAY") || set("WAYLAND_DISPLAY"),
    }
}

/// A login's own browser profile, under the store, so its sessions are kept
/// with it and never in a profile the user browses with.
pub fn profile_dir(store: &Path, provider: &str, name: &str) -> PathBuf {
    store.join(".browser").join(provider).join(name)
}

impl Choice {
    pub fn describe(&self) -> String {
        match &self.browser {
            None => "the default browser".to_owned(),
            Some((b, _)) => match self.window {
                Window::Private => format!("a private window of {}", b.name),
                Window::Profile => format!("{}, in this login's own profile", b.name),
                Window::Normal => b.name.to_owned(),
            },
        }
    }

    /// False when there is no display or nothing could be started.
    pub fn open(&self, url: &str, profile: &Path) -> bool {
        if !displayed() {
            return false;
        }
        let Some(page) = redirect_file(url) else {
            return false;
        };
        match &self.browser {
            None => spawn(Path::new(OPENER), &[page.as_os_str()]),
            Some((b, program)) => {
                if self.window == Window::Profile && fsx::create_private_dir(profile).is_err() {
                    return false;
                }
                let args = arguments(b, self.window, profile, &page);
                let args: Vec<&OsStr> = args.iter().map(|a| a.as_os_str()).collect();
                spawn(program, &args)
            }
        }
    }
}

fn arguments(b: &Browser, window: Window, profile: &Path, page: &Path) -> Vec<std::ffi::OsString> {
    let mut args: Vec<std::ffi::OsString> = match (window, b.family) {
        (Window::Private, family) => vec![family.private_flag().into()],
        (Window::Profile, Family::Firefox) => {
            vec!["--new-instance".into(), "--profile".into(), profile.into()]
        }
        (Window::Profile, Family::Chromium | Family::Edge) => {
            let mut dir = std::ffi::OsString::from("--user-data-dir=");
            dir.push(profile);
            vec![
                dir,
                "--no-first-run".into(),
                "--no-default-browser-check".into(),
            ]
        }
        (Window::Normal, _) => Vec::new(),
    };
    args.push(page.into());
    args
}

fn default_browser() -> Option<String> {
    if cfg!(target_os = "macos") {
        return None;
    }
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

fn launcher(desktop: &str, path: Option<&OsStr>) -> Option<(&'static Browser, PathBuf)> {
    let id = desktop.strip_suffix(".desktop").unwrap_or(desktop);
    let browser = BROWSERS.iter().find(|b| b.desktop.contains(&id))?;
    Some((browser, program(browser, path)?))
}

fn program(browser: &Browser, path: Option<&OsStr>) -> Option<PathBuf> {
    let dirs: Vec<PathBuf> = env::split_paths(path?).collect();
    browser
        .programs
        .iter()
        .flat_map(|p| dirs.iter().map(move |d| d.join(p)))
        .find(|p| {
            p.metadata()
                .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_session_over_ssh_or_with_no_display_is_remote() {
        let only = |vars: &'static [&'static str]| move |var: &str| vars.contains(&var);
        assert!(remote(&only(&["SSH_CONNECTION", "DISPLAY"])));
        assert!(remote(&only(&["SSH_TTY", "WAYLAND_DISPLAY"])));
        assert!(!remote(&only(&["DISPLAY"])));
        assert!(!remote(&only(&["WAYLAND_DISPLAY"])));
        assert_eq!(remote(&only(&[])), cfg!(target_os = "linux"));
    }

    #[test]
    fn a_window_shows_on_linux_with_a_display_and_on_a_mac_but_over_ssh() {
        let only = |vars: &'static [&'static str]| move |var: &str| vars.contains(&var);
        let mac = cfg!(target_os = "macos");
        assert_eq!(shows(&only(&[])), mac);
        assert!(shows(&only(&["DISPLAY"])));
        assert!(shows(&only(&["WAYLAND_DISPLAY"])));
        assert!(!shows(&only(&["SSH_TTY"])));
        assert_eq!(shows(&only(&["SSH_CONNECTION", "DISPLAY"])), !mac);
    }

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

        let flag = |desktop: &str| {
            launcher(desktop, Some(&path)).map(|(b, program)| (program, b.family.private_flag()))
        };
        assert_eq!(
            flag("brave-browser.desktop"),
            Some((tmp.path().join("brave-browser"), "--incognito"))
        );
        assert_eq!(
            flag("firefox.desktop"),
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
    fn each_family_is_asked_for_its_window_its_own_way() {
        let by = |id: &str| BROWSERS.iter().find(|b| b.id == id).unwrap();
        let profile = Path::new("/s/.browser/claude/work");
        let page = Path::new("/run/open-x.html");
        let args = |id: &str, window| -> Vec<String> {
            arguments(by(id), window, profile, page)
                .iter()
                .map(|a| a.to_string_lossy().into_owned())
                .collect()
        };

        assert_eq!(
            args("brave", Window::Private),
            ["--incognito", "/run/open-x.html"]
        );
        assert_eq!(
            args("edge", Window::Private),
            ["--inprivate", "/run/open-x.html"]
        );
        assert_eq!(
            args("librewolf", Window::Private),
            ["--private-window", "/run/open-x.html"]
        );
        assert_eq!(
            args("chromium", Window::Profile),
            [
                "--user-data-dir=/s/.browser/claude/work",
                "--no-first-run",
                "--no-default-browser-check",
                "/run/open-x.html"
            ]
        );
        assert_eq!(
            args("firefox", Window::Profile),
            [
                "--new-instance",
                "--profile",
                "/s/.browser/claude/work",
                "/run/open-x.html"
            ]
        );
        assert_eq!(args("vivaldi", Window::Normal), ["/run/open-x.html"]);
    }

    #[test]
    fn a_named_browser_is_found_by_its_id_and_said_with_its_window() {
        let tmp = tempfile::tempdir().unwrap();
        bin(tmp.path(), "vivaldi", 0o755);
        let path = tmp.path().as_os_str();

        let (b, program) = installed_as("vivaldi", Some(path)).unwrap();
        assert_eq!((b.name, program), ("Vivaldi", tmp.path().join("vivaldi")));
        assert_eq!(installed_as("chromium", Some(path)), None);
        assert_eq!(installed_as("lynx", Some(path)), None);

        let choice = |window| Choice {
            browser: installed_as("vivaldi", Some(path)),
            window,
        };
        assert_eq!(
            choice(Window::Private).describe(),
            "a private window of Vivaldi"
        );
        assert_eq!(
            choice(Window::Profile).describe(),
            "Vivaldi, in this login's own profile"
        );
        assert_eq!(choice(Window::Normal).describe(), "Vivaldi");
        let desktop = Choice {
            browser: None,
            window: Window::Profile,
        };
        assert_eq!(desktop.describe(), "the default browser");
        assert_eq!(
            profile_dir(Path::new("/s"), "codex", "work"),
            Path::new("/s/.browser/codex/work")
        );
        assert_eq!("profile".parse::<Window>(), Ok(Window::Profile));
        assert!("incognito".parse::<Window>().is_err());
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
