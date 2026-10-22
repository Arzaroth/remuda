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
client of its own. After a refresh or a switch remuda only asks TokenGauge's
daemon to fetch sooner, through its socket.

Spending a Claude banked reset is the one exception: right before the POST,
remuda asks the usage endpoint itself which grant the server would accept now.
The snapshot only says how many are left, may be minutes old, and carries no
grant id, and spending the wrong one, or one already gone, wastes a click at
best and a reset at worst.

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

The units say they are one (`--scheduled`, `REMUDA_SERVICE=1`) rather than
remuda guessing from `INVOCATION_ID`: GNOME runs its terminal as a systemd
user service, so every shell it opens carries that variable, and its
commands never recorded anything. Units from before the marker are known by
their cgroup (`remuda-*.service`), since nothing guarantees an update
rewrote them, and one taken for a shell would refresh the shell's active
login.

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
fetches them, and `remuda --update` uses selvedge, the updater TokenGauge and
TailGauge share, pinned by tag. remuda has no desktop frontends, so selvedge
replaces the binary alone, and the new binary then updates the systemd units
it carries, since the old one cannot know them.

## The page is built from `web/`, and the build is committed

The page outgrew one hand-written file, so it is a Solid and TypeScript app
under `web/`, built by Vite into the single `src/serve.html` that `serve.rs`
compiles in. The built file is committed rather than built by `cargo`, so
building or releasing remuda needs no Node. The price is a generated file to
keep in step: CI rebuilds it and fails when it differs, and it is never edited
by hand. Solid was picked over Svelte because codegauge's page already uses
it, with the same Vite setup.

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

## The page edits the config the user edits by hand

`config.toml` is written by the page and by the user's editor, so a page save
rewrites only the values that changed and keeps every comment, rather than
serialising the config afresh. It is saved against the version the page read,
the digest of the file's bytes, so a hand edit made while the page was open is
refused rather than overwritten ([config.rs](../src/config.rs) `save`).

## Auto mode runs inside `serve`

The page's service already runs all the time and already follows TokenGauge's
snapshot, so auto mode is a thread there rather than a third unit or a run of
the refresh timer, whose half hour is too coarse for a 5-hour window. The
price: no `serve`, no auto mode.

## Auto mode spends first what a reset would wipe

Within a tier the score is the headroom left in the longest window per hour
until it resets, not the headroom alone: picking the login with the most left
leaves another's week to expire unspent. The same idea frees a reserved login
outside its schedule when its window resets before the schedule starts.

## Auto mode waits for Codex sessions to end

Claude Code checks its credential file before each request and writes back
only over the token it started from, so a switch reaches every open session.
Codex never re-reads `auth.json` while it runs, fails on its next refresh when
another account is there, and a refresh in flight writes the old account's
tokens into the new file (codex-cli 0.162.1,
`codex-rs/login/src/auth/manager.rs`). So auto mode switches Codex only while
no `codex` process runs.

## The proxy spends the active login and never refreshes it

A request through the proxy uses the login `remuda use` made live, read again
on every request, so switching a CLI switches its clients too and there is no
second "active" login to keep in step. Refreshing it is the CLI's job, as
everywhere else in remuda ("Never refresh the active credential"): the CLI
holds that refresh token, and a refresh behind its back would leave it
holding a dead one. So an expired access token is refused with a 401 that
says to run the CLI or switch, rather than renewed. Pinning the proxy to a
stored, inactive login it could refresh was turned down: it never stalls,
but it is a second active login per CLI to manage.

## Keep-warm spends the active login and waits for TokenGauge between pings

A ping on the login a CLI is on uses its live credentials and never refreshes
them, as the proxy does, for the same reason: the CLI holds that refresh token.
Pinging only inactive logins was turned down, since the active one is the
login whose window matters most. A login is pinged again only once TokenGauge has asked about
that login since the last ping: remuda has no figures of its own, and until
TokenGauge asks, a closed window still looks closed, so without the wait every
round would ping again. The snapshot's own time is not enough, since
TokenGauge carries a login the CLI is not on between asks, and a nudge after
each ping would otherwise turn into a loop. A failed ping waits 10 minutes
for the same reason.

