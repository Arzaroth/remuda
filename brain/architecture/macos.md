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
  again after a failure (`KeepAlive` `SuccessfulExit` false), at most every
  30 seconds (`ThrottleInterval`), like the unit's `Restart=on-failure`.
  launchd has no start limit, so it never gives up.
- Both set `REMUDA_SERVICE=1`, a `PATH` starting with `~/.local/bin` and
  Homebrew's, and the directory variables the installing shell set
  (`CLAUDE_CONFIG_DIR`, `CODEX_HOME`, `REMUDA_STORE`, `XDG_DATA_HOME`,
  `XDG_CONFIG_HOME` and the others `environment.d` holds on Linux), and log
  to `~/Library/Logs/remuda`. Each is read as remuda reads it, its
  `REMUDA_OUTER_` form first inside a `remuda run` session, and a relative
  directory is written absolute, since launchd starts the agent in `/`. A
  variable the shell does not set keeps the value the installed plist has.

`install.sh` asks the binary for them (`remuda sync-units --install timer
--install serve`), which writes each plist, then `launchctl bootout`,
`enable` and `bootstrap gui/$UID`. That needs someone logged in to the Mac's
desktop, SSH or not. After an update, `sync-units` rewrites the plists that
differ, as it does the systemd units, and boots out and bootstraps again a
loaded agent whose plist changed. An unchanged loaded `serve` gets
`launchctl kickstart -k`, onto the new binary. The page's update runs
`sync-units` in a process group of its own, so the restart of `serve` does not
end it too.

A loaded timer is booted out, by a sync or by `--install timer`, only holding
the lock of the store it refreshes, which every refresh holds while it runs,
waiting up to five minutes for one to end, so a bootout never cuts one before
it saves the tokens it rotated. That store is found as a refresh in that job
finds it, from the environment `launchctl print` shows for it (its own, then
what it inherits, as from `launchctl setenv`, then launchd's default):
`REMUDA_STORE`, else `XDG_DATA_HOME`'s, else the default, never from the plist
on disk. A whole `sync-units` holds `~/Library/LaunchAgents/.remuda.lock`, so
two never rewrite the agents at once.

When something fails, the sync fails and says what is left:

- `launchctl print` failing with anything but "not found" (113): the install
  refuses, and a sync puts that agent's old plist back.
- A reload that fails: the old plist goes back and is loaded again, so the
  next sync sees the difference and retries.
- The old plist cannot be written back, or loaded again: the error says so
  and names the `launchctl bootstrap` that loads it. A later sync skips an
  agent that is not loaded, so until then it is the doctor's `disabled`
  state, with that command as its fix.

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

A window can be shown in any session but an SSH login. A browser chosen by
name (`--browser`, or `browser` in `config.toml`) whose binary is on `PATH` is
launched as on Linux, with its private-window flag when one is asked for.
Otherwise a link opens in the default browser through `/usr/bin/open`, which
cannot be asked for a private window: remuda does not read the Mac's default
browser, and the apps' binaries are seldom on `PATH`. Over SSH, sign-ins paste
their code as on a Linux machine without a display.

## The proxy on the LAN

From macOS 15, a LaunchAgent's process, `serve` and its proxy included, is
subject to Local Network privacy (Apple's TN3179). Listening and accepting a
LAN client's connection need no permission, but a connection remuda opens to
a LAN address, such as an upstream there, fails with `EHOSTUNREACH` until the
user allows remuda in System Settings, Privacy & Security, Local Network. A
`serve` started from Terminal or over SSH, and any Mac before macOS 15, are
not held to it.
The card's firewall check knows only ufw and firewalld, so a Mac gets no
firewall notice either. remuda works around neither.

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
