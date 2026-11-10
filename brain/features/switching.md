# Switching

`remuda use <name> [--discard]` makes a stored credential the one its CLI uses
(`ops::switch_by`, with who asked; auto mode's `ops::switch_when` also says why):

1. Sync the live login back (`ops::sync_live`).
2. If the target is already active, say so and stop. A target a `remuda run`
   session holds is refused, since its tokens would then be in two places
   ([run.md](run.md)).
3. If the live login is unstored, matched only by an account id that could not
   be confirmed, or one remuda cannot store (a Codex API key), refuse:
   switching would lose it. `--discard` switches anyway.
4. If the target's access token expires within 5 minutes, refresh it first and
   save the result, then sync the live login again: the CLI may have rotated
   its own token during that round trip.
5. Take `Provider::lock_live` (Codex: `auth.json.lock`, which TokenGauge
   refreshes under), read the live login once more and `Provider::install`
   against it: if the CLI rotated its login since, nothing is written, the new
   tokens are synced back into the store and the install is tried again (up to
   three times). `--discard` skips that check, except when the live login is
   stored and confirmed: then there is nothing to drop, and the check stays. For Claude the install writes
   `oauthAccount` into `.claude.json`, then `claudeAiOauth` into
   `.credentials.json`, keeping every other key; for Codex it replaces
   `auth.json`.
6. Note the time in `<store>/.last-switch.json` under the provider id
   (`runs::switched`, best effort), so the page can tell usage TokenGauge
   fetched before the switch from usage of the new login.
7. Log the switch with who made it, the CLI, the page or auto mode, and, for
   auto mode, why ([log.md](log.md)).

Once switched, `use` and the page ask TokenGauge's daemon to fetch at once
(`gauge::nudge`, see [web.md](../architecture/web.md)) rather than at its
next poll.

Running Claude Code sessions watch their credential files and adopt the new
login. A running Codex session may keep the login it started with until it is
restarted.

## Signing out

`remuda signout <cli> [--discard]` and the page's Sign out, on the login in
use, leave the CLI's own files with no login (`ops::sign_out`); a `remuda run`
session keeps the one it started on, and Grok an API key. Every login is then
the store's alone, so remuda refreshes it and signs it in again, even when it
is the only one; the page offers Sign in again only on a login the CLI is not
using. `remuda use` puts one back.

It syncs the live login back first and refuses as a switch does (unstored,
unconfirmed, `--discard` drops it). A login remuda cannot store, such as an API
key, is never signed out. Under `Provider::lock_live` it syncs again and
calls `Provider::sign_out` against the very login that sync read, so what
leaves the CLI is what the store holds, retrying up to three times when the CLI
rotated it meanwhile:

- **Claude Code**: removes `claudeAiOauth` from `.credentials.json`, then
  `oauthAccount` from `.claude.json`, keeping every other key (`mcpOAuth`).
- **Codex, Kimi, Cursor**: removes the file that is the login (`auth.json`,
  `credentials/kimi-code.json`), as their own logout does. The file is moved
  aside before it is compared, so a rotation that lands meanwhile is put back
  rather than removed unsynced. A file that is a link is refused rather than
  removed (`fsx::remove_json_if`).
- **Grok**: removes the `auth.x.ai::` entries from `auth.json`. An API key the
  file holds stays, so Grok keeps using it and remuda reports it as a login it
  cannot store.
- **GLM, opencode Go**: refused; they have no `signs_out`, so the page shows
  no Sign out for them.

What that last sync read is what it reports: a login gone meanwhile is already
signed out, and the message names the login it took out. It notes the time in
`.last-switch.json`, writes a ledger line with no account,
and logs a `switch` event. With auto mode on for that CLI, the message says
auto mode will put a login back: the next round picks one unless it holds,
as when TokenGauge has no figures, every login it may pick is full with no
fallback, or a session of the CLI has to end first, so the CLI can stay
signed out for longer.

## Whose spend it is

A transcript names no login, so remuda writes down which one each CLI's files
hold, in `<store>/.attribution.jsonl`, and TokenGauge gives each billed
request the login live when it was billed. A switch writes a `live` line for
the login it installed. `sync_live` writes one when the files hold another
login than the ledger's last line says, which means something outside remuda
changed them (`claude /login`); that line is dated by the credential file's
mtime, the latest the change can have happened, since remuda only notices on
its next read. A live login remuda does not store, or none, is a line with no
account: spend from then on is nobody's it knows. Nothing is written before a
CLI's first login, so spend from before the ledger stays unattributed. Each
switch drops lines older than 400 days, keeping the one still in force. Code:
`attribution::{switched, noticed}`.

## Matching the live login

`sync_live` finds the credential the live login belongs to by refresh token,
then access token. Failing that, it asks `Provider::identify` whose tokens they
are and files them under the credential of that account, whichever account the
CLI's own files name. Offline, it falls back to the files' account id but copies
nothing and reports the match as unconfirmed. A live copy whose access token
expires before the stored one's is older and never overwrites it. A matched
login also carries its `oauthAccount` block and email back (Claude). See
[decisions.md](../decisions.md).

## Sources

- [src/ops.rs](../../src/ops.rs)
- [src/claude.rs](../../src/claude.rs) `install`, `sign_out`
- [src/codex.rs](../../src/codex.rs) `install`, `sign_out`
- [src/fsx.rs](../../src/fsx.rs) `remove_json_if`
- [src/runs.rs](../../src/runs.rs) `switched`
- [src/attribution.rs](../../src/attribution.rs) `switched`, `noticed`
- [src/logbook.rs](../../src/logbook.rs), each switch logged with who made it ([log.md](log.md))