## The proxy has its own listener, guarded by its own keys

The page's listener reads bodies of at most 64 KiB, refuses chunked ones and
buffers every answer, which its guarantees rest on; a proxy streams
multi-megabyte requests and long answers. So the proxy runs a second listener,
reusing only the head reader and the connection slots. Its guard is an access
key the client sends as an API key, checked before any body is read, not the
page's token, `Host` and `Origin`: proxy clients are SDKs and CLIs that know
nothing of the page. It binds `127.0.0.1` only; listening further is the hub
work on the roadmap. The keys sit in clear in the store's `.proxy.json`,
beside the credentials they give access to, so the page can show and copy
them again.

## The proxy routes by model name, translates, and a Claude token goes out as Claude Code

One base URL serves every client: the path gives the format, the model's
prefix gives the provider, as CLIProxyAPI does, and `provider/model` covers a
name two providers share. Any format reaches any provider: rather than a
translator per pair of formats, as CLIProxyAPI has, requests go through one
conversation shape and answers through one stream of events, so a parser,
an emitter, a decoder and an encoder for each of the three formats cover
every pair, and a new format is one of each. A Claude subscription token is accepted for Claude
Code's own requests, so a request from any other client is sent with Claude
Code's headers and its identity line as the first system block. That is
against Anthropic's consumer terms for a client that is not Claude Code, and
the README says so; Claude Code's own requests pass untouched.

## The log is a file of its own, and a repeated failure is one line

What remuda did goes to `<store>/.log.jsonl`, not into the sidecars or
`.last-switch.json`: the sidecars are TokenGauge's contract, and the records
keep only the latest of each thing. It is a dot-file at the store's root, so
TokenGauge never takes it for a credential. It keeps its own lock, since most
lines are written under the store's and a second `flock` on that file from the
same process would wait on itself. A failure that repeats is written once,
until something about that login goes right: the refresh timer runs every half
hour, and a revoked spare would otherwise fill the log with one line per run
([features/log.md](features/log.md)).

## A run session links the CLI's directory and copies only the login's files

`remuda run` points the CLI at a directory of its own rather than switching,
so every other session keeps its login. Symlinking every entry keeps history,
projects and settings shared. The files that hold the login are copied
instead: Claude Code's `.claude.json` carries `oauthAccount` as well as
projects and MCP servers, and a link would let the session write another
account into the file every other session reads. The copies are merged back
key by key when the session ends, the login's keys excepted, so what the
session changed is not lost. The CLI's own copy wins where both changed. A
lease, the `flock` the session holds, marks the login in use rather than a
pid file, since the kernel drops it however the process dies. A directory
left with no lease held is a session that died, finished by the next command
([features/run.md](features/run.md)).

## A pin is read only from a file the user owns

A `.remuda.toml` chooses which of the user's logins spends in a directory, and
a cloned repository could carry one. As git does with `safe.directory`, a file
another user owns is ignored. An unpinned `remuda run claude` `exec`s the CLI
without touching the store, so shell functions that route every `claude`
through remuda cost nothing where nothing is pinned.

## Packages are made from the release archive

The `.deb`, the `.rpm` and `remuda-bin` repackage the per-architecture archive
rather than building in each distribution, so every install method runs the
bytes `--update` would install, and CI needs no toolchain per distribution.
That only works because the binary is static musl: the glibc build needed
2.39, and building it on an older runner only moves that floor. A package
enables no unit, since a package must not write into a home and the timer
needs `remuda ls` run first from the user's shell (see
[architecture/distribution.md](architecture/distribution.md#packages)).

## Sources

- [src/ops.rs](../src/ops.rs)
- [src/claude.rs](../src/claude.rs)
- [src/browser.rs](../src/browser.rs)
- [src/codex.rs](../src/codex.rs)
- [src/commands.rs](../src/commands.rs)
- [src/serve.rs](../src/serve.rs)
- [src/served.rs](../src/served.rs)
- [web/](../web/)
- [src/project.rs](../src/project.rs)
- [src/proxy/](../src/proxy/)
- [packaging/nfpm.yaml](../packaging/nfpm.yaml)
