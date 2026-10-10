# The local page

`remuda serve` opens a page in four parts:

- A health line: whether the refresh timer is enabled, when it last ran, how
  many credentials it refreshed and, behind a toggle that opens a list under
  the line, what it skipped or failed on (from `<store>/.last-refresh.json`);
  whether TokenGauge's snapshot was found and how old it is; and the store's
  path.
- A strip with one tile per CLI, edged in the CLI's colour (TokenGauge's:
  Claude `#de7356`, Codex `#74aa9c`), and a combined meter: one usage window
  added up over every stored login TokenGauge has figures for, over a bar
  with a segment per login in its own tone, the soonest of their resets,
  and the login in use. A login with no figures for the window (skipped,
  failed, or a CLI's live login that is not stored) is left out.
  Weighted, the default wherever TokenGauge reports a `planWeight`, adds
  the logins up as TokenGauge's combined header does: each counts by its
  plan's nominal multiplier, in units of the largest plan, so a Max 20x at
  31% and a Pro at 100% make "36% of 105%"; each segment is as wide as its
  weight, but never under a tenth of the bar, so a few small plans beside a
  large one stay readable (TokenGauge's split bar does the same), and a
  login with no known weight is left out and named. A window no login with
  a weight reports is counted once per login instead. The figures are
  rounded half to even, as TokenGauge prints them: the server adds them up
  with `selvedge::plans`, TokenGauge's own code, and sends both sums
  (`shown` in the state), so the switch needs no round trip.
  Absolute counts every login once ("131% of 200%"). The switch shows only
  where there are weights, and the choice is kept per CLI in
  `localStorage`. The tile shows the window with the highest share
  used until Show, beside one of the other windows listed under it with
  their totals, picks another (5 hours, Weekly, a model-scoped limit, ...);
  the pick is kept per CLI in the browser's `localStorage`.
- Tabs: All, then one per CLI, each with its number of stored logins. A tab
  shows only that CLI's cards and picks it under "Add an account"; the
  choice is kept in `localStorage`.
- A CLI with no stored login whose own login is signed out gets no summary
  tile, tab or section (`usage.ts` `inUse`); "Add an account" still offers
  it. One signed into a login remuda has not stored stays, to import it. With
  no CLI in use at all, every one is shown.
- One card per stored credential, in a grid under its CLI: name, label,
  email, plan and state pills, a row per usage window (its title, percentage
  and reset over a bar), how long its access token and its sign-in (refresh
  token) have left, and Use, Refresh, Label, Rename and Remove along the
  bottom. The card in use has a green edge. Remove asks
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
  An open editor, and what was typed in it, survives the cards being
  redrawn after another action.

The theme follows the system until the Auto / Light / Dark switch in the
header overrides it; the choice is kept in `localStorage` and applied in
`<head>`, before the page draws.

When a CLI's live login is not stored, an Import box sits above its cards.
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
The page follows the store and TokenGauge's snapshot: a switch, import,
label or refresh made from the CLI or another page, a run of the refresh
timer, or a new TokenGauge fetch shows within a couple of seconds, without a
reload, in every open tab. A change to a CLI's own login, such as signing in
from the CLI, or enabling the refresh timer shows within a minute.
`remuda open` opens the page a running `serve` shows, and
`install.sh --serve` keeps one running as a user service. How the page is
guarded, the API it calls and the service are in
[architecture/web.md](../architecture/web.md).

## Sources

- [web/src/](../../web/src/), built into [src/serve.html](../../src/serve.html)
- [src/serve.rs](../../src/serve.rs)
- [src/gauge.rs](../../src/gauge.rs)
- [src/runs.rs](../../src/runs.rs)
