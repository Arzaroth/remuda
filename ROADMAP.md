# Roadmap

Features remuda may grow next. None is started; order is not priority.
Small fixes and chores go in [TODO.md](TODO.md). When a feature ships, its entry
leaves this file and a doc for it lands in `brain/features/`.

## More providers, at parity with TokenGauge

remuda keeps every provider TokenGauge reads but OpenRouter, whose key is a
management key as often as an inference one. What is left:

- `remuda run` for Grok, Kimi and Cursor through each CLI's override variable
  (`GROK_HOME`, `KIMI_CODE_HOME`, `CURSOR_CONFIG_DIR`), so `remuda run
  grok/work` works as it does for Claude Code and Codex
  ([brain/features/run.md](brain/features/run.md)), and an API key handed to
  its CLI in the environment.
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

The proxy on `127.0.0.1` exists ([brain/features/proxy.md](brain/features/proxy.md)):
it spends the active login of whichever CLI a model names, with per-client
keys made on the page. It translates between formats, so any client reaches
any provider. What the second way still needs is the bind address, pairing
and TLS above.

## Banked resets: what is left

A Claude login's banked resets are in its details, and the page spends one
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
of CLIProxyAPI's management center. The page already has a sidebar with an
overview, a page per CLI (its logins and its automation), the API proxy and
status, and a Log; still to come is a settings page for what remuda
gains on the way: the refresh timer, the hub's bind address and satellites.

The page is already written in Solid under `web/`, which Vite builds into the
one file the binary compiles in, so sections grow there as components.

The guards stay as they are (token, `Host`, `Origin`), and the page sends a
Content-Security-Policy that allows no outside origin.

## Constraints every item keeps

The rules in [CLAUDE.md](CLAUDE.md) hold for all of the above: a switch changes
only the login's keys, the active (or in-use) login is never refreshed, and a
live login is synced back before anything reads the store. Auto-switching must
never drop an unstored live login the way `remuda use --discard` does.
