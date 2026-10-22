# The local page

`remuda serve` opens a page with a sidebar of places and one page for each,
picked by the address's hash (`#/`, `#/cli/<id>`, `#/cli/<id>/automation`,
`#/proxy`, `#/status`; `route.ts`), so the browser's back button and a
bookmark work. The token, which also travels in the hash, is read only from
a hash that does not start with `#/`.

The sidebar lists, top to bottom: Overview; one link per CLI in use, with
its icon, the login in use (or "signed out"), the share used of its tile's
window over a small meter, and an "auto" tag when auto mode is on; the API
proxy, with an on/off dot; Status, with a dot that turns amber when the
refresh timer is off or its last run is over 75 minutes old and red when it
reported problems (`healthTone`). Its foot holds Add a login, Refresh tokens,
the theme switch and the look switch. On a narrow screen it becomes a bar
along the top, with Add a login first; Refresh tokens and the two switches
move to the foot of the Status page. Each CLI is shown by its own logo, in its own colour, both shared with
TokenGauge through selvedge (`icons.ts`), in the sidebar, the page heads, the overview, the proxy and
Add a login.

- **Overview** has one tile per CLI, headed by its logo, with a combined meter: one usage window
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
  the pick is kept per CLI in the browser's `localStorage`. Each tile links
  to its CLI's page.
- A CLI with no stored login whose own login is signed out gets no tile,
  link or page (`usage.ts` `inUse`); Add a login still offers it. One signed
  into a login remuda has not stored stays, to import it. With no CLI in use
  at all, every one is shown.
- **A CLI's page** opens with its logo, the login in use and how many are
  stored, and, on the Logins tab, an "Auto mode on/off" badge that leads to
  its Automation tab.
  It has two tabs. Logins shows the CLI's tile, then its logins as a list
  that can be ordered as stored, by most headroom or by name (kept in
  `localStorage`). Automation holds the CLI's auto mode and keep-warm
  settings (below); its tab says "unsaved" while edits wait for Save.
- **One row per stored login**: a state dot, the name, label and email,
  the plan and state pills (the plan is TokenGauge's when the credential
  file does not say, as for Grok; "Kept warm" when keep-warm picks it), one
  small meter per usage window, how long its access token has left, Use (or
  "In use" on the one in use, which also has a green edge), and a chevron
  that opens its details in place: a row per usage window (its title,
  percentage and reset over a bar, then "Figures from N minutes ago" when
  TokenGauge carried them from an earlier ask than its snapshot, `usage.ts`
  `carriedSince`), a Claude login's banked usage-limit resets and when they
  expire, with a Reset limits button that asks for a second click before it
  spends one and then holds until TokenGauge reports the new count, how long
  its access token and its sign-in (refresh token) have left, its
  keep-warm line, and Refresh, Label, Rename and Remove. Remove asks
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
  An open editor, and what was typed in it, survives the list being
  redrawn after another action.
- **API proxy** ([proxy.md](proxy.md)): an on/off switch and whether it
  listens, an OpenAI / Claude switch that shows that protocol's base URL with
  a copy button, the access key masked with show and copy buttons (a picker
  when there are several), the two shell `export` lines that protocol's
  clients read (`OPENAI_BASE_URL` and `OPENAI_API_KEY`, or
  `ANTHROPIC_BASE_URL` and `ANTHROPIC_API_KEY`) with one copy button, the
  key masked until shown but always copied whole, Manage keys to make and remove keys (a second
  click removes) and change the port, and the models it routes per CLI with
  the formats its endpoint speaks. The protocol last picked is remembered.
- **Status**: whether the refresh timer is enabled, when it last ran, how
  many credentials it refreshed and, behind a toggle, what it skipped or
  failed on (from `<store>/.last-refresh.json`); whether TokenGauge's
  snapshot was found and how old it is; and the store's path.

The theme follows the system until the Auto / Light / Dark switch overrides
it. The page has two looks, picked under the theme switch: Sage, green on a
pale ground with condensed Archivo figures, and Instrument, a navy sidebar
beside white panels with blue meters, amber buttons and Schibsted Grotesk.
Both choices are kept in `localStorage` (`remuda-theme`, `remuda-skin`) and
applied in `<head>`, before the page draws.

When a CLI's live login is not stored, an Import box sits above its logins.
Enter in any of the page's inputs confirms it: the Import name, the pasted
sign-in code, the Add a login name and the in-place editors.
Import, and Sign in under Add a login, ask "Overwrite <name> (<email>)?"
when the name is taken, and replace it with `force` on the second click, as
`--force` does. Every confirmation holds only for the provider and name it
named (editing the name asks again), lasts 4 seconds, and ignores a second
click within 400 ms, so a double-click only arms it.
"Refresh tokens" refreshes every inactive credential due within the hour.

Add a login opens a dialog that picks a CLI by its logo, preselecting the
page's, and a name, and starts a sign-in: the
server opens the provider's page in a private window, as `login` does (see
[sign-in.md](sign-in.md)). When it cannot, or runs with `--no-browser`, the page
opens a new tab instead. Either way the box can copy the link for pasting into
a private window by hand, which also serves a page viewed through a tunnel from
another machine. Then Claude shows a box for the pasted code
while Codex waits for its callback. Cancel, or closing the dialog, abandons
it, and for Codex frees port 1455 immediately.

Usage figures come from TokenGauge's snapshot, which remuda reads and never
writes. A refresh or a switch made from the page asks TokenGauge's daemon to
fetch at once, so a login it skipped as expired shows its figures within
seconds rather than at TokenGauge's next poll. Since TokenGauge 0.37 (its ADR 0003), its payloads and
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

## Auto mode

While auto mode is on, the Automation tab opens with what its last round decided
([auto-mode.md](auto-mode.md)): the rule in force, every login weighed with
its score and why it was or was not picked, the decision and why, and the last
switch auto mode made.

The Automation tab edits `config.toml`
([architecture/config.md](../architecture/config.md)): a card with auto
mode's switch, the minimum time on a login, the margin and the
ceiling, each login's own ceiling, the fallback, the
logins it may pick when no rule is in force, and its rules, each with a
schedule, its tiers and which logins it keeps for a schedule; a card of the
logins kept warm ([keep-warm.md](keep-warm.md)), apart from auto mode since it
runs whether auto mode is on or off; and a card of schedules, whose days and
hours are shared by every CLI. A schedule a rule
uses cannot be removed, and renaming one renames it in every rule.

Edits stay on the page until Save, and are checked as they are made
(`/api/config/check`): what auto mode could not act on shows under the card
or rule it concerns, and Save waits until there is none. A page with no edits
follows the file when it is changed elsewhere; one with edits says the file
changed and offers to load it, dropping them, since saving over it is
refused.


## Sources

- [web/src/](../../web/src/), built into [src/serve.html](../../src/serve.html);
  the settings in [AutoMode.tsx](../../web/src/components/AutoMode.tsx) and
  [config.ts](../../web/src/config.ts)
- [src/serve.rs](../../src/serve.rs)
- [src/gauge.rs](../../src/gauge.rs)
- [src/runs.rs](../../src/runs.rs)
