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
its root, every unit in `systemd/` and the licences. Release notes are the version's
`CHANGELOG.md` section.

`project.rs` tests hold the workflow, the installer and selvedge to the same
asset name, so a rename that would break `update` fails `cargo test`.

## Install

`scripts/install.sh` (curl | bash): finds the latest tag through the
`releases/latest` redirect (no API, no jq), downloads the archive for
`uname -m`, runs the binary once to prove it works on this libc, installs it
into `~/.local/bin`, enables `remuda-refresh.timer` (every 30 minutes, runs
`remuda refresh --scheduled`), and writes completions for bash, zsh and fish when present.
Flags: `--version` (with or without its `v`), `--no-timer`, `--serve`,
`--no-completions`. `--serve` installs `remuda-serve.service`, enables it and
restarts it, so a reinstall moves it onto the new binary (see
[web.md](web.md)). A `project.rs` test fails when a file in `systemd/` is not
shipped or not installed.

A systemd user service does not see what a shell exports, so the installer
writes whichever of `CLAUDE_CONFIG_DIR`, `CODEX_HOME` and `REMUDA_STORE` are set
to `~/.config/environment.d/60-remuda.conf` and into the running user manager.
Without that, the timer would look at the default locations, miss the live
login, and refresh the active credential.

A CLI directory set or changed after installing is caught too
([dirs.rs](../../src/dirs.rs)): every interactive command records where it
found each CLI's login in `<store>/.dirs.json`, and the timer runs
`remuda refresh --scheduled`, which refreshes only the CLIs whose directory
matches the record. Any other CLI is skipped with a line naming what to set,
and the run exits non-zero so the journal shows it. With no record at all, the
timer refreshes nothing (the installer runs `remuda ls` once so there is one),
and a store that does not exist stops it before anything is created.

- The record lives in its store, so each store keeps its own and a smoke test
  against a scratch `REMUDA_STORE` touches only that one. The price is that a
  timer looking at another store than the shell cannot tell; the installer's
  `environment.d` entry is what keeps them the same.
- A run under systemd (`INVOCATION_ID` is set) is checked like
  `--scheduled` and never records, which covers units installed by 0.1.0 that
  run plain `remuda refresh` and are not replaced by `remuda update`.
- A relative `CLAUDE_CONFIG_DIR`, `CODEX_HOME` or `REMUDA_STORE` is made
  absolute before use, so the same string from another directory is not taken
  for the same place.
- A record that cannot be written is a warning, never a failed command.

## Update

`remuda update [--check]` is selvedge `check_cached` / `apply` over the
`REMUDA` project (no frontends, no aliases). `REMUDA_REPO` overrides the repo
for a fork. An update that replaced the binary runs
`systemctl --user try-restart remuda-serve.service`, which restarts the
service only when it runs.

## CI

`.github/workflows/ci.yml` on pull requests and pushes to `master`: fmt, clippy
`-D warnings`, tests on both architectures, and shellcheck on the two scripts.

## Sources

- [scripts/release.sh](../../scripts/release.sh)
- [scripts/install.sh](../../scripts/install.sh)
- [.github/workflows/release.yml](../../.github/workflows/release.yml)
- [.github/workflows/ci.yml](../../.github/workflows/ci.yml)
- [systemd/remuda-refresh.timer](../../systemd/remuda-refresh.timer)
- [systemd/remuda-serve.service](../../systemd/remuda-serve.service)
- [src/project.rs](../../src/project.rs)
