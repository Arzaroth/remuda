# Roadmap

Features remuda may grow next. None is started; order is not priority.
Small fixes and chores go in [TODO.md](TODO.md). When a feature ships, its entry
leaves this file and a doc for it lands in `brain/features/`.

## Auto-switch when a limit is maxed

When the active login's usage window is used up, switch the CLI to a stored
login with headroom left. Needs a watcher (the refresh timer, or `serve`) that
reads TokenGauge's figures, and a rule for which login to pick.

Two refinements:

- **Eligible logins.** The user marks which stored logins auto-switch may pick
  (a per-credential flag, or a list per CLI). A login left out is never
  switched to automatically, only by hand.
- **A real auto mode.** Not only a fallback when the active login is maxed:
  remuda keeps the CLI on the most suitable eligible login at every moment. A
  window that resets soon loses whatever is left of it, so a login close to its
  weekly reset with headroom left should be spent before one whose week has just
  started. A score could weigh the headroom left in each window against the time
  until it resets, skipping any login whose 5-hour window is maxed.

Every switch auto mode makes is still a switch: the live login is synced back
first, and it never fires while a session is mid-request if that can be told
apart. Switches should not flap, either: a minimum time on a login, or a margin
the new pick has to beat.

## Auto-switch by rule

Switch on criteria set in advance, e.g. a login per time range (work hours,
evenings). Other criteria are still open.

## `remuda run`

`remuda run <profile>` (or `remuda claude <profile>`) starts a Claude Code
session on one login without switching the one every other session uses. It
builds a temporary `CLAUDE_CONFIG_DIR` whose entries are symlinks to the real
config, so history, settings and MCP logins stay shared, except for the
credentials, which are that login's own.

While the session runs, its login is in use (flock, or a check that the process
is alive) and remuda treats it like the active credential: never refreshed, and
its tokens synced back to the store when the session ends, since Claude Code
may have rotated them.

## Hub and satellites

Every account is signed in on one machine only, the hub, which runs remuda.
Other machines, the satellites, point to it. A satellite may run remuda as
well, only to apply that setup: the local remuda connects to the hub and takes
what it needs. It is meant for a direct LAN or a tailnet (Tailscale, NetBird),
never the open internet.

Two ways to do it, which do not keep the same rules:

- **The hub hands over credentials.** A satellite fetches the active login's
  credential file and writes it into its CLI's files. Then a satellite's CLI
  refreshes on its own and rotates the refresh token behind the hub's back,
  which breaks "never lose a refresh token". It only holds if satellites get
  an access token and no refresh token, and fetch a new one from the hub
  before it expires, and Claude Code and Codex have to cope with a credential
  that has no refresh token.
- **The hub proxies the API.** The satellite's CLI talks to the hub
  (`ANTHROPIC_BASE_URL`, Codex's base URL setting) and the hub adds the active
  login's token to each request. Tokens never leave the hub, and switching on
  the hub switches every satellite at once. Then remuda runs an HTTP proxy that
  streams responses, and it has to check that each CLI still works when its
  base URL is changed while signed in with OAuth.

Either way, the hub listens beyond `127.0.0.1`, which the page refuses to do
today: it needs a bind address the user chooses, a pairing step that gives each
satellite its own token (revocable, never on a command line), and, on a plain
LAN with no tailnet, TLS or a warning that traffic is in clear.

## Banked resets

List the resets each Claude and Codex login has banked, and use one from remuda
(the CLI and the page). Where the figures come from is still open: TokenGauge's
snapshot, or a call remuda makes itself. In the second case the endpoint gets
read out of the CLI's binary like the OAuth constants, not guessed. Using a
reset cannot be undone, so it asks for a second click on the page, and a
confirmation (or a flag) in the CLI.

## A management page

`remuda serve` grows from one page into a small management app, along the lines
of CLIProxyAPI's management center: a sidebar with sections (Dashboard, Quota,
Accounts, Add an account, Settings, Logs), a quota view per provider with
tabs and one card per provider (the summed or tightest window across its
logins, with a bar per login), rows giving every window of each login with its
reset, and a settings section for what remuda gains on the way: auto-switch and
which logins are eligible, the refresh timer, the hub's bind address and
satellites.

Settings mean remuda gets a config file it does not have today
(`$XDG_CONFIG_HOME/remuda/config.toml`), shared by the CLI and the page.

A framework is worth it once there are several sections and shared state, but
the page is compiled into the binary and loads nothing from the network, and
both have to stay true. Options:

- Preact with htm, vendored as ES modules: components with no build step and no
  Node in the toolchain.
- Svelte or Solid with a Vite build whose output gets embedded with
  `include_bytes!`: a nicer authoring experience, at the cost of Node in CI and
  in the release, and a built bundle to keep in sync.
- Vanilla, split into modules and web components: no new dependency, and more
  code written by hand.

The guards stay as they are (token, `Host`, `Origin`), and the page sends a
Content-Security-Policy that allows no outside origin.

## Constraints every item keeps

The rules in [CLAUDE.md](CLAUDE.md) hold for all of the above: a switch changes
only the login's keys, the active (or in-use) login is never refreshed, and a
live login is synced back before anything reads the store. Auto-switching must
never drop an unstored live login the way `remuda use --discard` does.
