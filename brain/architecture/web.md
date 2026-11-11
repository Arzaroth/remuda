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
and sends it as `X-Remuda-Token`. A tab opened from one of the page's links
starts without it, so before its first request it asks the other tabs over the
BroadcastChannel `remuda-token` (same origin only, so only this server's
page), and waits a second for an answer before giving up; every tab holding
the token answers (`web/src/token.ts`). A tab left from an earlier `serve` on
the same port answers too, with a token the server no longer takes, so the
new tab tries each answer on `GET /api/token`, which answers `{}` to a
request carrying the token, and keeps the first one it takes. Only a 200
counts: a full server's 503 comes before the token is checked.

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

`remuda open [--no-browser] [--add]` (`served::find`) takes the URL only when the
status passes that check and `serve.url` names its port, then opens it through
a redirect file like everything else (or prints it). When nothing serves and
`remuda-serve.service` is found in the user unit search path, it runs
`systemctl --user start` on it and waits up to 10 s for the page. `--add` puts
`?add` in front of the fragment (`served::adding`); the page keeps that query
past taking the token and, once the state has loaded, drops it so a reload
does not open the sheet again, then opens the add-account sheet. TokenGauge's `+`
button runs this.

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

## Restarting on an update

A remuda update made from the page under the service ends in `sync-units`,
which restarts the service, and a new `serve` draws a new token, which the
open page does not have. So before it, the page's process writes
`serve.handoff` beside `serve.url`, `{port, at}` and no secret, and the next
`serve`, under the service only, takes the token from `serve.url` when the
marker names its port and is under a minute old. The marker is removed
whether it was taken or not, so any other restart draws a new token.

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
of the store's `*.json` and `*.jsonl` files (the log included, so a new line also starts a round of auto mode and keep-warm), one directory level down, of TokenGauge's
snapshot and its two window histories (a fleet sync cycle rewrites the view without the snapshot) and of `config.toml`, links followed, and sends `data: changed` when it moved. Other files
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
| `GET /api/token` | `{}`: a new tab checks a token another tab offered |
| `GET /api/state` | Same JSON as `ls --json`, plus `providers`, `health`, `usage` and `auto` (each CLI's last auto mode round, `{enabled: false}` for one it is off for, or `error` for a config it cannot read) |
| `GET /api/events` | A server-sent event stream: `data: changed` when the page should reload the state |
| `GET /api/log` | `{events}`: the log's latest 500 events, oldest first ([../features/log.md](../features/log.md)) |
| `GET /api/config` | `{config, version, problems, path}`: the config as JSON, the version to save against, what auto mode cannot act on |
| `POST /api/config/check` `{config}` | `{problems}` for a config the page has not saved, a malformed one reported `at: "config"`; a login not stored is one only when config.toml does not name it already |
| `POST /api/config` `{config, version}` | Save it ([config.md](config.md)), refused with any problem or when the file changed since `version` |
| `POST /api/use` `{name, discard?}` | Switch, then nudge TokenGauge |
| `POST /api/import` `{provider?, name, force?}` | Store the live login |
| `POST /api/label` `{name, text?}` / `rename` `{name, to}` / `remove` `{name}` | As the commands |
| `POST /api/refresh` `{name?, force?}` | Refresh inactive credentials, then nudge TokenGauge |
| `POST /api/reset` `{name, kind?}` | Spend one of a Claude login's banked usage-limit resets with its own token, of `kind` (`session`, `weekly`, `full`, `fable`) when given, refusing any other value, a non-string included, rather than spending the next grant, then nudge TokenGauge |
| `POST /api/login` `{provider?, name, force?, paste?}` | Begin a sign-in, opening nothing yet: `{id, url, needsCode, asksFor, fields, localOnly, userCode, machine, opensIn}`, `fields` being what it asks beside the code (`[{name, label, choices}]`, empty for most), `userCode` being the device code to type at `url` (null when there is none), `machine` being the hostname a callback sign-in's browser must be on; `paste` defaults to `[login] paste` |
| `POST /api/login/open` `{id}` | Open its link where `[login]` says: `{opened}` |
| `GET`/`POST /api/login/settings` `{browser?, window?, paste?}` | Read or save `[login]`: `{browsers, browser, window, paste, opensIn}` |
| `POST /api/login/finish` `{id, code?, fields?}` | Finish it, with `fields` the answers by name, and store the result |
| `POST /api/login/cancel` `{id}` | Abandon it; a waiting callback (Codex, Claude) stops and frees its port, and a `finish` already under way stores nothing |
| `POST /api/proxy/settings` `{enabled?, port?, lan?}` | Save the proxy's settings and start, move or stop it to match |
| `POST /api/proxy/keys` `{name}` / `keys/remove` `{id}` | Make or remove a proxy access key |
| `POST /api/usage/spend` `{days}` | `{export, logins}`: TokenGauge's `--usage-json` from the first day of the range, and every stored login by the digest it names logins with, then each removed one the ledger names (`removed: true`); or `{unavailable}` saying why there is none ([../features/web-page.md](../features/web-page.md#usage)) |
| `GET /api/versions` | `{tools, checkedAt}`: for remuda and TokenGauge, `installed`, `current` (null for a TokenGauge that does not answer `--version`), `latest`, `available`, `held` (why the page cannot update it), `url`, `releases` (`{version, published, url, notes}`) and `error` when GitHub could not be asked; the answer is kept 15 minutes ([../features/web-page.md](../features/web-page.md)) |
| `POST /api/versions` `{force?}` | The same, asking GitHub again when `force` |
| `POST /api/versions/update` `{tool}` | `{message, restarting}`: update `remuda` or `tokengauge` |
| `POST /api/usage/value` | `{weeks, logins}`: each login's weekly windows, newest first, as `{provider, login, openedAt, endsAt, open, used, usd, usdPerPercent, unattributedUsd, partial, unpriced, unattributedUnpriced}` (times in ms), against the spend of the last 400 days; or `{unavailable}` when TokenGauge does not export or its window history cannot be read ([../features/web-page.md](../features/web-page.md#usage)) |
| `POST /api/usage/requests` `{days}` | `{since, rows}`: the proxy's request log over the range, summed per quarter hour (every time zone splits its days on one), provider, model, login, key name and outcome |

Pending sign-ins live in memory, one per provider; starting another cancels the
previous one. `finish` stores its login only if it is the one that takes the
sign-in out of that map, so a cancel that lands during it wins. `App::api` takes the store lock once for every route except the
`/api/login` ones, which lock only to save (`commands::save_login`), and the
`/api/usage` and `/api/versions` ones, which only read, ask GitHub and run
TokenGauge or an update, so a slow one holds up nothing else; `/api/token`
answers before it too. The name
check before a sign-in and the "stored ..." message are `commands::begin_login`
and `commands::stored_message`, shared with the CLI.

The nudge (`gauge::nudge`) writes `{"cmd":"refresh"}` to `tokengauge.sock`
beside the snapshot, the command TokenGauge's own refresh button sends its
daemon, and waits up to a second for the ack. With no daemon listening it does
nothing. TokenGauge still does the fetching ([decisions](../decisions.md)).

`/api/state` reports each CLI's live login as `signed_out`, `stored` (with
`confirmed`), `unstored`, `foreign` (an API key; the page says only
`remuda use --discard` replaces it) or `unreadable` (with the error).

