# The local page

`remuda serve` opens a page with a sidebar of places and one page for each,
picked by the address's hash (`#/`, `#/cli/<id>`, `#/cli/<id>/automation`,
`#/usage`, `#/usage/requests`, `#/proxy`, `#/status`; `route.ts`), so the
browser's back button and a
bookmark work. The token, which also travels in the hash, is read only from
a hash that does not start with `#/`.

The sidebar lists, top to bottom: Overview; one link per CLI in use, with
its icon, the login in use (or "signed out"), the share used of its tile's
window over a small meter, and an "auto" tag when auto mode is on; Usage; the
API proxy, with an on/off dot; Status, with a dot that turns amber when a
vital is a warning and red when one is bad (`healthTone`,
[doctor.md](doctor.md)); Versions. Its foot holds Add a login, Refresh tokens,
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
  plan's nominal multiplier, in units of the largest plan, so an Ultra at
  31% and a Pro at 100% make "41% of 110%"; each segment is as wide as its
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
  link or page (`windows.ts` `inUse`); Add a login still offers it. One signed
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
  percentage and reset over a bar; under the bar the window's trail since it
  opened, a step line on the window's whole length with the forecast dashed
  on from now, and "Full Thu 14:00 at this pace" or "Ends ~60% at this pace",
  toned like a meter, with the recent rate on hover, `forecast.ts`; neither
  when TokenGauge has no history or burn for the window; then "Figures from N minutes ago" when
  TokenGauge carried them from an earlier ask than its snapshot, `windows.ts`
  `carriedSince`), a Claude login's banked usage-limit resets and when they
  expire, with a Reset limits button that asks for a second click before it
  spends one and then holds until TokenGauge reports the new count, how long
  its access token and its sign-in (refresh token) have left, its
  keep-warm line, and Refresh, Sign in again, Label, Rename and Remove.
  Sign in again opens the Add a login dialog set to that login, with no CLI
  or name to pick and no overwrite to confirm, and closes it once stored; it
  stands out when the sign-in has run out. Remove asks
  for a second click; Use, Refresh and Remove are disabled on the active one,
  and Sign in again too, since the CLI signs its own login in.
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
- **Status**: the vitals `remuda doctor` checks ([doctor.md](doctor.md)),
  one row each with a dot for its level, its fix and, behind a toggle, its
  items (what the last refresh skipped or failed on, TokenGauge's errors, the
  logins to sign in again); and the store's path.
