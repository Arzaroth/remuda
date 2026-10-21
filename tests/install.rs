//! `scripts/install.sh` piped into bash the way `curl | bash` runs it, against
//! a release laid out in a local directory and a throwaway home.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use sha2::{Digest, Sha256};

const VERSION: &str = "v9.9.9";

struct Release {
    dir: tempfile::TempDir,
}

fn arch() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "x86_64",
        "aarch64" => "aarch64",
        other => panic!("no release for {other}"),
    }
}

fn archive_name() -> String {
    format!("remuda-{VERSION}-linux-{}.tar.gz", arch())
}

fn write_exe(path: &Path, body: &str) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(path, body).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

impl Release {
    /// A release whose archive holds a stand-in binary and the real units,
    /// listed in a SHA256SUMS that `sums` may then rewrite.
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let stage = dir.path().join("stage");
        std::fs::create_dir_all(stage.join("systemd")).unwrap();
        write_exe(
            &stage.join("remuda"),
            &format!("#!/bin/sh\necho 'remuda {}'\n", &VERSION[1..]),
        );
        for unit in std::fs::read_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/systemd")).unwrap() {
            let unit = unit.unwrap();
            std::fs::copy(unit.path(), stage.join("systemd").join(unit.file_name())).unwrap();
        }
        let dist = dir.path().join("dist");
        std::fs::create_dir_all(&dist).unwrap();
        let status = Command::new("tar")
            .arg("-czf")
            .arg(dist.join(archive_name()))
            .arg("-C")
            .arg(&stage)
            .arg(".")
            .status()
            .unwrap();
        assert!(status.success());
        let release = Release { dir };
        let digest = release.digest_of(&archive_name());
        release.sums(&format!("{digest}  {}\n", archive_name()));
        release
    }

    fn dist(&self) -> PathBuf {
        self.dir.path().join("dist")
    }

    fn digest_of(&self, file: &str) -> String {
        let bytes = std::fs::read(self.dist().join(file)).unwrap();
        Sha256::digest(&bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    fn sums(&self, text: &str) {
        std::fs::write(self.dist().join("SHA256SUMS"), text).unwrap();
    }

    /// Runs the installer on stdin with only the tools it needs on PATH, and a
    /// `systemctl` that answers nothing, so no real user session is touched.
    fn install(&self, home: &Path, args: &[&str]) -> Output {
        let stubs = self.dir.path().join("stubs");
        std::fs::create_dir_all(&stubs).unwrap();
        write_exe(&stubs.join("systemctl"), "#!/bin/sh\nexit 1\n");
        let script =
            std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/install.sh")).unwrap();
        let mut child = Command::new("bash")
            .arg("-s")
            .arg("--")
            .args(args)
            .env_clear()
            .env("HOME", home)
            .env("PATH", format!("{}:/usr/bin:/bin", stubs.display()))
            .env("REMUDA_DOWNLOAD_BASE", self.dist())
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        use std::io::Write;
        child.stdin.take().unwrap().write_all(&script).unwrap();
        child.wait_with_output().unwrap()
    }
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn a_piped_install_lands_the_release_it_checked() {
    let release = Release::new();
    let home = tempfile::tempdir().unwrap();
    let out = release.install(home.path(), &["--no-timer", "--no-completions"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let installed = home.path().join(".local/bin/remuda");
    let printed = Command::new(&installed).arg("--version").output().unwrap();
    assert_eq!(String::from_utf8_lossy(&printed.stdout), "remuda 9.9.9\n");
}

#[test]
fn an_archive_that_does_not_match_its_checksum_installs_nothing() {
    let release = Release::new();
    release.sums(&format!("{}  {}\n", "0".repeat(64), archive_name()));
    let home = tempfile::tempdir().unwrap();
    let out = release.install(home.path(), &["--no-timer", "--no-completions"]);
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("does not match its SHA256SUMS entry"),
        "{}",
        stderr(&out)
    );
    assert!(!home.path().join(".local/bin").exists());
}

#[test]
fn a_release_without_checksums_installs_nothing() {
    let release = Release::new();
    std::fs::remove_file(release.dist().join("SHA256SUMS")).unwrap();
    let home = tempfile::tempdir().unwrap();
    let out = release.install(home.path(), &["--no-timer", "--no-completions"]);
    assert!(!out.status.success());
    assert!(stderr(&out).contains("no SHA256SUMS"), "{}", stderr(&out));
    assert!(!home.path().join(".local/bin").exists());
}

#[test]
fn an_archive_missing_from_the_checksums_installs_nothing() {
    let release = Release::new();
    let home = tempfile::tempdir().unwrap();
    let out = release.install(
        home.path(),
        &["--version", "v9.9.8", "--no-timer", "--no-completions"],
    );
    assert!(!out.status.success());
    assert!(stderr(&out).contains("is not in"), "{}", stderr(&out));
    assert!(!home.path().join(".local/bin").exists());
}
