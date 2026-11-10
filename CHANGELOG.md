# Changelog

## [Unreleased]

### Added

- A Claude Code hook, `remuda hook artifact`: a call on an artifact another
  stored login published switches Claude Code to that login for the call, then
  back, instead of being refused after a switch. It learns who owns what from
  each publish and list.
- `remuda help skill` prints a SKILL.md for Claude Code sessions: what a switch
  does to them, the hook and its settings, and the commands.
- `remuda doctor` checks that the artifact hook is installed once two Claude
  logins are stored.
- `remuda signout <cli>`, and Sign out on the page, take the login out of a
  CLI's own files, keeping it in the store. remuda can then refresh and sign in again
  every login, even a lone one, without going through the CLI.
- Signing in shows the link before anything opens, so you choose where it
  goes: Enter in the terminal, or Open on the page, opens it here, and Grok,
  Kimi, Cursor and Claude with `--paste` can be signed in from a browser on
  another machine.
- Where a sign-in opens is a setting, `[login]` in config.toml, which the Add
  a login dialog sets too, and `--browser` / `--window` for one run: any
  browser remuda knows rather than only the default one, and a private window
  as before, a normal window, or a profile of the login's own, kept under the
  store, which stays signed in for the next time.
- A banked Claude reset says what it resets: `1 Fable reset` with a Reset
  Fable limit button, from TokenGauge's `bankedResets.kinds`.
- `remuda export` and `remuda restore` set another machine up in one step:
  the logins, config.toml, the proxy's settings and TokenGauge's fleet with its
  S3 keys (`--without-s3-keys` leaves them out), through wyrmyon or
  magic-wormhole, or a file sealed in age's format under a passphrase. API keys
  and Cursor logins go across as they are; Claude Code, Codex, Grok and Kimi
  logins are signed in again over there, since their refresh tokens are single
  use, and `--move` carries one's tokens for a machine being
  retired. Joining the fleet needs TokenGauge with `--sync-export` on the
  sending machine and `--sync-join` on the new one; when the sending one
  cannot export the fleet, export goes on without it, and when the new one
  cannot join it, restore says so and restores the rest. No
  refresh token is lost on the way: a moved login is set aside before it is
  removed, and stays when a refresh changed it meanwhile; `restore --force`
  keeps the login a CLI or a `remuda run` session uses, one stored here it
  cannot read, the proxy's client keys, and proxy settings it cannot read, and sets aside the tokens and API keys it replaces and the tokens,
  not the API keys, it cannot store, except a
  login of a provider it does not know, which stays on the sending machine,
  in its store or, once moved, set aside there; a bundle received that cannot be restored is kept. restore signs in the way `remuda
  login` does, with `--paste` and `--no-paste`.

### Changed

- Claude signs in the way Claude Code does: the browser calls back to remuda,
  with nothing to paste. `--paste`, `paste = true` under `[login]`, or the
  checkbox on the page brings the code to paste back, for a browser on another
  machine. `remuda login` pastes by default over SSH, or on Linux with no
  display, where the callback could not arrive, and `--no-paste` asks for the
  callback anyway. The page names the machine a callback sign-in's browser must
  be on.
- The same switch signs Codex in by device code, as `codex login --device-auth`
  does: remuda shows a code to type at auth.openai.com/codex/device from any
  machine's browser, so Codex no longer needs the browser on this machine.
  `remuda login` does so by default over SSH or, on Linux, with no display, as
  it pastes for Claude, and `--no-paste` keeps the callback. Its sign-in constants now
  follow codex-cli 0.167.0.
- Spending a banked reset skips one whose limits are all at 0%, as claude.ai
  does, rather than burning it on nothing.
- A Grok, Kimi, Cursor or Codex device sign-in rides out a busy or failing
  server the way it rides out a dropped connection, asking again until the
  third in a row, instead of throwing away an approval already given. Codex
  and Cursor count every 429 or 5xx; Grok and Kimi do not count one whose body
  says "pending" or "slow down", and start the count over on it. An approval
  cut off on its way back is asked for again the same way.

### Fixed

- A sign-in cancelled on the page, or replaced by another for the same CLI,
  while it was finishing is no longer stored.

