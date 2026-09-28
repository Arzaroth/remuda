# Changelog

## [Unreleased]

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
