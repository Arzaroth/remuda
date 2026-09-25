use selvedge::Project;

pub const REMUDA: Project = Project {
    binaries: &["remuda"],
    repo: "Arzaroth/remuda",
    repo_env: "REMUDA_REPO",
    version: env!("CARGO_PKG_VERSION"),
    frontends: &[],
    aliases: &[],
    legacy: &[],
    msi_marker_key: None,
};

/// selvedge downloads what the release workflow publishes, and nothing checks
/// the two agree until an update 404s on a user's machine.
#[cfg(test)]
mod tests {
    use super::*;
    use selvedge::update::{ARCHIVE_SUFFIX, arch_target};

    fn release_workflow() -> String {
        std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/.github/workflows/release.yml"
        ))
        .expect("release.yml")
    }

    #[test]
    fn the_release_publishes_the_archive_this_binary_downloads() {
        let release = release_workflow();
        assert!(release.contains("for arch in x86_64 aarch64; do"));
        assert!(release.contains("\"dist/remuda-$safe-linux-$arch.tar.gz\""));
        let wanted = format!("{}{ARCHIVE_SUFFIX}", arch_target().expect("a known arch"));
        assert!(
            ["linux-x86_64.tar.gz", "linux-aarch64.tar.gz"].contains(&wanted.as_str()),
            "apply() asks for {wanted}"
        );
        assert!(
            release.contains("dist/*.tar.gz"),
            "the archives are never uploaded"
        );
    }

    #[test]
    fn the_archive_carries_the_binary_at_its_root() {
        assert!(release_workflow().contains(&format!("\"$stage/{}\"", REMUDA.primary())));
    }

    #[test]
    fn the_installer_downloads_the_archive_the_release_publishes() {
        let install =
            std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/install.sh"))
                .expect("install.sh");
        assert!(install.contains("remuda-$tag-linux-$arch.tar.gz"));
        assert!(install.contains(REMUDA.repo));
    }

    #[test]
    fn the_release_refuses_a_tag_the_binary_disagrees_with() {
        assert!(release_workflow().contains("does not match the binary's version"));
    }
}