## [0.38.0] - 2026-11-10

### Added

- The API proxy card has a "Reachable from other machines" switch (`lan` in
  `.proxy.json`): the proxy listens on every interface and its base URLs carry
  the machine's LAN address. When ufw or firewalld blocks the port, the card
  shows the commands that open it on the LAN interface, and says when a rule
  for some machines may still keep them out.

### Fixed

- The page's dropdowns draw their arrow inside the field, clear of the rounded
  edge, instead of the browser's arrow pressed against the border.

## [0.37.0] - 2026-11-07

### Added

- `remuda open --add` lands on the sheet that adds an account, for a tool that
  offers an add button and leaves the sign-in to remuda.

## [0.36.0] - 2026-11-06

### Changed

- Auto mode uses logins in the order you list them: the first one under its
  ceiling wins, and the CLI goes back to an earlier one once it has room.
  Ranking a tier by the headroom a reset would wipe (the `%/h` score) is now
  the `balance` option, off by default.
- A switch held back by the minimum time on a login says which login it would
  go to and why, and every login auto mode weighs shows its window closest to
  its ceiling.
- The Automation tab has a Simple view (logins in order, arrows to move them,
  one ceiling each) and an Advanced one with the rest. A login shows ceiling
  fields only for the windows it has.
- The sidebar shows the login in use's own window closest to full, the figure
  its card shows, not the total across logins.

### Fixed

- Keep-warm no longer offers logins without a 5-hour window.
- A login auto mode may pick that is no longer stored shows as missing, and
  unticking it takes it out with its ceilings. It used to be hidden, while the
  config check kept Save from working.

## [0.35.0] - 2026-11-05

### Added

- With TokenGauge 0.58's fleet sync on, the Quota page's drain chart and the Value
  view read what the whole fleet saw of each limit
  (`tokengauge-window-fleet.json`), so a week this machine was off for is
  drawn from the samples the others took. Without it they read this
  machine's own history as before.

## [0.34.0] - 2026-11-05

### Changed

- Codex and OpenRouter show their current logos on the page: Codex's cloud
  with a prompt instead of OpenAI's knot, and OpenRouter's new mark. Codex,
  Cursor and OpenRouter logos are drawn in the page's ink and keep their colour
  for accents only.

## [0.33.2] - 2026-11-05

### Fixed

- A `remuda run` session's lock is let go as soon as remuda is done with it,
  even when `remuda serve` starts a program at that moment. The session could
  otherwise look held and be left for a later command to finish.

## [0.33.1] - 2026-11-05

### Fixed

- A page link opened in a new tab works: the new tab takes the token from
  a tab already open instead of asking for the link remuda printed.

## [0.33.0] - 2026-11-05

### Added

- The page's sidebar puts a dot on Versions while remuda or TokenGauge has a
  newer release, so an update shows without opening the view.
- A Quota page pools every login of a CLI into one allowance and charts it
  over two weeks: what was left, as TokenGauge recorded it, and where it is
  heading if the burn of the last 1, 3 or 6 hours holds (or half, double or
  none of it), each reset filling its login back up and labelled with what it
  gives back. It says when the pool runs dry, the first login to, the next
  reset and the burn, and can chart the burn itself.

### Changed

- On a phone, the page's places no longer scroll sideways along the top: a
  menu button opens them as a panel of tiles, with Add a login, Refresh tokens
  and the theme and look switches, which leave the Status page.

### Removed

- A login's details no longer draw each window's trail or say "Full Thu 14:00
  at this pace": the Quota page projects the whole pool instead.

### Fixed

- opencode 2 keys work again. opencode 2 moves its logins out of `auth.json`
  into its own database and stops reading the file, so remuda saw opencode as
  signed out and `use` changed nothing. remuda now reads the key opencode
  uses from that database and switches it there: a key opencode already holds
  is made the active one, so it also shows in `opencode auth list`. opencode
  1 still uses `auth.json`.

## [0.32.0] - 2026-11-04

### Added

- The Usage tab has a Value view: what a week of each login bought, in API
  dollars per percent of its weekly allowance, week by week, and how the last
  week compares with the ones before. A subscription whose limits were cut or
  raised shows up as a step. Needs TokenGauge 0.56 or later, which records
  each week as it closes; until a week has closed, only the current one shows.

