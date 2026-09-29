# Changelog

## [Unreleased]

### Fixed

- Adding an account no longer signs your browser's own claude.ai or ChatGPT
  session over to it. `remuda login` and the `serve` page open the sign-in page
  in a private window of the default browser (Brave, Chrome, Chromium, Vivaldi,
  Edge, Firefox, LibreWolf; not Snap or Flatpak packages) instead of a normal
  tab. With another browser, or no display, they fall back to the old behaviour. The page can also copy the
  sign-in link for pasting into a private window by hand.

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
