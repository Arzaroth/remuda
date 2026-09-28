# Distribution

Users are not expected to have cargo.

## Release

`scripts/release.sh <x.y.z>` (on a clean `master` level with origin) moves
`[Unreleased]` into `## [x.y.z] - <date>`, bumps `Cargo.toml` and `Cargo.lock`,
runs fmt, clippy and the tests, checks `--version`, commits
`[master] chore(release): x.y.z`, tags `vx.y.z` and pushes. A failing gate undoes
the bump.

The tag starts `.github/workflows/release.yml`: native x86_64 and aarch64 builds
(tests first, then a check that `--version` matches the tag), then one archive
per architecture, `remuda-vX.Y.Z-linux-<arch>.tar.gz`, holding the binary at
its root, `systemd/` and the licences. Release notes are the version's
`CHANGELOG.md` section.

`project.rs` tests hold the workflow, the installer and selvedge to the same
asset name, so a rename that would break `update` fails `cargo test`.

## Install

`scripts/install.sh` (curl | bash): finds the latest tag through the
`releases/latest` redirect (no API, no jq), downloads the archive for
`uname -m`, runs the binary once to prove it works on this libc, installs it
into `~/.local/bin`, enables `remuda-refresh.timer` (every 30 minutes, runs
`remuda refresh --scheduled`), and writes completions for bash, zsh and fish when present.
Flags: `--version` (with or without its `v`), `--no-timer`, `--no-completions`.

A systemd user service does not see what a shell exports, so the installer
writes whichever of `CLAUDE_CONFIG_DIR`, `CODEX_HOME` and `REMUDA_STORE` are set
to `~/.config/environment.d/60-remuda.conf` and into the running user manager.
Without that, the timer would look at the default locations, miss the live
login, and refresh the active credential.

A variable set or changed after installing is caught too: every interactive
command records the store and each CLI's directory in
`$XDG_STATE_HOME/remuda/dirs.json` (`dirs.rs`), and the timer runs
`remuda refresh --scheduled`, which stops if its store differs from the
recorded one and skips any CLI whose directory does, naming what to set, with a
non-zero exit the journal shows.

## Update

`remuda update [--check]` is selvedge `check_cached` / `apply` over the
`REMUDA` project (no frontends, no aliases). `REMUDA_REPO` overrides the repo
for a fork.

## CI

`.github/workflows/ci.yml` on pull requests and pushes to `master`: fmt, clippy
`-D warnings`, tests on both architectures, and shellcheck on the two scripts.

## Sources

- [scripts/release.sh](../../scripts/release.sh)
- [scripts/install.sh](../../scripts/install.sh)
- [.github/workflows/release.yml](../../.github/workflows/release.yml)
- [.github/workflows/ci.yml](../../.github/workflows/ci.yml)
- [systemd/remuda-refresh.timer](../../systemd/remuda-refresh.timer)
- [src/project.rs](../../src/project.rs)
