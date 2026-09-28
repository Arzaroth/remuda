# Import, list and remove

## `remuda import <name> [-p codex] [--force]`

Stores the login the CLI is signed into now. Refuses when that login is already
stored (under any name), when the name is taken (unless `--force`), when the
account is already stored under another name (even with `--force`), and when
the CLI is signed out or its files do not say whose login it is.

## `remuda ls [--json]`

Syncs every provider's live login back, then prints a block per provider that
has credentials or a live login: name (with its label), email, plan, when the
access token and (Claude) the refresh token expire, `*` on the active one, and a
line when the live login is signed out, unstored or unconfirmed. With nothing
at all it prints the store path.

`--json` prints `{store, credentials: [...], live: [...]}`. Each credential has
`provider`, `name`, `label`, `email`, `accountId`, `plan`, `active`,
`expiresAt`, `refreshTokenExpiresAt`; each live entry has `provider`, `state`
(`signed_out`, `stored`, `unstored`) and `name`/`confirmed` or `email`. The page
reads the same JSON.

## `remuda rm <name>`

Deletes the credential and its sidecar. Refuses the active credential: switch
away first.

## Sources

- [src/commands.rs](../../src/commands.rs) `import`, `list`, `list_json`, `remove`
- [src/main.rs](../../src/main.rs)
