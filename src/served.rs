use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::fsx;

/// What a running `serve` tells other programs: no token, so anything of the
/// user's may read it.
#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct Status {
    pub pid: u32,
    pub started: u64,
    pub port: u16,
    pub version: String,
}

/// When the process started, in clock ticks since boot (field 22 of
/// `/proc/<pid>/stat`): with the pid, it names one process even after the pid
/// is reused.
fn started(pid: &str) -> Option<u64> {
    let stat = std::fs::read_to_string(Path::new("/proc").join(pid).join("stat")).ok()?;
    stat.rsplit_once(')')?
        .1
        .split_whitespace()
        .nth(19)?
        .parse()
        .ok()
}

fn status_path(dir: &Path) -> PathBuf {
    dir.join("serve.json")
}

fn url_path(dir: &Path) -> PathBuf {
    dir.join("serve.url")
}

/// The URL first, so a reader that finds the status finds its URL too.
pub fn announce(dir: &Path, port: u16, url: &str) -> Result<()> {
    fsx::write_private(&url_path(dir), &format!("{url}\n"))?;
    let status = Status {
        pid: std::process::id(),
        started: started("self").context("cannot read this process's start time")?,
        port,
        version: env!("CARGO_PKG_VERSION").to_owned(),
    };
    fsx::write_json(&status_path(dir), &serde_json::to_value(&status)?)
}

/// A `serve` that stopped leaves its files behind, and its pid can be reused,
/// so only the same process, its port still answering, counts.
pub fn running(dir: &Path) -> Option<String> {
    let status: Status = fsx::read_record(&status_path(dir))?;
    if started(&status.pid.to_string()) != Some(status.started) {
        return None;
    }
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, status.port));
    TcpStream::connect_timeout(&addr, Duration::from_secs(1)).ok()?;
    let url = std::fs::read_to_string(url_path(dir)).ok()?;
    let url = url.trim();
    url.starts_with(&format!("http://127.0.0.1:{}/#", status.port))
        .then(|| url.to_owned())
}

/// The running page's URL, after starting the service when nothing serves
/// and `start` can.
pub fn find(dir: &Path, start: impl FnOnce() -> bool, wait: Duration) -> Result<String> {
    if let Some(url) = running(dir) {
        return Ok(url);
    }
    if start() {
        let deadline = std::time::Instant::now() + wait;
        while std::time::Instant::now() < deadline {
            if let Some(url) = running(dir) {
                return Ok(url);
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        return running(dir).context("remuda-serve.service started but is not serving; see `journalctl --user -u remuda-serve`");
    }
    anyhow::bail!(
        "remuda is not serving; run `remuda serve`, or install remuda-serve.service with `install.sh --serve`"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    fn listening() -> (TcpListener, u16) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        (listener, port)
    }

    #[test]
    fn the_status_names_the_server_and_only_the_url_file_holds_the_token() {
        let tmp = tempfile::tempdir().unwrap();
        let (_listener, port) = listening();
        let url = format!("http://127.0.0.1:{port}/#secret");

        announce(tmp.path(), port, &url).unwrap();

        let status = std::fs::read_to_string(status_path(tmp.path())).unwrap();
        assert!(!status.contains("secret"));
        let status: serde_json::Value = serde_json::from_str(&status).unwrap();
        assert_eq!(
            status,
            serde_json::json!({
                "pid": std::process::id(),
                "started": started("self").unwrap(),
                "port": port,
                "version": env!("CARGO_PKG_VERSION"),
            })
        );
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(url_path(tmp.path()))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
        assert_eq!(running(tmp.path()), Some(url));
    }

    #[test]
    fn a_server_that_stopped_is_not_running() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(running(tmp.path()), None);

        let (listener, port) = listening();
        announce(tmp.path(), port, &format!("http://127.0.0.1:{port}/#t")).unwrap();
        drop(listener);
        assert_eq!(running(tmp.path()), None);

        let (_listener, port) = listening();
        announce(tmp.path(), port, &format!("http://127.0.0.1:{port}/#t")).unwrap();
        let me = started("self").unwrap();
        let record = |pid, started| {
            let status = Status {
                pid,
                started,
                port,
                version: String::new(),
            };
            fsx::write_json(
                &status_path(tmp.path()),
                &serde_json::to_value(status).unwrap(),
            )
            .unwrap();
        };
        record(u32::MAX, me);
        assert_eq!(running(tmp.path()), None);
        record(std::process::id(), me + 1);
        assert_eq!(running(tmp.path()), None);
        record(std::process::id(), me);
        assert!(running(tmp.path()).is_some());
    }

    #[test]
    fn a_start_time_is_the_twenty_second_stat_field() {
        let stat = std::fs::read_to_string("/proc/self/stat").unwrap();
        let fields: Vec<&str> = stat
            .rsplit_once(')')
            .unwrap()
            .1
            .split_whitespace()
            .collect();
        assert_eq!(started("self"), fields[19].parse().ok());
        assert_eq!(started("no-such-pid"), None);
    }

    #[test]
    fn a_url_for_another_port_is_not_taken() {
        let tmp = tempfile::tempdir().unwrap();
        let (_listener, port) = listening();
        announce(tmp.path(), port, "http://127.0.0.1:1/#t").unwrap();
        assert_eq!(running(tmp.path()), None);
    }

    #[test]
    fn nothing_serving_starts_the_service_and_waits_for_it() {
        let tmp = tempfile::tempdir().unwrap();
        let err = find(tmp.path(), || false, Duration::ZERO).unwrap_err();
        assert!(err.to_string().contains("not serving"), "{err}");

        let (_listener, port) = listening();
        let url = format!("http://127.0.0.1:{port}/#t");
        let dir = tmp.path().to_owned();
        let found = find(
            tmp.path(),
            || {
                let url = url.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_millis(200));
                    announce(&dir, port, &url).unwrap();
                });
                true
            },
            Duration::from_secs(5),
        )
        .unwrap();
        assert_eq!(found, url);
    }

    #[test]
    fn a_service_that_never_serves_is_reported() {
        let tmp = tempfile::tempdir().unwrap();
        let err = find(tmp.path(), || true, Duration::from_millis(200)).unwrap_err();
        assert!(err.to_string().contains("journalctl"), "{err}");
    }
}
