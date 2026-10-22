# Token refresh

Access tokens live hours (Claude) to days (Codex). A stored credential nobody
refreshes goes stale, and Claude's refresh token eventually expires too, so
anything reading the store (TokenGauge) needs the inactive ones kept fresh.

`remuda refresh [name] [--force] [--within MIN]` refreshes every inactive
credential whose access token expires within `MIN` minutes (default 60), or
has no known expiry. `--force` refreshes regardless; a name narrows it to one.
The active credential is always skipped: the CLI owns it (a named active one
says so). So is a login a `remuda run` session holds, whose CLI owns it in
turn ([run.md](run.md)). An API key (GLM, opencode Go) never expires and is never refreshed. A CLI whose live login cannot be read is skipped whole, since which of
its credentials is active is unknown. A refresh that answers for a different
account than the sidecar's is refused and not saved, and the tokens it rotated
are kept in a `.set-aside-...` file named in the error. A refused refresh token
says to sign that credential in again. Failures are listed per credential and
the command exits non-zero.

A credential whose sidecar does not match it (`[unverified]` in `ls`) is not
refreshed until it has been identified again.

`remuda-refresh.timer`, installed by `install.sh`, runs `remuda refresh
--scheduled` every 30 minutes. `--scheduled`, or any run as a remuda unit, first
checks each CLI's directory against the store's record of where the last
interactive command found it, and skips a CLI where they differ or there is
no record; naming a credential of a skipped CLI says so (see
[distribution.md](../architecture/distribution.md)).

`commands::refresh` prints each line as it goes and also fills a `Report`:
the credentials it refreshed, and a line per problem. When any credential
failed it returns `commands::Failed`. A scheduled run then writes
`<store>/.last-refresh.json` (`runs::record`, built by `runs::Run::of`): when
it ran, what it refreshed, and as problems the CLIs it skipped, the report's
lines and any other error that ended the run (an unreadable store, a CLI that
could not be synced), but not `Failed` itself, which only counts them. The
page's health line reads it. Only a run with no store at all records nothing.
Interactive runs record nothing.

When the report lists anything refreshed, the command asks TokenGauge's daemon
to fetch at once (`gauge::nudge`), so a login it skipped as expired gets its
figures back without waiting for its next poll. A run that refreshed nothing,
such as most timer runs, leaves TokenGauge alone. The page's refresh always
asks, since someone is looking at it.

## Sources

- [src/commands.rs](../../src/commands.rs) `refresh`
- [src/ops.rs](../../src/ops.rs) `refresh_entry`
- [src/runs.rs](../../src/runs.rs)
- [systemd/remuda-refresh.timer](../../systemd/remuda-refresh.timer)
