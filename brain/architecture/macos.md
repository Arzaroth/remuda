# macOS

remuda runs on macOS (Apple Silicon and Intel) with the same store, config and
directory layout as on Linux: `~/.local/share/remuda/credentials`,
`~/.config/remuda/config.toml`, `~/.local/bin/remuda`. So a bundle from
`remuda export` on Linux restores on a Mac as it is, and TokenGauge's readers
find the store where they look. What differs is below.

## Where the CLIs keep their logins

- **Claude Code**: the login Keychain, through `/usr/bin/security`, with
  `.credentials.json` as Claude Code's own fallback
  ([providers.md](providers.md#claude-code)).
- **cursor-agent**: two Keychain items, `cursor-access-token` and
  `cursor-refresh-token` ([providers.md](providers.md#cursor)).
- **Codex, Grok, Kimi**: the same files as on Linux.

Over SSH the login Keychain is locked. `security` then reads nothing, and
remuda refuses a Claude Code or Cursor login the Keychain holds, naming
`security unlock-keychain` ([decisions.md](../decisions.md)). It never prompts
and never waits on a dialog.

## Services

The refresh timer and `serve` are launchd agents rather than systemd units
([launchd.rs](../../src/launchd.rs)), which the binary writes itself:

- `~/Library/LaunchAgents/com.github.arzaroth.remuda.refresh.plist` runs
  `~/.local/bin/remuda refresh --scheduled` every 1800 seconds.
- `com.github.arzaroth.remuda.serve.plist` runs `remuda serve` at load and
  again after a crash (`KeepAlive` `Crashed`), the closest to
  `Restart=on-abnormal`.
- Both set `REMUDA_SERVICE=1`, a `PATH` starting with `~/.local/bin` and
  Homebrew's, and the directory variables the installing shell set
  (`CLAUDE_CONFIG_DIR`, `CODEX_HOME`, `REMUDA_STORE` and the others
  `environment.d` holds on Linux), and log to `~/Library/Logs/remuda`.

`install.sh` asks the binary for them (`remuda sync-units --install timer
--install serve`), which writes each plist, then `launchctl bootout`,
`enable` and `bootstrap gui/$UID`. That needs someone logged in to the Mac's
desktop, SSH or not. After an update, `sync-units` rewrites the plists that
differ, as it does the systemd units, and `launchctl kickstart -k` restarts a
loaded `serve` (and an agent whose plist changed). The page's update runs
`sync-units` in a process group of its own, so the restart of `serve` does not
end it too.

`units::Manager` hides which one runs: names, states and fixes for the doctor,
`remuda open` starting `serve`, and whether a running `serve` is the
service's (on macOS its pid is the one `launchctl print` gives). The states
are `absent` (no plist), `masked` (`launchctl print-disabled` says
disabled), `enabled` (loaded) and `disabled` (a plist, not loaded).

## Processes

- Start times come from `proc_pidinfo` in microseconds since the epoch
  ([pidstart.rs](../../src/pidstart.rs)); selvedge reads only `/proc`. Claude
  Code writes its `procStart` there as `LC_ALL=C TZ=UTC ps -o lstart=`
  prints it, so a session file is compared in that form, to the second.
- Auto mode lists processes with `proc_listallpids` and reads their
  arguments and environment with `sysctl(KERN_PROCARGS2)`, which answers for
  this user's own ([auto.rs](../../src/auto.rs)).

## Browser

A window can be shown in any session but an SSH login, and a link opens in
the default browser through `/usr/bin/open`, never in a private window: the
browsers' private-window flags need their binaries on `PATH`, which a Mac's
apps are not. Over SSH, sign-ins paste their code as on a Linux machine
without a display.

## The proxy on the LAN

A non-Apple binary run by launchd, `serve` and its proxy included, is subject
to macOS's Local Network privacy: until the user allows remuda in System
Settings, Privacy & Security, Local Network, its traffic with LAN addresses
fails with `EHOSTUNREACH`. The card's firewall check knows only ufw and
firewalld, so a Mac gets no firewall notice either. remuda works around
neither.

## Release

The release builds `aarch64-apple-darwin` on `macos-15` and
`x86_64-apple-darwin` on `macos-15-intel`, packed as
`remuda-<tag>-macos-<arch>.tar.gz` beside the Linux archives, so `SHA256SUMS`
and its signature cover them and `remuda --update` finds its own. They are not
signed or notarized by Apple: `install.sh` and `--update` download with curl
and reqwest, which set no quarantine flag, while an archive downloaded in a
browser needs `xattr -d com.apple.quarantine` first. There is no Homebrew
formula.

## Sources

- [src/keychain.rs](../../src/keychain.rs), [src/securestorage.rs](../../src/securestorage.rs)
- [src/launchd.rs](../../src/launchd.rs), [src/units.rs](../../src/units.rs) `Manager`
- [src/pidstart.rs](../../src/pidstart.rs)
- [src/auto.rs](../../src/auto.rs) `listed`, `procargs`
- [src/browser.rs](../../src/browser.rs) `OPENER`, `shows`
- [src/cursor.rs](../../src/cursor.rs)
- [scripts/install.sh](../../scripts/install.sh)
- [.github/workflows/release.yml](../../.github/workflows/release.yml)
