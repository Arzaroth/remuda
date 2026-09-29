# Token refresh

Access tokens live hours (Claude) to days (Codex). A stored credential nobody
refreshes goes stale, and Claude's refresh token eventually expires too, so
anything reading the store (TokenGauge) needs the inactive ones kept fresh.

`remuda refresh [name] [--force] [--within MIN]` refreshes every inactive
credential whose access token expires within `MIN` minutes (default 60), or
has no known expiry. `--force` refreshes regardless; a name narrows it to one.
The active credential is always skipped: the CLI owns it (a named active one
says so). A CLI whose live login cannot be read is skipped whole, since which of
its credentials is active is unknown. A refresh that answers for a different
account than the sidecar's is refused and not saved, and the tokens it rotated
are kept in a `.set-aside-...` file named in the error. A refused refresh token
says to sign that credential in again. Failures are listed per credential and
the command exits non-zero.

A credential whose sidecar does not match it (`[unverified]` in `ls`) is not
refreshed until it has been identified again.

`remuda-refresh.timer`, installed by `install.sh`, runs `remuda refresh
--scheduled` every 30 minutes. `--scheduled`, or any run under systemd, first
checks each CLI's directory against the store's record of where the last
interactive command found it, and skips a CLI where they differ or there is
no record; naming a credential of a skipped CLI says so (see
[distribution.md](../architecture/distribution.md)).

A scheduled run then writes `<store>/.last-refresh.json` (`runs::record`):
when it ran, a line per credential it refreshed, and a line per CLI it skipped
or credential that failed. The page's health line reads it. A run that stops
before refreshing (no store, a CLI that cannot be synced) leaves the previous
record in place, so an old "last ran" is itself the sign. Interactive runs
record nothing.

## Sources

- [src/commands.rs](../../src/commands.rs) `refresh`
- [src/ops.rs](../../src/ops.rs) `refresh_entry`
- [src/runs.rs](../../src/runs.rs)
- [systemd/remuda-refresh.timer](../../systemd/remuda-refresh.timer)
