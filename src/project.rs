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
