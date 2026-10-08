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

`remuda run <profile>` starts a CLI session on one stored login without
switching the one every other session uses. The profile names the CLI as well:
names resolve the way they do everywhere else (`commands::resolve`), so a bare
name that only one provider has picks that provider, and with it which CLI to
launch and how to set it up. When several providers have the name, remuda asks
for the provider, which `remuda <cli> <profile>` gives (`remuda claude work`,
`remuda codex work`), as does the qualified `remuda run codex/work`. Anything
after `--` goes to the CLI unchanged.

- **Claude Code:** a temporary `CLAUDE_CONFIG_DIR` whose entries are symlinks to
  the real config, so history, settings and MCP logins stay shared, except for
  the credentials, which are that login's own.
- **Codex:** the same through a temporary `CODEX_HOME`, with `auth.json` being
  the login's own and everything else linked to `~/.codex`.

While the session runs, its login is in use (flock, or a check that the process
is alive) and remuda treats it like the active credential: never refreshed, and
its tokens synced back to the store when the session ends, since the CLI may
have rotated them.

## More providers, at parity with TokenGauge

remuda keeps every provider TokenGauge reads but OpenRouter, whose key is a
management key as often as an inference one. What is left:

- `remuda run` support through each CLI's override variable, so `remuda kimi
  work` and `remuda grok work` work like `remuda claude work`, and an API key
  is handed to the CLI in its environment.
- OpenRouter, once it is settled which of its two keys a stored credential is.

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

List the limit resets each Claude and Codex login has banked, with their expiry,
and spend one from remuda (the CLI and the page). Both providers expose them;
the endpoints below were read out of Claude Code 2.1.289 and codex-cli 0.153.4,
and get re-read from the binary when a release moves them.

**Claude** calls them grants, under the `cedar_ember` program (`/limit-reset` in
Claude Code, "Limit resets" in claude.ai's settings).

- List: `GET /api/oauth/usage?cedar_ember=1&skip_spend=1` adds a `cedar_ember`
  block to the usage answer: `eligible`, `ineligible_reason`, `at_limit`,
  `exhausted`, `next_grant_id`, `weekly_resets_at`, `cooldown_until` and
  `grants[]`. Each grant has `id`, `label`, `resets_total`, `resets_left`,
  `starts_at`, `ends_at` (its expiry), `clears` (the windows it resets),
  `paused`, `usable_now`, `use_requires_limit`, `percent_used` and `blocking`.
- Spend: `POST /api/organizations/<organizationUuid>/reset_rate_limits` with
  `{"program": "cedar_ember", "grant_id", "request_id"}`, where `request_id`
  is a fresh id (`[A-Za-z0-9_-]{1,64}`) so a retried request is not spent
  twice. It answers `result` (`reset`, `already_used`, `not_limited`,
  `cooldown`, `ineligible`, `unavailable`), `reason`, `resets_left`, `cleared`,
  `weekly_resets_at` and `cooldown_until`. The organization comes from the
  sidecar's `oauthAccount`, and the token needs the `user:profile` scope.

**Codex** calls them rate limit reset credits (`/usage` in Codex: "View account
usage or redeem an earned reset"), under `https://chatgpt.com/backend-api`.

- Count: `GET /wham/usage` carries `rate_limit_reset_credits.available_count`.
- List: `GET /wham/rate-limit-reset-credits`. Each credit has a `credit_id`, a
  `reset_type`, `granted_at` and `expires_at`, and a status (`available`,
  `redeeming`, `cooldown_active`).
- Spend: `POST /wham/rate-limit-reset-credits/consume` with `credit_id` and a
  fresh `redeem_request_id`. The outcome is `reset`, `nothingToReset` or
  `alreadyRedeemed`, along with the windows it reset.

Codex's field names come from its type names and want checking against a real
answer before they are relied on.

Listing belongs with the rest of the usage figures. TokenGauge already calls
both usage endpoints, so the cheapest route is TokenGauge adding
`cedar_ember=1` and the reset credits to its snapshot (an ADR 0003 change), and
remuda reading them from there. Spending is remuda's own call, made with the
stored credential's access token. For the active login that is the live token,
which remuda reads and never refreshes. An inactive login whose access token
has expired is refreshed first, the usual way. A reset cannot be undone, and
a grant marked `use_requires_limit` answers `not_limited` until a window is
maxed, so the page asks for a second click and shows what the grant clears,
and the CLI (`remuda reset <name>`) asks for confirmation unless given `--yes`.
Auto mode can weigh banked resets too: a login with one about to expire is
worth spending first.

## A management page

`remuda serve` grows from one page into a small management app, along the lines
of CLIProxyAPI's management center: a sidebar with sections (Dashboard, Quota,
Accounts, Add an account, Settings, Logs) and a settings section for what
remuda gains on the way: auto-switch and which logins are eligible, the
refresh timer, the hub's bind address and satellites.

Settings mean remuda gets a config file it does not have today
(`$XDG_CONFIG_HOME/remuda/config.toml`), shared by the CLI and the page.

The page is already written in Solid under `web/`, which Vite builds into the
one file the binary compiles in, so sections grow there as components.

The guards stay as they are (token, `Host`, `Origin`), and the page sends a
Content-Security-Policy that allows no outside origin.

## Constraints every item keeps

The rules in [CLAUDE.md](CLAUDE.md) hold for all of the above: a switch changes
only the login's keys, the active (or in-use) login is never refreshed, and a
live login is synced back before anything reads the store. Auto-switching must
never drop an unstored live login the way `remuda use --discard` does.
