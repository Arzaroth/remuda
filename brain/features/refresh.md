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

`remuda-refresh.timer`, installed by `install.sh`, runs it every 30 minutes,
with the directories the installing shell had set (see
[distribution.md](../architecture/distribution.md)).

## Sources

- [src/commands.rs](../../src/commands.rs) `refresh`
- [src/ops.rs](../../src/ops.rs) `refresh_entry`
- [systemd/remuda-refresh.timer](../../systemd/remuda-refresh.timer)
