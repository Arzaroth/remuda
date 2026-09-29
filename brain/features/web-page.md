# The local page

`remuda serve` opens a page in three parts:

- A health line: whether the refresh timer is enabled, when it last ran, how
  many credentials it refreshed and what it skipped or failed on (from
  `<store>/.last-refresh.json`); whether TokenGauge's snapshot was found and
  how old it is; and the store's path.
- A strip with one tile per CLI: the login in use and its tightest usage
  window, as a large percentage with a meter and its reset.
- One row per stored credential: name, label, email, plan and state pills,
  the usage windows, how long its access token and its sign-in (refresh
  token) have left, and Use, Refresh, Label, Rename and Remove. Remove asks
  for a second click; Use, Refresh and Remove are disabled on the active one.

When a CLI's live login is not stored, its section offers an Import box.
"Refresh tokens" refreshes every inactive credential due within the hour.

"Add an account" picks a provider and a name and starts a sign-in: the
provider's page opens in a new tab, then Claude shows a box for the pasted code
while Codex waits for its callback. Cancel abandons it, and for Codex frees port
1455 immediately.

Usage figures come from TokenGauge's snapshot, which remuda reads and never
writes or refreshes. The snapshot holds the login TokenGauge last saw for each
CLI, so only the active credential shows usage; the others say so. Once
TokenGauge's ADR 0003 has payloads name their credential, each row can show
its own. Without TokenGauge, the page points to it and shows everything else.
How it is guarded and the API it calls are in
[architecture/web.md](../architecture/web.md).

## Sources

- [src/serve.html](../../src/serve.html)
- [src/serve.rs](../../src/serve.rs)
- [src/gauge.rs](../../src/gauge.rs)
- [src/runs.rs](../../src/runs.rs)
