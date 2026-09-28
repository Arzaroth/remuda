# The store

## Location

`$REMUDA_STORE`, else `$XDG_DATA_HOME/remuda/credentials`, else
`~/.local/share/remuda/credentials` ([paths.rs](../../src/paths.rs)
`store_root`).

## Layout

```
<store>/.lock
<store>/claude/<name>.json        {"claudeAiOauth": {...}}, the login only
<store>/claude/<name>.meta.json
<store>/codex/<name>.json         auth.json, whole
<store>/codex/<name>.meta.json
<store>/<cli>/.set-aside-<account>-<ms>.json   tokens a refresh rotated for another account
```

A credential file has the shape of the CLI's own file, so a reader that parses
the CLI's file parses the stored one unchanged. Claude's holds only
`claudeAiOauth`: `mcpOAuth` stays in the live file, shared.

The sidecar (`store::Meta`, camelCase): `accountId`, `email`, `capturedAt`
(ms), optional `label`, and for Claude `oauthAccount`, the block restored into
`.claude.json` on a switch. `accountId` is Claude's `accountUuid`, and for
Codex the seat (`chatgpt_account_user_id`), not the workspace id in
`auth.json`.

Files starting with `.` are never credentials. A credential that cannot be
read (no sidecar, bad JSON) is skipped with a warning rather than failing the
listing. When a refresh answers for another account than the sidecar's, the
server has already rotated the token, so the result is kept in a
`.set-aside-...` file and named in the error rather than dropped.

## Names

Letters, digits, `-`, `_`, `.`; not starting with `.`, not ending in `.meta`
(`store::validate_name`), so a name can neither escape the directory nor shadow
a sidecar.

## Writes and the lock

Every write goes through `fsx::write_private`: a sibling temp file, fsync,
rename, mode 0600 (or the target's existing mode). A symlinked target is
written where the link points, so the link survives. Directories are created
0700. A credential and its sidecar are two renames, sidecar first; `rename`
undoes the first if the second fails.

`Store::lock` takes an exclusive `flock` on `<store>/.lock`, released when the
process exits, so a crashed command never leaves the store locked.

## The TokenGauge contract

TokenGauge's ADR 0003 reads this store to draw one meter per credential and
never writes it. What it relies on: the paths above, the file shapes, and the
sidecar keys `accountId`, `email` and `label`. A change to any of those is a
change to that ADR as well.

## Sources

- [src/store.rs](../../src/store.rs)
- [src/fsx.rs](../../src/fsx.rs)
- [src/paths.rs](../../src/paths.rs)