- **Versions**: a card each for remuda and TokenGauge with the version
  installed (TokenGauge's from `tokengauge --version`) and GitHub's latest
  release, and an Update button, unless a package manager owns the binary
  (selvedge's owner check for remuda, a path under `/usr` outside
  `/usr/local` for TokenGauge), which the card then says instead; a
  TokenGauge not installed gets a link to its repository, and one that does
  not answer `--version` is shown as not answering, with no Update. Below, the release
  notes of every release newer than the one installed, up to ten, or of the
  latest when none is, for the tool picked: the CHANGELOG section GitHub's
  release carries, its `###` headings as labels, up to the first `##`
  (GitHub's generated list of pull requests), with bold, code and `http(s)`
  links rendered and anything else as text (`notes.ts`). GitHub is asked
  when the page opens, at most every 15 minutes, and on Check for updates.
  Updating remuda is selvedge's `apply`, as `remuda --update`, then `remuda
  sync-units` a second after the answer; under the service that restarts
  the page, which keeps its token
  ([architecture/web.md](../architecture/web.md#restarting-on-an-update)),
  and the page reloads once it answers again. Updating TokenGauge runs
  `tokengauge --update`, which refreshes its own frontends.

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

## Usage

Three tabs, each filtered by a CLI and a login, the first two by a range (7,
30 or 90 days, 12 months), and the second by a proxy key.

- **Spend** is TokenGauge's `tokengauge --usage-json` (TokenGauge 0.52 or
  later), which `serve` runs on demand (`usage::spend`: found on `PATH` or in
  `~/.local/bin`, 10 seconds at most, one answer kept a minute): the total
  and each CLI's share with its sessions, daily cost per CLI, the token
  totals, cost and tokens by rate, spend per login and per model, and a
  model's own view. remuda prices nothing: every dollar is TokenGauge's, at
  each month's prices, so the page and the panel agree. A row TokenGauge has
  no price for counts its tokens and shows as unpriced, never as free (a total
  with some marks `+`). Logins come back as account digests and are named from
  the store's sidecars; a removed login keeps the last name the attribution
  ledger wrote down for it, marked "(removed)", and one the ledger never named
  is "A login no longer stored". Spend from before remuda recorded who spent it is
  unattributed ([switching.md](switching.md#whose-spend-it-is)). Without a
  TokenGauge that exports, the tab says why.
- **Proxy requests** is the proxy's request log
  ([proxy.md](proxy.md#the-request-log)), summed by the server per quarter hour,
  provider, model, login, key and outcome, and again by the page for each
  view: requests, output tokens per second while answers were written,
  tokens, the success rate (canceled requests aside), cache hit, latency,
  requests per minute, tokens per day per model, outcomes and the models that
  failed most, requests per day by outcome, and the split by login.
- **Value** is what a week of each login bought: the API dollars it spent per
  percent of its weekly allowance, week by week, with a chart once there are
  two weeks and how the last closed week compares with the four before it.
  `value::weeks` joins TokenGauge's window history (the weeks it recorded as
  they closed, TokenGauge 0.56 or later, and the one still open) with the
  spend export of the last 400 days by login. A login's week is its first
  weekly window of the three fixed ones, since a model's own window counts one
  model and the spend is every model's. A day the week covers in part counts
  in proportion, a week reset early ends where the next one opened, and the
  open week runs to now ("so far"). Spend no record attributes is shown beside
  the week, a week from before the usage record starts is marked, and a week
  of a login the store no longer names has no figure. Use the export cannot
  see (the web, the desktop apps, a machine without TokenGauge) makes a week
  look worth less than it was, and the tab says so.

Charts are inline SVG (`Charts.tsx`) with a hover readout; models and token
rates take a fixed categorical palette (`--c1` to `--c7`, validated for both
themes), CLIs their brand colour, outcomes the status colours, each with a
legend or labelled values.

## Auto mode

While auto mode is on, the Automation tab opens with what its last round decided
([auto-mode.md](auto-mode.md)): the rule in force, every login weighed with
its score and why it was or was not picked, the decision and why, and the last
switch auto mode made.

The Automation tab edits `config.toml`
([architecture/config.md](../architecture/config.md)): a card with auto
mode's switch, the minimum time on a login, the margin and the
ceiling, each login's own ceilings (for its shortest window and for the
others), the fallback and whether to take it even when maxed, the
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

- [web/src/](../../web/src/), built by [build.rs](../../build.rs);
  the settings in [AutoMode.tsx](../../web/src/components/AutoMode.tsx) and
  [config.ts](../../web/src/config.ts)
- [src/serve.rs](../../src/serve.rs)
- [src/gauge.rs](../../src/gauge.rs), [web/src/forecast.ts](../../web/src/forecast.ts)
- [src/runs.rs](../../src/runs.rs)
- [src/versions.rs](../../src/versions.rs), [web/src/components/Versions.tsx](../../web/src/components/Versions.tsx),
  [web/src/notes.ts](../../web/src/notes.ts)
- [src/value.rs](../../src/value.rs), [web/src/components/Value.tsx](../../web/src/components/Value.tsx), [web/src/value.ts](../../web/src/value.ts)
- [src/usage.rs](../../src/usage.rs), [web/src/components/Usage.tsx](../../web/src/components/Usage.tsx),
  [Charts.tsx](../../web/src/components/Charts.tsx), [spend.ts](../../web/src/spend.ts),
  [requests.ts](../../web/src/requests.ts)