## [0.31.1] - 2026-11-03

### Fixed

- `remuda doctor` and the Status page showed Claude Code's version for Codex
  when both are installed through a version manager such as mise, whose
  shims all link to one program.

## [0.31.0] - 2026-11-03

### Added

- A login's details draw each usage window's trail since it opened and say
  where it is heading at its recent pace: "Full Thu 14:00 at this pace" when
  it runs out before its reset, "Ends ~60% at this pace" otherwise. The pace
  is how fast the window has been filling lately, as TokenGauge measures it,
  so a morning's burst followed by a quiet afternoon does not read as a
  run-out. Needs TokenGauge 0.54 or later; with an older one the meters show
  as before.
- The page has a Versions view: the remuda and TokenGauge installed here
  beside GitHub's latest release of each, an Update button for either, and
  the release notes of every version since the one installed. Updating
  remuda under `remuda-serve.service` restarts the page, which keeps its
  link working and reloads by itself. A binary a package manager installed
  is left to it, and the card says so.
- `remuda doctor` checks what remuda relies on and says how to fix what is
  wrong: whether `remuda serve` runs, as the service and at this version,
  when auto mode, keep-warm or the proxy need it; the refresh timer and its
  last run; how old TokenGauge's figures are, since auto mode holds on a
  stale snapshot; logins whose refresh token expired or that are unverified;
  CLIs on a login remuda has not stored; the installed Claude Code and Codex
  against the releases remuda's sign-in was read from; and an untrusted
  `.remuda.toml` here. It exits non-zero when something is broken, and
  `--json` prints the checks as data.

### Changed

- The page's Status shows the same checks as `remuda doctor`, each with its
  fix, and its sidebar dot turns amber or red for any of them, not only the
  refresh timer.

### Fixed

- Usage shows a removed login's spend under the name it last had, marked
  "(removed)", instead of "A login no longer stored".
- A usage window whose reset has passed shows 0% on the page, instead of
  its old percentage and a reset in the past, while TokenGauge has not
  fetched that login since.

## [0.30.0] - 2026-11-02

### Added

- Plain `remuda` shows a table of every stored login, the one each CLI uses,
  and how long ago its credential file was last written (a refresh, the CLI's
  own rotation, or a label), in colour on a terminal unless
  `NO_COLOR` is set. It used to print the help, which `remuda --help` still
  does.

## [0.29.0] - 2026-11-02

### Added

- The page has a Usage tab. **Spend** shows what each CLI, model and login
  spent over 7 days to 12 months, priced by TokenGauge (0.52 or later):
  daily cost, the token totals, cost and tokens by rate, and each model on
  its own. **Proxy requests** shows what the API proxy answered: requests,
  success rate, output speed, latency, cache hits, tokens per model per day,
  and the split by login and key.
- remuda writes down which login each CLI spends and from when, in
  `.attribution.jsonl` in its store: on every switch, when it finds a login
  changed outside it, and for each session a `remuda run` Claude Code goes
  through. TokenGauge reads it to tell what each login spent, from now on.
- The API proxy logs every request it forwards to `.proxy-requests.jsonl` in
  the store: the login and key it spent, how it ended, how long it took, its
  tokens, and the ids the client got, by which TokenGauge counts a request
  Claude Code also wrote down only once.

## [0.28.0] - 2026-11-01

### Changed

- Claude's Max 5x and Max 20x are listed as `max` and `ultra`, their new
  names, in `ls`, `ls --json` and on the page. The page's weighted total takes
  its weights from TokenGauge, which now counts an Ultra as 10 Pros, not 20.
- The page's Auto, Light and Dark theme choices are icons, so they share one
  line with the Sage and Instrument look.

## [0.27.0] - 2026-10-27

### Added

