# Decisions

Each entry is a choice a reader might want to undo, with the reason not to.

## Copy credentials, never symlink them

A switch writes the stored login into the CLI's file. A symlink from the CLI's
file into the store looks simpler but breaks silently: a CLI that saves through
a temp file and a rename replaces the symlink with a plain file, and from then
on the store stops following it.

## Sync back before every command

A CLI rotates the refresh token of the account it is signed into. The copy in
the store then holds a refresh token the server has already spent, and
switching back to it would fail. So every command first copies the live tokens
into the credential they belong to ([ops.rs](../src/ops.rs) `sync_live`).

## Match by token, then by confirmed account id

Matching the live login to a credential tries the refresh and access tokens
first. When they have rotated, it falls back to the account id, but for Claude
the account id lives in `.claude.json` while the tokens live in
`.credentials.json`, and the two can disagree (a login done by another tool, a
half-written switch). Filing new tokens under the wrong name would destroy a
credential, so the fallback asks the profile endpoint whose tokens they are
before copying anything. Offline, it copies nothing and `use` refuses to switch
away (`--discard` overrides). Codex keeps the account id inside `auth.json`
with its tokens, so its confirmation needs no network.

## Never refresh the active credential

The CLI refreshes its own login on its own schedule. If remuda refreshed it too,
the two would race on one rotating refresh token and one of them would lock the
other out. `refresh` skips the active credential; `use` refreshes the target
only when it is about to go live.

## Swap only the login's keys

For Claude Code, `claudeAiOauth` in `.credentials.json` and `oauthAccount` in
`.claude.json` are rewritten and every other key is kept, notably `mcpOAuth`, so
MCP server logins stay shared across accounts. The account block is written
first: Claude Code adopts a login when the credentials file changes, and by
then its UI already shows the right account. Codex's `auth.json` holds nothing
but the login, so it is replaced whole.

## Only ChatGPT sign-ins for Codex

An API key or personal access token has no account to switch between, so
`Codex::live` treats a file without OAuth tokens as signed out.

## The store lock is never held across a wait on the user

Every command holds `<store>/.lock` while it reads and writes. A sign-in waits
minutes on the browser, and the refresh timer would stall behind it, so
`login` takes the lock only to save, and re-checks the name then.

## TokenGauge reads the store, remuda writes it

Usage meters per credential belong to TokenGauge, whose ADR 0003 defines the
store as a contract. remuda owns every write, TokenGauge only reads, so there
is exactly one process that refreshes a stored token.

## The page is guarded by a token, Host and Origin

A page on any site can make the browser send requests to `127.0.0.1`, and DNS
rebinding can make one look same-origin. So `serve` requires the random token
from the URL it prints, a `Host` naming its own listener, and no foreign
`Origin`. See [architecture/web.md](architecture/web.md).

## selvedge for updates, a curl installer for installs

Users do not have cargo, so releases ship prebuilt archives, `install.sh`
fetches them, and `remuda update` uses selvedge, the updater TokenGauge and
TailGauge share, pinned by tag. remuda has no desktop frontends, so selvedge
replaces the binary alone.

## OAuth constants are copied from the installed CLIs

Client ids, endpoints and scopes are not a public API. They were read out of
Claude Code 2.1.282 and codex-cli 0.153.4 binaries and are pinned in one place
per provider, so a CLI release that moves them is one edit.

## Sources

- [src/ops.rs](../src/ops.rs)
- [src/claude.rs](../src/claude.rs)
- [src/codex.rs](../src/codex.rs)
- [src/commands.rs](../src/commands.rs)
- [src/serve.rs](../src/serve.rs)
- [src/project.rs](../src/project.rs)
