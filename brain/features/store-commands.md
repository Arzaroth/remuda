# Import, list and remove

## `remuda import <name> [-p <cli>] [--force]`

With `login`, the only command that creates a missing store; every other one
refuses it, naming what it was doing.

Stores the login the CLI is signed into now, under the account the provider
says the tokens belong to (`Provider::identify`), keeping the CLI's own account
block when it agrees. Offline, it trusts the CLI's files only while the store
holds nothing for that CLI, and otherwise refuses. Also refuses when that login
is already stored (under any name), when the name is taken (unless `--force`),
when the account is already stored under another name (even with `--force`),
and when the CLI is signed out.

## `remuda`

With no command, remuda prints a table of every stored login (`commands::status`):
its CLI, its name, `●` on the one the CLI uses (or `run` when a `remuda run`
session holds it), and "kept fresh", how long ago its credential file in the
store was last written. Its tokens changing rewrites it, by a refresh or by
the CLI's own rotation synced back, so the column follows the login in use
too; so does any other save of the entry, such as a label. A CLI whose login
cannot be read marks its rows `?`, and so does a login it only looks signed
into, with a line pointing at `ls`. The live logins are synced first, as `ls`
does. A
terminal gets the header and the age dimmed and the `●` green, unless
`NO_COLOR` is set; piped, it is plain text. Without a store it prints how to
start one and exits 0. It used to print the help (`remuda --help` still does).

## `remuda ls [--json]`

Syncs every provider's live login back, then prints a block per provider that
has credentials or a live login: name (with its label), email, plan, when the
access token and (Claude) the refresh token expire, `*` on the active one, and a
line when the live login is signed out, unstored or unconfirmed. With nothing
at all it prints the store path.

`--json` prints `{store, credentials: [...], live: [...]}`. Each credential has
`provider`, `name`, `label`, `email`, `accountId`, `plan`, `active`,
`expiresAt`, `refreshTokenExpiresAt`, `verified`, `team` and `seat`
([team-seats.md](team-seats.md)); each live entry has `provider` and a
`state`: `signed_out`, `stored` (with `name` and `confirmed`), `unstored` (with
`email`), `foreign` (with `what`, e.g. "an API key") or `unreadable` (with
`error`). A CLI whose files cannot be read is reported and the others are still
listed. The page reads the same JSON.

## `remuda env <name>`

Prints `export VAR='key'` for a stored API key, for a shell to `eval`: GLM
reads its key from the environment only, and opencode's variable wins over the
key it stores. Any other provider refuses.

## `remuda rm <name>`

Deletes the credential and its sidecar, and takes it out of `config.toml`.
Refuses the active credential: switch away first. Refuses one a `remuda run`
session holds as well ([run.md](run.md)).

## Sources

- [src/commands.rs](../../src/commands.rs) `status`, `import`, `list`, `list_json`, `print_env`, `remove`
- [src/main.rs](../../src/main.rs)