`proxy` is the proxy's settings and state ([proxy.md](proxy.md)): `enabled`,
`port`, `lan`, `host` (the LAN address with `lan`, null when none was found,
else `127.0.0.1`; the base URLs fall back to `127.0.0.1`), `firewall` (null, or `name`, `open` true, false or
null, and the `commands` that open the port; only while it is on with `lan`,
and only once the background ask has answered),
`running` (the bound port, or null), `error` (why it could not bind,
or why `.proxy.json` could not be read),
`baseUrls` (`openai`, `claude`), `keys` (`id`, `name`, `key`, `created`) and
`routes` (per provider the proxy can spend: `provider`, `models`, `formats`).
`.proxy.json` sits at the store's root, so `App::fingerprint` follows it
already.

`health` is `vitals`, the checks `remuda doctor` makes
([features/doctor.md](../features/doctor.md)), then `tokengauge` (whether the
snapshot exists) and `switchedAt` (provider id to the time of its last
switch, `runs::switches`). A unit's state comes from systemd's user unit
search path in its order (`paths::systemd_user_units`: the config directory,
`/etc`, the runtime directories, the data directory and `$XDG_DATA_DIRS`,
`/usr/local/lib`, `/usr/lib`, `units::state`): `masked` when the first copy of
the unit is a link to `/dev/null`, `enabled` when a link to it under the
target's `.wants` directory that still resolves exists in any of them,
`disabled` when only the unit does, else `absent`. An enabled timer that was
stopped still reads as enabled, and shows as a late last run.

