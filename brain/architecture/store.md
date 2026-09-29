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
(ms), optional `label`, `credsDigest`, and for Claude `oauthAccount`, the block restored into
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
0700.

A credential and its sidecar are two renames, so the sidecar records
`credsDigest`: the SHA-256 of the credential file's exact bytes, as 64
lowercase hex characters. The credential is written first, so a crash in
between leaves a sidecar whose digest does not match. The one exception is a
sidecar with no digest (written by 0.1.0), which would pass for anything: it
is replaced first. `Store::get` marks a mismatched entry unverified, and
`ops::heal` runs at the start of every sync, the CLI signed in or not: it asks
the provider whose tokens the entry holds, refreshing them first if the access
token has expired (the rotated tokens are saved either way), and rewrites the
sidecar. It never refreshes an entry holding the CLI's live login, which would
sign the CLI out. Until an entry is identified it is not refreshed, switched
to, labelled or matched by account, `ls` marks it `[unverified]` and the page
disables it; if it holds the live login, that login is reported as the
unconfirmed active one, so nothing overwrites it without `--discard`. A digest
missing from a sidecar is trusted. `rename` moves the sidecar first and undoes
that if the credential cannot follow.

`<store>/.dirs.json` records where the last interactive command found each
CLI's login; see [distribution.md](distribution.md). `<store>/.last-refresh.json`
records the last scheduled refresh for the page; see
[../features/refresh.md](../features/refresh.md). `<store>/.last-switch.json`
records when each provider was last switched; see
[../features/switching.md](../features/switching.md). None is under a
provider directory, and all are dot-files, so none is a credential to
TokenGauge. They are written through `fsx::write_record`, best effort.

`Store::lock` takes an exclusive `flock` on `<store>/.lock`, released when the
process exits, so a crashed command never leaves the store locked.

## The TokenGauge contract

TokenGauge's ADR 0003 reads this store to draw one meter per credential and
never writes it. What it relies on: the paths above, the file shapes, and the
sidecar keys `accountId`, `email`, `label` and `credsDigest` (how it is
computed included). A change to any of those is a
change to that ADR as well.

## Sources

- [src/store.rs](../../src/store.rs)
- [src/fsx.rs](../../src/fsx.rs)
- [src/paths.rs](../../src/paths.rs)
