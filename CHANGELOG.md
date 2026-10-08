# Changelog

## [Unreleased]

### Fixed

- On a desktop that runs its terminal as a systemd service (GNOME), commands
  typed in it were taken for systemd runs: they never recorded where the
  CLIs keep their logins, so `remuda-serve.service` would not start, and
  `import` refused a missing store. remuda now tells its own units by their
  name, and `remuda update` installs units that also say so
  (`REMUDA_SERVICE=1`).

### Changed

- Only `import` and `login` create the store. `ls`, `use`, `serve` and the
  other commands now refuse when there is none, naming what they were doing,
  so an unset or mistyped `REMUDA_STORE` no longer starts an empty second
  store. On a fresh install, start with `remuda import` or `remuda login`;
  `install.sh --serve` leaves the page's service for `remuda open` to start
  after that.

## [0.6.0] - 2026-10-03

### Changed

- `remuda update` also brings the systemd units you installed up to date
  with the new release, and reloads systemd when one changed. It never adds a
  unit you did not install: `remuda-serve.service` still comes only from
  `install.sh --serve`. This takes effect from the update after this release.

## [0.5.0] - 2026-10-03

### Added

- `remuda open` opens the page a running `remuda serve` shows, so you no
  longer need the URL it printed. With `--no-browser` it prints the URL.
- `install.sh --serve` keeps the page served by a systemd user service,
  `remuda-serve.service`, so `remuda open` works any time; `open` starts the
  service when it is stopped. Like the refresh timer, the service refuses to
  start when the shell has moved `CLAUDE_CONFIG_DIR` or `CODEX_HOME` since,
  and it never writes the page's token to the journal. `remuda update` and
  the installer restart it on the new binary.

### Changed

- The `serve` page updates itself within a couple of seconds when the store
  or TokenGauge's usage changes, for example after a switch from the CLI, a
  refresh-timer run or a new TokenGauge fetch, instead of only once a minute.

## [0.4.2] - 2026-09-29

### Fixed

- The `serve` page shows each login's own usage with TokenGauge 0.37 and
  later, which reports every stored login. It used to show the last login
  TokenGauge listed as the one in use, so another account's figures could
  appear under it. A login TokenGauge skipped, because its access token
  expired or it is unverified, says so.

## [0.4.1] - 2026-09-29

### Fixed

- Adding an account no longer signs your browser's own claude.ai or ChatGPT
  session over to it. `remuda login` and the `serve` page open the sign-in page
  in a private window of the default browser (Brave, Chrome, Chromium, Vivaldi,
  Edge, Firefox, LibreWolf; not Snap or Flatpak packages) instead of a normal
  tab. With another browser, or no display, they fall back to the old
  behaviour. The page can also copy the sign-in link for pasting into a private
  window by hand.

## [0.4.0] - 2026-09-29

### Added

- remuda has an icon: a horseshoe holding a keyhole. The `serve` page uses it
  as its favicon, served by remuda itself.

### Changed

- On the `serve` page, Enter confirms the Import name and the pasted sign-in
  code too, as it already did the Add an account name and the editors.

## [0.3.0] - 2026-09-29

### Added

- The `serve` page can switch away from a live login remuda cannot keep, and
  replace a stored credential on import or sign-in, each after a second click,
  as `use --discard` and `--force` do.

### Fixed

- The `serve` page finds the refresh timer wherever systemd loads user units
  from, not only `~/.config/systemd/user`, and reports a masked timer as
  masked rather than enabled.
- `use --discard` no longer turns off the check that keeps a login the CLI
  rotated mid-switch when the live login is stored and confirmed.

## [0.2.0] - 2026-09-29

### Added

- The `serve` page shows how long each credential's access token and sign-in
  have left, whether the refresh timer is on and what its last run refreshed
  or failed on, and, with TokenGauge installed, the usage of the login in use.
- The scheduled refresh records its last run in `<store>/.last-refresh.json`.
- A switch records when it happened in `<store>/.last-switch.json`, so the
  page does not show the previous login's usage for the new one.

### Changed

- The `serve` page is redesigned: a summary per CLI whose limit you pick, then
  one row per credential with meters and icon actions, and a light/dark
  switch.

## [0.1.1] - 2026-09-28

### Fixed

- `serve` checks a request's host and token before reading its body, bounds
  how long a client may take and how many connections it serves at once, and
  no longer stops when another local user holds connections open.
- A credential whose sidecar was written for other tokens (a save interrupted
  halfway) is detected and re-identified with the provider, even while the CLI
  is signed out or the access token has expired, instead of being refreshed,
  switched to or labelled under the wrong account.
- Switching no longer reverts changes Claude Code saved to `.claude.json` or
  `.credentials.json` meanwhile, and never overwrites a login the CLI rotated
  mid-switch.
- The refresh timer refuses to refresh a CLI whose login it would look for in
  another place than your shell does, including with a unit installed by
  0.1.0.

## [0.1.0] - 2026-09-28

- `import`, `login`, `use`, `ls`, `refresh`, `label`, `rename` and `rm` for
  Claude Code and Codex logins. A name is bare when only one CLI has it, and
  `claude/<name>` or `codex/<name>` otherwise.
- `serve`: a local page to list, switch, label, rename, remove, import and sign
  in credentials, guarded by a token in the URL it prints.
- `completions <shell>` for bash, zsh, fish and the other shells clap knows.
- `update`, and an installer that fetches the release, enables the refresh
  timer and installs completions.
