//! The packages are built from files nothing else reads, so nothing else
//! notices when a unit or a completion script is left out of one.

fn read(rel: &str) -> String {
    std::fs::read_to_string(format!("{}/{rel}", env!("CARGO_MANIFEST_DIR")))
        .unwrap_or_else(|e| panic!("{rel}: {e}"))
}

fn units() -> Vec<(String, String)> {
    let mut units: Vec<_> = std::fs::read_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/systemd"))
        .unwrap()
        .map(|unit| {
            let unit = unit.unwrap();
            (
                unit.file_name().into_string().unwrap(),
                std::fs::read_to_string(unit.path()).unwrap(),
            )
        })
        .collect();
    units.sort();
    units
}

#[test]
fn every_unit_is_in_the_deb_and_the_rpm_and_checked_there() {
    let nfpm = read("packaging/nfpm.yaml");
    let check = read("scripts/check-package.sh");
    for (name, _) in units() {
        assert!(
            nfpm.contains(&format!(
                "src: ./systemd/{name}\n    dst: /usr/lib/systemd/user/{name}\n"
            )),
            "nfpm.yaml does not ship {name}"
        );
        assert!(
            check.contains(&name),
            "check-package.sh never looks for {name}"
        );
    }
}

/// The packages move the binary to /usr/bin by rewriting this one prefix; a
/// unit that runs it some other way would keep pointing at ~/.local/bin.
#[test]
fn every_unit_runs_the_binary_the_packages_rewrite() {
    for (name, text) in units() {
        for line in text.lines().filter(|l| l.starts_with("ExecStart=")) {
            assert!(
                line.starts_with("ExecStart=%h/.local/bin/remuda "),
                "{name}: {line}"
            );
        }
    }
    let rewrite = r"s|^ExecStart=%h/\.local/bin/remuda |ExecStart=/usr/bin/remuda |";
    for file in [
        "scripts/package.sh",
        "packaging/arch/remuda/PKGBUILD",
        "packaging/arch/remuda-bin/PKGBUILD",
    ] {
        assert!(
            read(file).contains(rewrite),
            "{file} does not rewrite ExecStart"
        );
    }
}

#[test]
fn the_packages_carry_the_completions_the_archive_ships() {
    let release = read(".github/workflows/release.yml");
    let nfpm = read("packaging/nfpm.yaml");
    let bin = read("packaging/arch/remuda-bin/PKGBUILD");
    let source = read("packaging/arch/remuda/PKGBUILD");
    for (shell, file) in [
        ("bash", "remuda.bash"),
        ("zsh", "_remuda"),
        ("fish", "remuda.fish"),
    ] {
        assert!(
            release.contains(&format!("remuda completions {shell} >completions/{file}")),
            "the archive has no {file}"
        );
        assert!(
            release.contains(r#"cp -r completions "$stage/""#),
            "the completions never reach the archive"
        );
        assert!(
            nfpm.contains(&format!("src: ./archive/completions/{file}")),
            "nfpm.yaml: {file}"
        );
        assert!(
            bin.contains(&format!("completions/{file}")),
            "remuda-bin: {file}"
        );
        assert!(
            source.contains(&format!("completions {shell} >target/completions/{file}")),
            "remuda: {file}"
        );
    }
}

#[test]
fn the_release_publishes_every_package_it_builds() {
    let release = read(".github/workflows/release.yml");
    for glob in [
        "dist/*.tar.gz",
        "dist/*.deb",
        "dist/*.rpm",
        "dist/*.pkg.tar.zst",
        "dist/*.sig",
        "dist/*.asc",
        "dist/SHA256SUMS",
    ] {
        assert!(release.contains(glob), "{glob} is never published");
    }
}
