# Switching

`remuda use <name> [--discard]` makes a stored credential the one its CLI uses
(`ops::switch`):

1. Sync the live login back (`ops::sync_live`).
2. If the target is already active, say so and stop.
3. If the live login is unstored, or matched only by an account id that could
   not be confirmed, refuse: switching would lose its newest tokens. `--discard`
   switches anyway.
4. If the target's access token expires within 5 minutes, refresh it first and
   save the result.
5. `Provider::install`: for Claude, write `oauthAccount` into `.claude.json`,
   then `claudeAiOauth` into `.credentials.json`, keeping every other key; for
   Codex, replace `auth.json`.

Running Claude Code sessions watch their credential files and adopt the new
login. A running Codex session may keep the login it started with until it is
restarted.

## Matching the live login

`sync_live` finds the credential the live login belongs to by refresh token,
then access token, then account id. An account-id match is trusted only after
`Provider::confirm` agrees; a confirmed match copies the live tokens (and for
Claude the `oauthAccount` block and email) into the credential. See
[decisions.md](../decisions.md).

## Sources

- [src/ops.rs](../../src/ops.rs)
- [src/claude.rs](../../src/claude.rs) `install`
- [src/codex.rs](../../src/codex.rs) `install`
