# The local page

`remuda serve` opens a page in three parts:

- A health line: whether the refresh timer is enabled, when it last ran, how
  many credentials it refreshed and, behind a toggle that opens a list under
  the line, what it skipped or failed on (from `<store>/.last-refresh.json`);
  whether TokenGauge's snapshot was found and how old it is; and the store's
  path.
- A strip with one tile per CLI: the login in use and one of its usage
  windows, as a large percentage with a meter and its reset. It shows the
  tightest window until a chip under it picks another (5 hours, Weekly, a
  model-scoped limit, ...); the pick is kept per CLI in the browser's
  `localStorage`.
- One row per stored credential: name, label, email, plan and state pills,
  the usage windows, how long its access token and its sign-in (refresh
  token) have left, and Use, Refresh, Label, Rename and Remove. Remove asks
  for a second click; Use, Refresh and Remove are disabled on the active one.
  Use asks for a second click too when switching would drop the CLI's live
  login, naming what goes: "Drop alice@example.com unsaved?" for one not
  stored, "Drop the API key?", or "Lose work's newest tokens?" for one that
  could not be confirmed. It then switches with `discard`, as
  `remuda use --discard`.
  Label and Rename turn the label or the name into an input in its place,
  filled with the current value and sized to the text so nothing moves: the
  check button beside it or Enter saves, and the same button or Escape
  cancels, asking for a second click ("Discard?") when the value was changed.
  An open editor, and what was typed in it, survives the rows being redrawn
  after another action.

The theme follows the system until the Auto / Light / Dark switch in the
header overrides it; the choice is kept in `localStorage` and applied in
`<head>`, before the page draws.

When a CLI's live login is not stored, its section offers an Import box.
Enter in any of the page's inputs confirms it: the Import name, the pasted
sign-in code, the Add an account name and the in-place editors.
Import, and Sign in under "Add an account", ask "Overwrite <name> (<email>)?"
when the name is taken, and replace it with `force` on the second click, as
`--force` does. Every confirmation holds only for the provider and name it
named (editing the name asks again), lasts 4 seconds, and ignores a second
click within 400 ms, so a double-click only arms it.
"Refresh tokens" refreshes every inactive credential due within the hour.

"Add an account" picks a provider and a name and starts a sign-in: the
server opens the provider's page in a private window, as `login` does (see
[sign-in.md](sign-in.md)). When it cannot, or runs with `--no-browser`, the page
opens a new tab instead. Either way the box can copy the link for pasting into
a private window by hand, which also serves a page viewed through a tunnel from
another machine. Then Claude shows a box for the pasted code
while Codex waits for its callback. Cancel abandons it, and for Codex frees port
1455 immediately.

Usage figures come from TokenGauge's snapshot, which remuda reads and never
writes or refreshes. Since TokenGauge 0.37 (its ADR 0003), its payloads and
errors name the stored credential they belong to, so each row shows its own
login's figures, or its own error, and the tile the active one's. A login
TokenGauge skipped because its access token expired or it is unverified says
so. An older snapshot names no credential and holds the
login TokenGauge last saw for each CLI: then only the active credential shows
usage, the others say so, and until TokenGauge has fetched again after a
switch the new login says it is waiting rather than showing the previous
account's figures. A stale snapshot shows TokenGauge's reason, and a failed
fetch its error. Without TokenGauge, the page points to it and shows
everything else.
How it is guarded and the API it calls are in
[architecture/web.md](../architecture/web.md).

## Sources

- [src/serve.html](../../src/serve.html)
- [src/serve.rs](../../src/serve.rs)
- [src/gauge.rs](../../src/gauge.rs)
- [src/runs.rs](../../src/runs.rs)
