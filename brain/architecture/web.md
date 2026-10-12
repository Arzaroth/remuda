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

## Finding a running page (`served.rs`)

Once listening, `serve` writes two files into the same runtime directory
(`paths::runtime_dir`, the store when `$XDG_RUNTIME_DIR` is unset):
`serve.url`, the URL with its token (0600), and then `serve.json`,
`{pid, started, port, version}`, which holds no secret and is what other
programs read to learn a page is up. `started` is the process's start time,
field 22 of `/proc/<pid>/stat`. Nothing removes them: a page that stopped is
told apart by `/proc/<pid>/stat` no longer giving that start time (the
process is gone, or the pid was reused) or its port no longer answering, and
the next `serve` overwrites both. Two pages on two ports: the last one started
wins.

`remuda open [--no-browser]` (`served::find`) takes the URL only when the
status passes that check and `serve.url` names its port, then opens it through
a redirect file like everything else (or prints it). When nothing serves and
`remuda-serve.service` is found in the user unit search path, it runs
`systemctl --user start` on it and waits up to 10 s for the page.

## Under systemd

`systemd/remuda-serve.service` runs `remuda serve` (`Restart=on-abnormal`, so
a refusal is not retried) with `REMUDA_SERVICE=1`, and `serve` then:

- checks the directories record like the scheduled refresh
  ([distribution.md](distribution.md)), printing one line per CLI it would not
  serve and exiting non-zero rather than serve a login other than the
  shell's;
- prints the URL without its token, since stdout is the journal;
- opens no browser at start. Sign-ins still open their private window when
  the user manager has a display.

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
server. Every answer but the event stream below closes the connection after
draining what the client sent, so it is not reset before it reads the
answer: for at most 300 ms in all, and on the accepting thread (the 503) only
what has already arrived, so no client without the token can hold a slot past
its deadline or stall the accept loop.
Responses carry `Cache-Control: no-store`, `nosniff` and `no-referrer`.

`GET /api/events` is the one answer that stays open. After the guards (so
only a holder of the token gets one), it takes one of 8 stream slots, inside
the 32 connections, or gets a 503. It sends the head and `: connected`, then
every 2 s compares `App::fingerprint`, the path, modification time and size
of the store's `*.json` files, one directory level down, and of TokenGauge's
snapshot, links followed, and sends `data: changed` when it moved. Other files
in the store are left out: the temp files writes go through, and the redirect
pages kept there when `$XDG_RUNTIME_DIR` is unset. `App::streams` names the
route; the listener serves it itself, since it outlives one `Response`. After 15 s with nothing
to send it writes a `: ` comment, so a client that vanished is found by the
failed write. The client has nothing to send after its head, so anything it
sends, or its closing the connection, ends the stream and frees the slot.
Reading the state writes in the store only when it brings the live login
forward or confirms an unverified entry, which the next read then finds done
(and, on a fresh store, creates `.lock`, which is not JSON), so a page that
reloads on `changed` does not keep waking itself.

What the limits do not stop: another local user holding the free connections
(32, less one per browser following the page), re-opened every 5 s, keeps the
page answering 503. That is a nuisance with no
access to anything, and it ends when they stop.

## API

| Route | Does |
| --- | --- |
| `GET /api/state` | Same JSON as `ls --json`, plus `providers`, `health` and `usage` |
| `GET /api/events` | A server-sent event stream: `data: changed` when the page should reload the state |
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
nothing cached), `credentialState`, `planWeight` (TokenGauge's nominal plan multiplier, when it knows one: any positive number, since a Claude Team seat is 1.25 or 6.25), `windows` (`title`, `usedPercent`,
`resetsAt`), placeholders dropped, and `accounts`: the same fields per stored
credential name. TokenGauge 0.37 (snapshot schema 2) names the credential a
payload or an error belongs to in its `credential` field; those go under
`accounts`, the rest stay at provider level. A row takes its own entry in
`accounts`, looked up as an own key. The active login without one falls back
to the provider-level figures, which are then the live login's. When
`accounts` is empty the snapshot names no credential, so the provider-level
figures belong to the active login and are shown only when the snapshot is
newer than its last switch.

`shown` is the page's figures, worked out from the rest by
[shown.rs](../../src/shown.rs) so the page only draws: per CLI, `usage` (the
provider-level figures, or `{behind: true}` while the snapshot is older than
the last switch), `accounts` (what each stored login's card shows, by the
lookup above, `{passive}` or `{missing}` when TokenGauge has nothing for it),
`weighable`, and `weighted` and `absolute`: every window added up across the
logins that report it, each with `title`, `weighted`, `used`, `of`, `pooled`,
`parts` (`name`, `active`, `window`, `weight`), `widths` and `leftOut`. The sum
is `selvedge::plans`, the same code TokenGauge's ALL PLANS header runs, so the
two cannot disagree. `App`
takes both locations as `Places`, so tests point them at a temporary
directory.

The page is written in Solid and TypeScript under `web/`, and Vite with
`vite-plugin-singlefile` builds it into `src/serve.html`: one self-contained
file, scripts and styles inlined, which `serve.rs` compiles in
(`include_str!`). The built file is committed, so building remuda needs no
Node; after a change under `web/`, `pnpm build` there regenerates it, and CI
fails when the two differ. `pnpm dev` serves the source with hot reload and
passes `/api` to a running `remuda serve` on `$REMUDA_PORT` (7429 by
default), dropping the `Origin` it would refuse; open
`http://localhost:5173/serve.html#<token>`. The page makes no external
requests and has a light and a dark theme. It reads `/api/events` through `fetch` (an `EventSource`
cannot send the token header). A browser allows only about 6 connections per
host across all its tabs, so one tab follows the stream, holding the Web Lock
`remuda-events`, and passes each change to the others over the
BroadcastChannel `remuda-changes`; when it closes, the next tab takes the lock.
Every tab reloads the state on each `changed`, on the stream reconnecting, and
every minute, which keeps the relative times current and catches what the
stream does not watch: the CLIs' own files (a sign-in made in the CLI) and the
systemd unit directories (the timer's state), which show within a minute.
Only the newest load is drawn, so a slow one never overwrites a later one. A dropped stream is retried after 1 s,
doubling to 30 s; a 401 (the server restarted with another token) stops it
and frees the lock for another tab.
None of these reloads happens while an editor is open, a confirmation is
armed or an input in the list has the focus. A focused button does not hold
them: Solid keeps the cards' buttons across a redraw, so a clicked one keeps
the focus.

## Sources

- [src/serve.rs](../../src/serve.rs)
- [src/served.rs](../../src/served.rs)
- [src/http.rs](../../src/http.rs)
- [web/](../../web/), built into [src/serve.html](../../src/serve.html)
- [src/main.rs](../../src/main.rs) `Serve`, `open`
- [systemd/remuda-serve.service](../../systemd/remuda-serve.service)
