# Distribution

Users are not expected to have cargo.

## Release

`scripts/release.sh <x.y.z>` (on a clean `master` level with origin) moves
`[Unreleased]` into `## [x.y.z] - <date>`, bumps `Cargo.toml` and `Cargo.lock`,
runs fmt, clippy and the tests, checks `--version`, commits
`[master] chore(release): x.y.z`, tags `vx.y.z` and pushes. A failing gate undoes
the bump.

The tag starts `.github/workflows/release.yml`, in jobs:

1. **binary**: native x86_64 and aarch64 runners, tests first, then a static
   musl build (`<arch>-unknown-linux-musl`, `musl-tools`), refused if
   `readelf -d` shows any `NEEDED` or `--version` disagrees with the tag. The
   glibc build needed 2.39, which Debian 12 and Ubuntu 22.04 do not have.
2. **assets**: one archive per architecture,
   `remuda-vX.Y.Z-linux-<arch>.tar.gz`, holding the binary at its root, every
   unit in `systemd/`, `completions/` (bash, zsh and fish, written by the
   x86_64 binary since they do not depend on the architecture) and the
   licences. `scripts/package.sh` then turns each archive into a `.deb` and
   an `.rpm` (nfpm, `packaging/nfpm.yaml`).
3. **arch** (in an `archlinux` container): `scripts/arch.sh` builds
   `remuda-bin` for both architectures from the archives, and builds the
   source `remuda` PKGBUILD from the tag's GitHub tarball (a `git archive` on
   a pull request) to prove it. Only `remuda-bin`'s packages ship, with both
   rendered PKGBUILDs and `.SRCINFO`s in `remuda-vX.Y.Z-pkgbuild.tar.gz`.
4. **checksums**: signs, then writes `SHA256SUMS` over every asset (see
   [Signing](#signing)).
5. **verify**: checks the sums and signatures, installs the `.deb` on Debian
   12 and Ubuntu 22.04, the `.rpm` on Fedora and `remuda-bin` on Arch, runs
   `scripts/check-package.sh` in each (binary, units rewritten to
   `/usr/bin/remuda`, completions), removes them, and runs `install.sh`
   against the assets through `REMUDA_DOWNLOAD_BASE`. Both architectures,
   except Arch, which has no arm64 image.
6. **publish**: only from a tag or a `workflow_dispatch`. Release notes are
   the version's `CHANGELOG.md` section.

A pull request that touches the workflow, a packaging script, `packaging/`,
`systemd/` or the manifests runs jobs 1 to 5 as a rehearsal, since a tag is
never moved and the first run of a changed release should not be the one that
ships.

`project.rs` tests hold the workflow, the installer and selvedge to the same
asset name, so a rename that would break `--update` fails `cargo test`.
`tests/packaging.rs` fails when a unit or a completion script is missing from
a package, or a unit runs the binary by a path the packages do not rewrite.

## Packages

Every package is made from the release archive, so a package carries the
bytes `--update` would install. The binary goes to `/usr/bin/remuda`, the
units to `/usr/lib/systemd/user` with `ExecStart=%h/.local/bin/remuda`
rewritten to `/usr/bin/remuda`, the completions to the system directories
(zsh's to `vendor-completions` on Debian, `site-functions` elsewhere), and the
licences and docs to `/usr/share`. Each unit is listed on its own in
`nfpm.yaml`: a `tree` entry gives its mode to the directory too, and the
`.deb` then set `/usr/lib/systemd/user` to `0644`.

A package enables nothing: the user runs `remuda ls` once and enables the
timer, and writes `environment.d` themselves if their CLIs live elsewhere.
`--update` on `/usr/bin/remuda` is refused by selvedge's owner check (only
once a newer release exists), and `sync-units` never looks at
`/usr/lib/systemd/user`. Units in `~/.config/systemd/user` from an earlier
`install.sh` override the package's, so moving to a package means removing
those and `~/.local/bin/remuda`.

## Signing

`packaging/remuda.asc` is the public release key (Ed25519, sign-only, no
expiry, fingerprint `0630256DBF85D5A91C9ED7AD18B546E18A0C5512`). Its secret
half and passphrase are the `REMUDA_GPG_KEY` and `REMUDA_GPG_PASSPHRASE`
repository secrets; the maintainer's copy, with its revocation certificate,
is under `~/.config/remuda/` (`gnupg/` and `release-signing.*`), as smith's,
wyrmyon's and TailGauge's are under theirs.

With the public key committed, a release refuses to publish without the
secret, or with a secret that is not that key. The checksums job signs the
`.rpm`s (`rpmsign`), writes a detached `.sig` beside each Arch package, copies
the public key in as `remuda-release-key.asc`, then signs `SHA256SUMS` into
`SHA256SUMS.asc`. A pull request never sees the secret, so its rehearsal is
unsigned. The `.deb` is not signed: apt checks repositories, not files.

## Install

`scripts/install.sh` (curl | bash): finds the latest tag through the
`releases/latest` redirect (no API, no jq), downloads the release's
`SHA256SUMS` and the archive for `uname -m`, refuses the archive unless its
digest matches (and a release with no `SHA256SUMS`, which is every one before
0.19.0), runs the binary once to prove it works on this libc, installs it
into `~/.local/bin`, enables `remuda-refresh.timer` (every 30 minutes, runs
`remuda refresh --scheduled`), and writes completions for bash, zsh and fish when present.
Flags: `--version` (with or without its `v`), `--no-timer`, `--serve`,
`--no-completions`. `--serve` installs `remuda-serve.service`, enables it and
restarts it (see [web.md](web.md)). Without `--serve`, a service an earlier
install left running is restarted, so it does not stay on the old binary. A `project.rs` test fails when a file in `systemd/` is not
shipped or not installed. Everything runs from `main`, called on the last
line, so a download cut short runs nothing. `REMUDA_DOWNLOAD_BASE` (a URL or a
directory) replaces the GitHub release as the source of `SHA256SUMS` and the
archive; with no `--version` it takes the one `linux-<arch>` archive the sums
list. The verify job and `tests/install.rs` use it.

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

`remuda --update` / `--check-update` is selvedge `check_cached` / `apply` over the
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
`-D warnings`, tests on both architectures, and shellcheck on `install.sh`,
`release.sh`, `package.sh`, `arch.sh` and `check-package.sh`.

## Sources

- [scripts/release.sh](../../scripts/release.sh)
- [scripts/install.sh](../../scripts/install.sh)
- [.github/workflows/release.yml](../../.github/workflows/release.yml)
- [.github/workflows/ci.yml](../../.github/workflows/ci.yml)
- [scripts/package.sh](../../scripts/package.sh)
- [scripts/arch.sh](../../scripts/arch.sh)
- [scripts/check-package.sh](../../scripts/check-package.sh)
- [packaging/nfpm.yaml](../../packaging/nfpm.yaml)
- [packaging/arch/](../../packaging/arch/)
- [packaging/remuda.asc](../../packaging/remuda.asc)
- [tests/install.rs](../../tests/install.rs)
- [tests/packaging.rs](../../tests/packaging.rs)
- [systemd/remuda-refresh.timer](../../systemd/remuda-refresh.timer)
- [systemd/remuda-serve.service](../../systemd/remuda-serve.service)
- [src/project.rs](../../src/project.rs)
- [src/units.rs](../../src/units.rs)
