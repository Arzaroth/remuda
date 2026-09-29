# The local page

`remuda serve [--port 7429] [--no-browser]` binds `127.0.0.1` only,
prints `http://127.0.0.1:<port>/#<token>`, and opens it. The token is 32 random
bytes, base64url. The browser is not given the URL as an argument (every local
user can read a command line): remuda writes a redirect page to
`$XDG_RUNTIME_DIR/remuda/open-<random>.html` (0600, in a 0700 directory; one
per open, removed ten minutes on) and opens
that. The same goes for the sign-in URLs `login` and the page open, which go
to a private window when the default browser allows it.

The page reads the token from the fragment (fragments never reach a server log
or a `Referer`), keeps it in `sessionStorage`, strips it from the address bar,
and sends it as `X-Remuda-Token`.

## Guards (`App::handle`)

1. `Host` must be `127.0.0.1:<port>` or `localhost:<port>`: defeats DNS
   rebinding.
2. An `Origin`, when sent, must be that same listener: defeats a cross-site
   `fetch` from another tab.
3. Every `/api/` call needs the token. `GET /` and `GET /favicon.svg` (the
   icon, `assets/remuda.svg`, compiled in) are served without it; neither holds
   data.

## The listener (`http.rs`)

remuda reads requests itself rather than through an HTTP library, because the
order matters: the request line and headers are read within a 5 s deadline and
a 16 KiB cap, `App::check` decides from them alone whether the request may be
served, and only then is a body read, at most 64 KiB, within the same
deadline. A client without the token never gets to make remuda wait on a
body. A chunked body gets a 411. At most 32 connections are handled at once;
the next gets a 503 and is closed. Each runs on its own thread, because a
Codex `login/finish` holds its request open until the browser calls back, and
a thread that cannot start is a refused connection rather than a stopped
server. Every answer closes the connection after draining what the client
sent, so it is not reset before it reads the answer: for at most 300 ms in
all, and on the accepting thread (the 503) only what has already arrived, so
no client can hold a slot past its deadline or stall the accept loop.
Responses carry `Cache-Control: no-store`, `nosniff` and `no-referrer`.

What the limits do not stop: another local user holding 32 idle connections,
re-opened every 5 s, keeps the page answering 503. That is a nuisance with no
access to anything, and it ends when they stop.

## API

| Route | Does |
| --- | --- |
| `GET /api/state` | Same JSON as `ls --json`, plus `providers`, `health` and `usage` |
| `POST /api/use` `{name, discard?}` | Switch |
| `POST /api/import` `{provider?, name, force?}` | Store the live login |
| `POST /api/label` `{name, text?}` / `rename` `{name, to}` / `remove` `{name}` | As the commands |
| `POST /api/refresh` `{name?, force?}` | Refresh inactive credentials |
| `POST /api/login` `{provider?, name, force?}` | Begin a sign-in and open it in a private window: `{id, url, needsCode, opened}` |
| `POST /api/login/finish` `{id, code?}` | Finish it and store the result |
| `POST /api/login/cancel` `{id}` | Abandon it; a waiting Codex callback stops |

Pending sign-ins live in memory, one per provider; starting another cancels the
previous one. `App::api` takes the store lock once for every route except the
`/api/login` ones, which lock only to save (`commands::save_login`). The name
check before a sign-in and the "stored ..." message are `commands::begin_login`
and `commands::stored_message`, shared with the CLI.

`/api/state` reports each CLI's live login as `signed_out`, `stored` (with
`confirmed`), `unstored`, `foreign` (an API key; the page says only
`remuda use --discard` replaces it) or `unreadable` (with the error).

`health` is `timer`, from systemd's user unit search path in its order
(`paths::systemd_user_units`: the config directory, `/etc`, the runtime
directories, the data directory and `$XDG_DATA_DIRS`, `/usr/local/lib`,
`/usr/lib`): `masked` when the first copy of the unit is a link to
`/dev/null`, `enabled` when a `timers.target.wants/remuda-refresh.timer` link
that still resolves exists in any of them, `disabled` when only the unit does,
else `absent`. An enabled timer that was stopped still reads as enabled, and
shows as a late last run. Then `lastRefresh` (`runs::last`, or null), `tokengauge` (whether the snapshot exists) and `switchedAt` (provider
id to the time of its last switch, `runs::switches`).

`usage` is `gauge::read` of TokenGauge's snapshot, or null. The snapshot is
where TokenGauge puts it: the top-level `cache_file` of its config
(`$TOKENGAUGE_CONFIG`, else `$XDG_CONFIG_HOME/tokengauge/config.toml`),
unless that is TokenGauge's old temp path, which TokenGauge itself replaces
with the default `$XDG_STATE_HOME/tokengauge/tokengauge-usage.json`. It is
read only when it is a regular file, and at most 4 MiB of it. `usage` holds
`updatedAt` and, per provider id, `stale`, `staleReason`, `error` (from the
snapshot's top-level `errors`, where TokenGauge files a fetch that failed with
nothing cached), `windows` (`title`, `usedPercent`, `resetsAt`),
placeholders dropped, and `accounts`: the same fields per stored credential
name, for payloads that carry an `account`. A row takes its own entry in
`accounts`. When `accounts` is empty the snapshot names no credential, so the
provider-level figures belong to the active login and are shown only when the
snapshot is newer than its last switch. `App`
takes both locations as `Places`, so tests point them at a temporary
directory.

The page is one self-contained file (`include_str!`), no external requests,
light and dark. It reloads the state every minute unless an editor is open.

## Sources

- [src/serve.rs](../../src/serve.rs)
- [src/http.rs](../../src/http.rs)
- [src/serve.html](../../src/serve.html)
- [src/main.rs](../../src/main.rs)
