# Roadmap

Features remuda may grow next. None is started; order is not priority.
Small fixes and chores go in [TODO.md](TODO.md). When a feature ships, its entry
leaves this file and a doc for it lands in `brain/features/`.

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

## Banked resets: what is left

A Claude login's banked resets are on its card, and the page spends one
([brain/features/web-page.md](brain/features/web-page.md)). Still to come:

- **Codex**, which calls them rate limit reset credits (`/usage` in Codex: "View
  account usage or redeem an earned reset"), under
  `https://chatgpt.com/backend-api`. The endpoints were read out of codex-cli
  0.153.4 and get re-read from the binary when a release moves them.
  - Count: `GET /wham/usage` carries `rate_limit_reset_credits.available_count`.
  - List: `GET /wham/rate-limit-reset-credits`. Each credit has a `credit_id`,
    a `reset_type`, `granted_at` and `expires_at`, and a status (`available`,
    `redeeming`, `cooldown_active`).
  - Spend: `POST /wham/rate-limit-reset-credits/consume` with `credit_id` and a
    fresh `redeem_request_id`. The outcome is `reset`, `nothingToReset` or
    `alreadyRedeemed`, along with the windows it reset.

  The field names come from Codex's type names and want checking against a
  real answer before they are relied on. Listing comes from TokenGauge's
  snapshot, as Claude's does, which needs TokenGauge to report the credits (an
  ADR 0003 change); spending is remuda's own call, with the same rules as
  Claude's.
- **The CLI**: `remuda reset <name>`, which asks for confirmation unless given
  `--yes`, since a reset cannot be undone.
- **Auto mode** can weigh banked resets: a login with one about to expire is
  worth spending first.

## A management page

`remuda serve` grows from one page into a small management app, along the lines
of CLIProxyAPI's management center: a sidebar with sections (Dashboard, Quota,
Accounts, Add an account, Settings, Logs) and a settings section for what
remuda gains on the way: the refresh timer, the hub's bind address and
satellites. Auto mode brought the config file and the first settings the page
edits; this section grows on what it started.

The page is already written in Solid under `web/`, which Vite builds into the
one file the binary compiles in, so sections grow there as components.

The guards stay as they are (token, `Host`, `Origin`), and the page sends a
Content-Security-Policy that allows no outside origin.

## Constraints every item keeps

The rules in [CLAUDE.md](CLAUDE.md) hold for all of the above: a switch changes
only the login's keys, the active (or in-use) login is never refreshed, and a
live login is synced back before anything reads the store. Auto-switching must
never drop an unstored live login the way `remuda use --discard` does.