- Auto mode's `long_ceilings` gives a login its own ceiling for its windows
  other than the shortest (Claude's weekly one), or for its only window, as
  `ceilings` does for the 5-hour one: `long_ceilings = { perso = 94 }` leaves
  perso at 94% of its week while the other logins run to the CLI's ceiling.
- `fallback_when_maxed = true` goes to the fallback when every login is full
  even if the fallback is maxed itself, so the login being left is spared.
  Both are on the Automation tab.

### Changed

- The page's countdowns name a second unit: a window resets "in 1 day and
  10 hours" or "in 1 hour and 25 minutes" rather than "in 1 day" or "in 1
  hour".
- Building remuda from source needs Node and pnpm, and `pnpm install` in
  `web/`: `cargo` builds the page, which is no longer kept built in the
  repository. The Arch `remuda` package lists `nodejs` and `pnpm` as build
  dependencies. Release archives and packages are unchanged.

## [0.26.0] - 2026-10-23

### Added

- A login's card on the page has a Sign in again button, for a login whose
  refresh token was rejected or whose sign-in ran out. It signs the same name
  in again in the browser. Signed in with the same account, the login keeps
  its label, as it now does with `remuda login <name> --force`.

## [0.25.0] - 2026-10-23

### Added

- The API proxy page has a Shell field with the two `export` lines a client
  reads (`ANTHROPIC_BASE_URL` and `ANTHROPIC_API_KEY`, or `OPENAI_BASE_URL`
  and `OPENAI_API_KEY`), copied in one click with the whole key.

## [0.24.0] - 2026-10-22

### Added

- `remuda run <name>` starts Claude Code or Codex on a stored login for that
  session only, without switching the one every other terminal uses
  (`remuda claude work` for short). The session shares your history, projects,
  settings and MCP servers, the tokens its CLI refreshes go back to the store
  when it exits, and while it runs that login is not refreshed, switched to,
  removed or pinged. A second terminal on the same login joins the session.
  `ls` and the page say which logins are in a session.
- Pins: `remuda pin claude/work` writes `.remuda.toml` in a repository, and
  with `eval "$(remuda shell-init zsh)"` every `claude` or `codex` started
  there runs on the pinned login. Elsewhere they run as before. A
  `.remuda.toml` remuda did not write is read only once `remuda pin --allow`
  trusts it.

## [0.23.0] - 2026-10-22

### Added

- remuda keeps a log of what it did: every switch with who made it (the CLI,
  the page or auto mode) and why, refreshes and the ones that failed, auto
  mode's decisions, keep-warm pings, banked resets, the proxy turning a login
  down, and logins stored, renamed or removed. `remuda log` prints it, narrowed
  to a CLI or a login, and the page has a Log in its sidebar, by day, narrowed
  by CLI or to failures. A failure that repeats is logged once, so a revoked
  spare the timer cannot refresh is one line rather than one every half hour.

## [0.22.0] - 2026-10-22

### Changed

- Each CLI's logo and colour are the ones TokenGauge draws, both now taken
  from selvedge, and follow the provider's own: Kimi's is its current K, in ink,
  with the blue dot, opencode's has the block inside its frame, and Z.ai and
  opencode, whose brands are black and white, are drawn in the page's ink
  rather than red and blue.

### Fixed

- A CLI's Automation tab shows auto mode's switch once: the header's "Auto mode
  on/off" badge is on the Logins tab only, and the card no longer repeats the
  CLI's name.
- Keep-warm has its own card on the Automation tab, beside auto mode's rather
  than inside it, since it runs whether auto mode is on or off.

## [0.21.0] - 2026-10-22

### Changed

- The page is reorganised around a sidebar: an overview, a page per CLI with
  its logins and its automation on two tabs, the API proxy and status, each
  with its own address so back and bookmarks work. A CLI's logins are a list
  you can order by headroom or name, with a meter per window in each row and
  the rest a click away. Every CLI is shown by its real logo, from
  TokenGauge, Add a login is a dialog, and the page works on a phone.
- The page has two looks to choose from, Sage and Instrument, beside the
  light and dark themes.

## [0.20.0] - 2026-10-21

### Added

- Keep-warm: `remuda serve` reopens a chosen login's 5-hour window with the
  smallest request it takes as soon as it closes, so one is always running.
  Logins opt in under `[warm]` in `config.toml`, or with chips on each CLI's
  card in the page's Auto mode section; a login card shows the last ping, and
  `remuda auto` says which logins are due.
- A login's usage on the page says how old it is when it is older than
  TokenGauge's snapshot ("Figures from 23 minutes ago"). TokenGauge asks about
  a login the CLI is not signed into less often and carries its figures in
  between, so a login spent from another machine could look current while its
  figures were half an hour old.

## [0.19.0] - 2026-10-21

### Added

- Packages: each release has a `.deb`, an `.rpm` and Arch packages
  (`remuda-bin`, and a PKGBUILD that builds from source) for x86_64 and
  aarch64, with the binary in `/usr/bin`, the completions and the systemd
  units. `remuda --update` leaves a packaged binary to its package manager.
- Releases are signed: the `.rpm`s, the Arch packages and `SHA256SUMS` carry
  signatures from the key in `packaging/remuda.asc`.

### Changed

- `install.sh` checks the archive against the release's `SHA256SUMS` before
  running anything from it, and refuses a release that has none (anything
  before this one).

### Fixed

- The release binary is static, so it runs on Debian 12 and Ubuntu 22.04: it
  needed glibc 2.39 before.

## [0.18.0] - 2026-10-21

### Added

- The proxy translates between formats, so any client reaches any CLI:
  OpenAI chat completions, OpenAI responses and Anthropic messages are turned
  into each other, streamed or not. Claude Code can run on a Codex login
  (`--model gpt-5`), and an OpenAI SDK on a Claude login. A request Codex gets
  without `stream: true` is answered whole instead of refused.

## [0.17.0] - 2026-10-21

### Added

- Auto mode ceilings: a window can count as full before 100% (`ceiling`), and
  a login can have its own ceiling for its 5-hour window (`ceilings`), so auto
  mode moves a CLI on before it hits the wall. `fallback` names the login to go
  to when every login, the active one included, is full. All three are on the
  page.

## [0.16.0] - 2026-10-21

### Added

- An API proxy in `remuda serve`, off until switched on from the page's new
  API proxy card. Point an OpenAI or Anthropic client at
  `http://127.0.0.1:8317` (`/v1` for OpenAI) with a key made on the card: the
  model name picks the CLI (`claude-*`, `gpt-*`, `grok-*`, `kimi-*`, `glm-*`,
  or `provider/model`) and the request spends that CLI's active login, which
  the proxy never refreshes. Each CLI takes the formats its own endpoint
  speaks.

## [0.15.0] - 2026-10-21

### Added

- Auto mode: `remuda serve` keeps each CLI on the most suitable of the logins
  you allow, and moves it off one that is maxed. Rules pick logins by schedule
  (work hours, evenings) in tiers, a login can be kept for a schedule unless
  its week resets before the schedule starts, and within a tier the login with
  the most headroom to lose at its next reset goes first. Off by default, set
  up per CLI on the page or in `~/.config/remuda/config.toml`. Codex is
  switched only while no Codex session runs, since a running one cannot take
  a new login.
- `remuda auto` says which login auto mode would put each CLI on now, and why.
- `remuda rename` and `remuda rm` keep the config's login names in step.

## [0.14.0] - 2026-10-20

### Added

- A Claude login's card shows its banked usage-limit resets and when they
  expire, as TokenGauge reports them, and Reset limits spends one with that
  login's own token after a second click.

## [0.13.1] - 2026-10-13

### Fixed

- A login refreshed or switched to shows its usage within seconds:
  `remuda use`, a `remuda refresh` that refreshed something, and the page's
  Use and Refresh ask TokenGauge to fetch at once, instead of the page
  showing "Its access token has expired" until TokenGauge's next poll.
- The page names a login's plan when only TokenGauge knows it, such as a
  Grok login's tier, instead of showing `-`.

## [0.13.0] - 2026-10-12

### Fixed

- A premium Claude Team seat is recognised. Anthropic reports its seat tier
  as `team_tier_1`, so `ls` printed `team team_tier_1`; the seat is now read
  off the rate-limit tiers the login carries, as TokenGauge reads it, and
  lists as `team premium` (or `team standard`). A Team plan no longer lists as
  `team 5x`.
- Re-identifying a credential whose sidecar was torn keeps the parts of
  Claude Code's account block a profile does not carry, including the tier
  that marks a premium seat.

### Changed

- `--update` refuses a binary a package manager installed (a distribution
  package, the Nix store, a snap) and says how to upgrade it instead, as it
  already did for Homebrew.

### Removed

- `remuda seat`, the page's seat picker and `POST /api/seat`: the seat is
  detected now. A seat chosen with them is ignored and dropped from the
  sidecar the next time it is saved. `ls --json` loses `reportedSeat`; its
  `seat` is the detected seat (`standard` or `premium`), and its `plan` is the
  plan as `ls` prints it.

## [0.12.0] - 2026-10-12

### Changed

- Updating is a flag, as in TokenGauge and TailGauge: `remuda --update`
  replaces the binary and `remuda --check-update` reports whether a newer
  release exists. The `remuda update` subcommand is gone.

## [0.11.0] - 2026-10-11

### Added

- `remuda seat <name> standard|premium|auto`, and a seat picker on a Claude
  Team login's card, say which Team seat a login holds when the one Anthropic's
  profile reports is wrong. TokenGauge 0.44.0 and later weigh the login by it.

## [0.10.0] - 2026-10-10

### Changed

- The page leaves out a CLI with no stored login that is signed out: no
  summary tile, tab or section for it. "Add an account" still lists every CLI,
  and a CLI signed into a login remuda has not stored still shows, to import
  it.

### Fixed

- The page counts a plan whose weight is not a whole number. TokenGauge now
  weighs a Claude Team seat as the 1.25x (Standard) or 6.25x (Premium) of a Pro
  it is sold as, and remuda read only whole weights, so a Team seat would have
  dropped out of the weighted total.

## [0.9.0] - 2026-10-09

### Added

- Grok, Kimi Code and Cursor (`cursor-agent`) logins: import, sign in, switch
  and keep fresh, like Claude Code and Codex. Each CLI's whole credential file
  is the login. Grok and Kimi sign in with a device code, approved in the
  browser with nothing listening locally; Cursor polls cursor.com the way
  `cursor-agent login` does.
- GLM and opencode Go API keys, stored by name. `remuda login -p glm <name>`
  opens the key page and asks for the key. A key never expires, so `refresh`
  leaves it alone. opencode is switched by rewriting the `opencode-go` entry of
  its `auth.json`; GLM reads its key from the environment, so
  `eval "$(remuda env glm/<name>)"` selects one in a shell.
- `remuda env <name>` prints the `export` line for a CLI that reads its login
  from the environment.

## [0.8.0] - 2026-10-08

### Changed

- The `serve` page is redesigned. Each login is a card with a row per usage
  window, its token lifetimes and its actions, and tabs above the cards show
  every CLI or one of them. Each CLI's summary adds up a usage window across
  all its logins over a bar with a segment per login, with the other
  windows listed under it. Logins count by their plan the way TokenGauge
  weighs them (a Max 20x and a Pro make 105%, not 200%), or once each with
  the Absolute switch.

## [0.7.0] - 2026-10-08

### Fixed

- On a desktop that runs its terminal as a systemd service (GNOME), commands
  typed in it were taken for systemd runs: they never recorded where the
  CLIs keep their logins, so `remuda-serve.service` would not start, and
  `import` refused a missing store. remuda now tells its own units by their
  name, and `remuda update` rewrites the units you installed so they also say
  so (`REMUDA_SERVICE=1`).

### Changed

- Only `import` and `login` create the store. `ls`, `use`, `serve` and the
  other commands that work on it now refuse when there is none, naming what
  they were doing, so an unset or mistyped `REMUDA_STORE` no longer starts an
  empty second store. On a fresh install, start with `remuda import` or `remuda login`;
  `install.sh --serve` leaves the page's service for `remuda open` to start
  after that.

## [0.6.0] - 2026-10-03

### Changed

- `remuda update` also brings the systemd units you installed up to date
  with the new release, and reloads systemd when one changed. It never adds a
  unit you did not install: `remuda-serve.service` still comes only from
  `install.sh --serve`. This takes effect from the update after this release.

## [0.5.0] - 2026-10-03

### Added

- `remuda open` opens the page a running `remuda serve` shows, so you no
  longer need the URL it printed. With `--no-browser` it prints the URL.
- `install.sh --serve` keeps the page served by a systemd user service,
  `remuda-serve.service`, so `remuda open` works any time; `open` starts the
  service when it is stopped. Like the refresh timer, the service refuses to
  start when the shell has moved `CLAUDE_CONFIG_DIR` or `CODEX_HOME` since,
  and it never writes the page's token to the journal. `remuda update` and
  the installer restart it on the new binary.

### Changed

- The `serve` page updates itself within a couple of seconds when the store
  or TokenGauge's usage changes, for example after a switch from the CLI, a
  refresh-timer run or a new TokenGauge fetch, instead of only once a minute.

## [0.4.2] - 2026-09-29

### Fixed

- The `serve` page shows each login's own usage with TokenGauge 0.37 and
  later, which reports every stored login. It used to show the last login
  TokenGauge listed as the one in use, so another account's figures could
  appear under it. A login TokenGauge skipped, because its access token
  expired or it is unverified, says so.

## [0.4.1] - 2026-09-29

### Fixed

- Adding an account no longer signs your browser's own claude.ai or ChatGPT
  session over to it. `remuda login` and the `serve` page open the sign-in page
  in a private window of the default browser (Brave, Chrome, Chromium, Vivaldi,
  Edge, Firefox, LibreWolf; not Snap or Flatpak packages) instead of a normal
  tab. With another browser, or no display, they fall back to the old
  behaviour. The page can also copy the sign-in link for pasting into a private
  window by hand.

## [0.4.0] - 2026-09-29

### Added

- remuda has an icon: a horseshoe holding a keyhole. The `serve` page uses it
  as its favicon, served by remuda itself.

### Changed

- On the `serve` page, Enter confirms the Import name and the pasted sign-in
  code too, as it already did the Add an account name and the editors.

## [0.3.0] - 2026-09-29

### Added

- The `serve` page can switch away from a live login remuda cannot keep, and
  replace a stored credential on import or sign-in, each after a second click,
  as `use --discard` and `--force` do.

### Fixed

- The `serve` page finds the refresh timer wherever systemd loads user units
  from, not only `~/.config/systemd/user`, and reports a masked timer as
  masked rather than enabled.
- `use --discard` no longer turns off the check that keeps a login the CLI
  rotated mid-switch when the live login is stored and confirmed.

## [0.2.0] - 2026-09-29

### Added

- The `serve` page shows how long each credential's access token and sign-in
  have left, whether the refresh timer is on and what its last run refreshed
  or failed on, and, with TokenGauge installed, the usage of the login in use.
- The scheduled refresh records its last run in `<store>/.last-refresh.json`.
- A switch records when it happened in `<store>/.last-switch.json`, so the
  page does not show the previous login's usage for the new one.

### Changed

- The `serve` page is redesigned: a summary per CLI whose limit you pick, then
  one row per credential with meters and icon actions, and a light/dark
  switch.

## [0.1.1] - 2026-09-28

### Fixed

- `serve` checks a request's host and token before reading its body, bounds
  how long a client may take and how many connections it serves at once, and
  no longer stops when another local user holds connections open.
- A credential whose sidecar was written for other tokens (a save interrupted
  halfway) is detected and re-identified with the provider, even while the CLI
  is signed out or the access token has expired, instead of being refreshed,
  switched to or labelled under the wrong account.
- Switching no longer reverts changes Claude Code saved to `.claude.json` or
  `.credentials.json` meanwhile, and never overwrites a login the CLI rotated
  mid-switch.
- The refresh timer refuses to refresh a CLI whose login it would look for in
  another place than your shell does, including with a unit installed by
  0.1.0.

## [0.1.0] - 2026-09-28

- `import`, `login`, `use`, `ls`, `refresh`, `label`, `rename` and `rm` for
  Claude Code and Codex logins. A name is bare when only one CLI has it, and
  `claude/<name>` or `codex/<name>` otherwise.
- `serve`: a local page to list, switch, label, rename, remove, import and sign
  in credentials, guarded by a token in the URL it prints.
- `completions <shell>` for bash, zsh, fish and the other shells clap knows.
- `update`, and an installer that fetches the release, enables the refresh
  timer and installs completions.
