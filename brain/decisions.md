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

## Match by token, then by who the tokens say they are

Matching the live login to a credential tries the refresh and access tokens
first. When they have rotated, it asks `Provider::identify` whose tokens they
are and files them under that account's credential. It does not trust the
account the CLI's files name: for Claude that is `.claude.json`, a different
file from the tokens, and it lags them whenever another session or tool writes
one and not the other. Filing tokens under the wrong name destroys a
credential; leaving them unfiled, as an earlier version did, left the rightful
one holding a spent token. Offline, nothing is copied and `use` refuses to
switch away (`--discard` overrides). `import` asks the same question before
storing, and trusts the files offline only while nothing is stored to collide
with.

## A Codex account is a seat, not a workspace

`auth.json`'s `tokens.account_id` is the ChatGPT workspace, which every seat of
a Team plan shares, so keying on it made two people one account. The identity
is the access token's `chatgpt_account_user_id`. It is read out of the token
that authenticates, so no file can disagree with it and no network is needed.

## An older copy never overwrites a newer one

Tokens only move forward, and the newer copy expires later. If the live login
expires before the stored copy of the same account (a store refreshed by a
timer that could not see the live login), copying it back would replace a
working refresh token with a spent one, so the stored copy is kept.

## Tokens rotated for the wrong account are kept

A refresh whose answer names another account than the credential's cannot be
saved there, but the server has already spent the old refresh token, so the
new tokens are the only copy. They go to a hidden `.set-aside-...` file.

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

The same split holds for usage the other way round: the page shows it by
reading TokenGauge's snapshot, never by calling a usage endpoint itself, so
TokenGauge stays the one process fetching usage and remuda needs no usage
client of its own.

## The page reads its own requests

An HTTP library that reads a body before handing over the request lets anyone
who can connect make remuda wait, and one that gives each connection a thread
with no limit lets them exhaust it. The page's needs are a dozen routes on
loopback, so `http.rs` reads requests itself, in the order that matters:
head, checks, and only then a capped body, with a connection limit.

## A sidecar says which tokens it describes

A credential and its sidecar cannot be written in one rename without breaking
the layout TokenGauge reads, so the sidecar records the credential file's
digest instead. A torn write is then detected, not trusted, and repaired by
asking the provider.

## Edits to a file Claude Code also writes are compare-and-swap

Claude Code rewrites `.claude.json` and `.credentials.json` without a lock
remuda could take. Reading, editing and renaming would revert whatever it
saved in between, so the rename happens only if the file still holds what was
read. The comparison is on content: inode, size and mtime miss a same-length
rewrite within one timestamp tick.

## A switch never lands on a login the CLI just rotated

Retrying the edit is right for `.claude.json`, but for the credentials it
would re-apply the switch on top of a refresh token the CLI had just rotated,
dropping it while the store holds the spent one. So `install` writes only
while the live login is still the one the switch synced, and `switch`, when it
moved, syncs the new tokens back before trying again.

## The timer checks it sees what the shell sees

A systemd user service does not inherit a shell's environment, and a timer
that looks in the default place for a login kept elsewhere refreshes the
active credential. The installer copies the variables once; the directories
record catches a change made later. It fails closed: no record, no refresh.
It lives in its store rather than under `$XDG_STATE_HOME`, a variable the
timer does not see either, and so that a one-off run against a scratch store
cannot stop the timer for the real one. `remuda-serve.service` has the same
blind spot, so a `serve` under systemd is checked the same way and refuses to
start.

## A running page announces itself, its token apart

A page that runs all the time is reached through `remuda open`, not a URL
copied from a log: the token is per run, and a journal is persistent and
readable by more than the page. So `serve` writes the URL to a 0600 file only
`open` reads, and a separate `serve.json` with no secret for any other
program that wants to know whether a page is up, such as a launcher button
that then runs `remuda open`. Nothing cleans them up, because a killed
process cannot; readers check that the pid still has the recorded start time,
so a reused pid does not pass, and that the port answers.

## No secret on a command line

Every local user can read another's command lines in `/proc`, and a browser
started by `xdg-open url` keeps that URL in its arguments for its whole
session. The page's URL carries its token and a sign-in URL carries its state,
so the browser is handed a 0600 redirect file under `$XDG_RUNTIME_DIR` instead.

## Sign-in opens a private window

Signing in as a second account in the browser's normal profile signs its
claude.ai or ChatGPT session over to that account. A hint to use a private
window came too late on the page, which had already opened a normal tab, so
remuda opens the private window itself. A page cannot ask for one, so the
server does it. It knows the common browsers' flags only; any other gets
`xdg-open` and the hint, since a wrong flag could open nothing at all.

## The page is guarded by a token, Host and Origin

A page on any site can make the browser send requests to `127.0.0.1`, and DNS
rebinding can make one look same-origin. So `serve` requires the random token
from the URL it prints, a `Host` naming its own listener, and no foreign
`Origin`. See [architecture/web.md](architecture/web.md).

## The page follows changes over server-sent events, read by `fetch`

The page learns of a change through `GET /api/events`, a server-sent event
stream, rather than a WebSocket: data only flows one way, and the
hand-written listener can hold a plain response open, where a WebSocket would
need its own handshake (SHA-1, which remuda does not otherwise carry) and
framing. The page reads it with `fetch`, not `EventSource`: neither
`EventSource` nor a browser WebSocket can send the `X-Remuda-Token` header,
and a token in the query string is one the fragment was chosen to avoid.
The server polls file metadata every 2 s rather than using inotify, so it
needs no new dependency and sees a store on any filesystem; the event says
only that something changed, and the page reloads `/api/state` as before.
One tab per browser holds the stream and relays it over a BroadcastChannel,
since a stream per tab would use up the browser's few connections per host.
See [architecture/web.md](architecture/web.md).

## selvedge for updates, a curl installer for installs

Users do not have cargo, so releases ship prebuilt archives, `install.sh`
fetches them, and `remuda update` uses selvedge, the updater TokenGauge and
TailGauge share, pinned by tag. remuda has no desktop frontends, so selvedge
replaces the binary alone.

## Only debug builds take a test endpoint

The rule is that nothing can redirect a token request, because a request
carries a refresh token. The end-to-end tests still need the built binary to
talk to a mock, so `Api::claude()` and `Api::openai()` read
`REMUDA_TEST_CLAUDE_API` / `REMUDA_TEST_OPENAI_API` under
`#[cfg(debug_assertions)]` only. The released binary is built with `--release`
and has no such variable.

## OAuth constants are copied from the installed CLIs

Client ids, endpoints and scopes are not a public API. They were read out of
Claude Code 2.1.282 and codex-cli 0.153.4 binaries and are pinned in one place
per provider, so a CLI release that moves them is one edit.

## Sources

- [src/ops.rs](../src/ops.rs)
- [src/claude.rs](../src/claude.rs)
- [src/browser.rs](../src/browser.rs)
- [src/codex.rs](../src/codex.rs)
- [src/commands.rs](../src/commands.rs)
- [src/serve.rs](../src/serve.rs)
- [src/served.rs](../src/served.rs)
- [src/project.rs](../src/project.rs)