`usage` is `gauge::read` of TokenGauge's snapshot, or null. The snapshot is
where TokenGauge puts it: the top-level `cache_file` of its config
(`$TOKENGAUGE_CONFIG`, else `$XDG_CONFIG_HOME/tokengauge/config.toml`),
unless that is TokenGauge's old temp path, which TokenGauge itself replaces
with the default `$XDG_STATE_HOME/tokengauge/tokengauge-usage.json`. It is
read only when it is a regular file, and at most 4 MiB of it. `usage` holds
`updatedAt` and, per provider id, `stale`, `staleReason`, `askedAt` (the payload's own
`usage.updatedAt`: TokenGauge carries a login it is not signed into between
asks, so it can be older than `updatedAt`), `error` (from the
snapshot's top-level `errors`, where TokenGauge files a fetch that failed with
nothing cached), `credentialState`, `planWeight` (TokenGauge's nominal plan multiplier, when it knows one: any positive number, since a Claude Team seat is 1.25 or 6.25), `plan` (the snapshot's `usage.loginMethod`, the plan as TokenGauge names it, such as Grok's tier, which no credential file holds), `windows` (`title`, `usedPercent`,
`resetsAt`, `trail`), placeholders dropped, a window whose `resetsAt` has passed
shown at 0% with no `resetsAt` (TokenGauge keeps a login's figures while it
cannot fetch, so they can predate a reset; auto mode reads them the same way),
and no `trail`. `trail` is the window's `[ms, percent]`
samples from TokenGauge's window history beside the snapshot
(`value::history_paths`, read in order: `tokengauge-window-fleet.json`, the
fleet's view, while TokenGauge's fleet sync keeps one, else
`tokengauge-window-history.json`; its trail for the same provider, credential
and window: `primary`, `secondary`, `tertiary`, or an extra window's id, else
title), from the window's opening on; null when the file or the trail is
missing, its `resetsAt` has passed, or it is not the window's reset (the
fleet's view can hold another machine's later instance under a login's name;
resets within a twentieth of the window, an hour at most, are one). `past`
is the closed instances of the same window TokenGauge kept the samples of
(its `closed` records with `samples`, eight weeks of them since TokenGauge
0.61), oldest first, each `{openedAt, samples}` with `openedAt` in ms.
The Quota page draws its pools from these trails
([web-page.md](../features/web-page.md#quota)).
`bankedResets` (`available`, `expiresAt`, and `kinds`,
`[{kind, count, expiresAt, clears, idle}]` with `kind` one of `session`,
`weekly`, `full`, `fable`, `clears` the window keys it resets when TokenGauge
names them and `idle` true when every reset of that kind would clear nothing
now, when TokenGauge splits the count: the Claude resets TokenGauge
counts usable, null when none are left), and `accounts`: the same fields per stored
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
`vite-plugin-singlefile` builds it into one self-contained `serve.html`,
scripts and styles inlined, which `serve.rs` compiles in (`include_str!`).
`build.rs` runs that build into Cargo's `OUT_DIR` whenever a file under `web/`
changes, so compiling remuda needs Node, pnpm and a `pnpm install` in `web/`,
and stops with that advice when `web/node_modules` is missing. It runs Vite
with `node`, not `pnpm`: a `pnpm` other than the one `package.json` pins
would download the pinned one, which an offline build cannot. Nothing built
is committed. `pnpm dev` serves the source with hot reload and
passes `/api` to a running `remuda serve` on `$REMUDA_PORT` (7429 by
default), dropping the `Origin` it would refuse; open
`http://localhost:5173/serve.html#<token>`. That is harmless only while Vite
binds localhost: `pnpm dev --host` would let another machine through the
Origin check, leaving the token as the only guard. The page makes no external
requests and has a light and a dark theme and two looks; its fonts
(Archivo and Schibsted Grotesk, Latin subsets, from `@fontsource-variable`)
and the provider marks and colours (`web/src/icons.ts`) are inlined too.
`icons.ts` is `selvedge::brands` written out, the table TokenGauge draws from:
`tests/brands.rs` fails when it differs from the pinned selvedge, and
`REMUDA_BLESS=1 cargo test --test brands` rewrites it. A mark is tinted in its
`mark` colour and accents use `color` (`brand.ts`); either one missing is the
page's ink, so Grok, Z.ai and opencode are drawn in ink, and Kimi's K too,
beside the blue dot its mark carries. Where the marks and the fonts come from,
and their licences, is in `THIRD-PARTY.md`. It reads `/api/events` through `fetch` (an `EventSource`
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
them: Solid keeps the rows' buttons across a redraw, so a clicked one keeps
the focus.

## Sources

- [src/serve.rs](../../src/serve.rs)
- [src/served.rs](../../src/served.rs)
- [src/versions.rs](../../src/versions.rs)
- [src/http.rs](../../src/http.rs)
- [web/](../../web/), built by [build.rs](../../build.rs)
- [src/main.rs](../../src/main.rs) `Serve`, `open`
- [systemd/remuda-serve.service](../../systemd/remuda-serve.service)
