# Switching

`remuda use <name> [--discard]` makes a stored credential the one its CLI uses
(`ops::switch`):

1. Sync the live login back (`ops::sync_live`).
2. If the target is already active, say so and stop.
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

Running Claude Code sessions watch their credential files and adopt the new
login. A running Codex session may keep the login it started with until it is
restarted.

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
- [src/claude.rs](../../src/claude.rs) `install`
- [src/codex.rs](../../src/codex.rs) `install`
- [src/runs.rs](../../src/runs.rs) `switched`
