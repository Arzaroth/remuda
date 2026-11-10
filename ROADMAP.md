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

The proxy exists ([brain/features/proxy.md](brain/features/proxy.md)):
it spends the active login of whichever CLI a model names, with per-client
keys made on the page, and can listen on every interface, warning that
traffic is in clear and showing the firewall commands that open its port. It
translates between formats, so any client reaches any provider. What the
second way still needs is TLS, a satellite-side remuda that sets its
CLIs up, and a choice of LAN interface on a machine with several: today the
address shown and the ufw rule follow the default route.

## Setting up another machine

Give a new machine the same accounts and settings as this one, and enrol it in
TokenGauge's fleet sync, in one step. It needs no browser on the new machine
and sends no password to it. Keeping machines in step afterwards is not this
item: that is hub and satellites.

`remuda export` builds one bundle and hands it to whichever transfer tool is
installed, rather than remuda carrying its own:

- **wyrmyon** (`wyrm`) first, then the Python **magic-wormhole** (`wormhole`):
  `export` sends the bundle and prints the code, and `remuda restore` on the
  new machine asks for the code and receives it. The code goes to the tool on
  stdin (wyrm reads a piped code), never on a command line. The wormhole is
  already sealed end to end under the code, so the bundle needs no
  passphrase; it lands in a private directory and is deleted once applied.
- **A file**, when neither is installed: `export <file>` seals it with a
  passphrase read from the terminal or a private file, in age's format so the
  `age` CLI can open it too, and `restore <file>` opens it. The file moves
  however the user likes.

What the bundle carries, and how each part lands:

- **API keys** (GLM, opencode) as stored, byte for byte, so `credsDigest` and
  `accountId` still hold. A key works from any machine.
- **Cursor logins** as they are. Cursor's refresh answers with an access token
  and no new refresh token, and the refresh token is a long-lived JWT, so two
  machines can both keep refreshing (read from open-source clients that call
  it, since cursor-agent is closed).
- **Claude Code, Codex, Grok and Kimi logins as a list, signed in again.**
  Their refresh is single use: two machines holding one token race, and the
  first refresh signs the other out. Kimi kills the old token at once; Grok's
  server keeps it about a minute, then treats a second use as theft and
  revokes the new token too, signing out both machines (grok-build,
  `xai-grok-login/src/oidc/refresh.rs`). So `restore` starts each sign-in on
  the new machine and prints its URL and a QR code, to approve from any
  browser: Grok, Kimi and Codex use a device code and the new machine polls;
  Claude's `code#state` is pasted back. Each login is filed only once
  `Provider::identify` names the expected account. The new machine gets its
  own session, and this one keeps its own. `export --move <name>` carries a
  login's tokens instead, for a machine being retired: it leaves this store
  once the bundle is sent, and the active login is refused.
- **`config.toml`** whole: schedules, auto mode, keep-warm. Logins keep their
  names, so the names in it still resolve; `Config::check` runs on it before it
  is written, and a config already there is not replaced without `--force`.
- **The proxy's settings**, without its client keys: a client points at one
  host and gets a new key there.
- **Fleet sync**, with everything the new machine needs to join on its own:
  the fleet key, the `[sync]` transport (`transport`, `[sync.dir] path` with
  this home rewritten to the new one, `[sync.s3]`, `[sync.providers]`,
  `retention_days`) and the S3 access keys. `restore` hands all of it to
  TokenGauge on stdin and runs `tokengauge --sync-test`; remuda never writes
  TokenGauge's files itself. Never the device id, which would make two
  machines write one object and break the single-writer rule, and never
  `label`, which names a device. A machine already in another fleet is refused
  without `--force`, since joining drops that fleet's peers. The S3 keys go
  across by default; `export --without-s3-keys` leaves them out.

Never carried: the log, the attribution ledger and the proxy's request log (this
machine's history, which TokenGauge bills by), `.dirs.json`, `.pins.json`
(paths on this machine; allowing a pin again is one prompt), `.runs/`, the
`.last-*` records and `.set-aside-*` files.

It needs, first:

- **From TokenGauge**, a join that takes the transport as well as the key
  (`--sync-join -` reading both) and an export of the same, so each side only
  touches its own files; and S3 keys kept in a private file beside
  `tokengauge-sync-key` (mode 0600) rather than read from `AWS_ACCESS_KEY_ID`
  and `AWS_SECRET_ACCESS_KEY`. Those come from `environment.d` today, which
  puts the keys in every process's environment, has no counterpart on macOS
  or Windows, and claims the names every other S3 client reads, so no other
  tool on the machine can use different keys.

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
