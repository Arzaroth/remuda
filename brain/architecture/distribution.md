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
restarts it (see [web.md](web.md)). Without `--serve`, a service an earlier
install left running is restarted, so it does not stay on the old binary. A `project.rs` test fails when a file in `systemd/` is not
shipped or not installed.

A systemd user service does not see what a shell exports, so the installer
writes whichever of `CLAUDE_CONFIG_DIR`, `CODEX_HOME`, `GROK_HOME`,
`GROK_AUTH_PATH`, `KIMI_CODE_HOME`, `CURSOR_CONFIG_DIR` and `REMUDA_STORE` are
set
to `~/.config/environment.d/60-remuda.conf` and into the running user manager.
Without that, the timer would look at the default locations, miss the live
login, and refresh the active credential.

A CLI directory set or changed after installing is caught too
([dirs.rs](../../src/dirs.rs)): every interactive command records where it
found each CLI's login in `<store>/.dirs.json`, and the timer runs
`remuda refresh --scheduled`, which refreshes only the CLIs whose directory
matches the record. Any other CLI is skipped with a line naming what to set,
and the run exits non-zero so the journal shows it. With no record at all, the
timer refreshes nothing (the installer runs `remuda ls` once so there is one;
on a fresh install, the first `import` or `login` writes it, and `--serve`
enables the service without starting it), and a store that does not exist
stops it before anything is created.

- The record lives in its store, so each store keeps its own and a smoke test
  against a scratch `REMUDA_STORE` touches only that one. The price is that a
  timer looking at another store than the shell cannot tell; the installer's
  `environment.d` entry is what keeps them the same.
- A run with `REMUDA_SERVICE` set to anything (`remuda-serve.service` sets
  it), or whose cgroup is a `remuda-*.service` unit, is checked like
  `--scheduled` and never records. The cgroup covers units written before the
  marker that are still installed: a 0.1.0 timer running plain
  `remuda refresh` that only `remuda update`s older than 0.6.0 ever followed,
  or a serve unit `install.sh` restarted without `--serve`. `INVOCATION_ID` is
  not taken as a sign of systemd: GNOME runs its terminal as a user service,
  so every shell it opens has it.
- A relative directory variable (`CLAUDE_CONFIG_DIR`, `CODEX_HOME`,
  `REMUDA_STORE` and the others above) is made
  absolute before use, so the same string from another directory is not taken
  for the same place.
- A record that cannot be written is a warning, never a failed command.

## Update

`remuda update [--check]` is selvedge `check_cached` / `apply` over the
`REMUDA` project (no frontends, no aliases). `REMUDA_REPO` overrides the repo
for a fork.

selvedge replaces the binary only, so an update that replaced it then runs
the new binary's hidden `remuda sync-units` (the path is taken before the
swap). The binary carries the units the release ships (`units.rs`,
`include_str!` of `systemd/`, a test holding the two to the same list), and
`sync-units` rewrites the ones already in `~/.config/systemd/user` (where
`install.sh` puts them) that differ. It never adds a unit, so `--no-timer` and
an install without `--serve` stay that way, and it leaves a link alone: a
masked unit, or one pointed elsewhere. When it changed any, it runs
`systemctl --user daemon-reload`. Then it restarts `remuda-serve.service` when
`is-active` says it runs. A step that fails is a warning naming the command to
run by hand.

The hook runs from the new binary, so it applies from the update after the
release that carries it: updating from 0.5.0 only restarts
`remuda-serve.service`, as 0.5.0 itself does, and refreshes no unit.

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
- [src/units.rs](../../src/units.rs)
