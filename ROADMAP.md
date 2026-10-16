# Roadmap

Features remuda may grow next. None is started; order is not priority.
Small fixes and chores go in [TODO.md](TODO.md). When a feature ships, its entry
leaves this file and a doc for it lands in `brain/features/`.

## Auto mode

remuda keeps each CLI on the most suitable of the logins the user made
eligible, and moves it to another when the active one is maxed. Each provider
is decided apart from the others.

**The watcher.** A thread in `remuda serve`, the process that already runs all
the time and already follows TokenGauge's snapshot. It decides again whenever
the snapshot changes, and once a minute so a schedule takes effect on time.
Auto mode works only while `serve` runs.

**The settings.** A config file remuda does not have yet,
`$XDG_CONFIG_HOME/remuda/config.toml`, which the page edits as well as the
user's editor. Each provider has its own section, off by default: whether auto
mode is on, which logins it may pick, its rules, the minimum time on a login
and the margin. Schedules are named once and shared by every provider.
`remuda rename` and `remuda remove` keep the logins named there in step.

```toml
[schedules.workday]
days  = ["mon", "tue", "wed", "thu", "fri"]
hours = "09:00-18:00"   # local time

[auto.claude]
enabled = true
dwell   = "30m"
margin  = 0.25

[[auto.claude.rule]]
when  = "workday"
order = [["work"], ["perso"]]

[[auto.claude.rule]]
when    = "otherwise"
order   = [["perso"], ["work"]]
reserve = { work = "workday" }
```

**The pick.**

1. The rule in force is the first whose schedule matches now; `otherwise`
   matches any time. With no rules, every eligible login sits in one tier.
2. A rule's `order` is a list of tiers. The first tier holding a usable login
   wins; a lower tier is reached only when every login above it is maxed or
   unusable.
3. Within a tier, the score decides: the headroom left in the login's longest
   window divided by the hours until it resets (at least one). Headroom about
   to be lost at a reset is spent first.
4. A login whose 5-hour window is maxed is not usable.
5. A reserved login (`reserve = { work = "workday" }`) is kept for its
   schedule. Outside it, it is usable only when its longest window resets
   before the schedule next starts, since whatever is left would be lost
   anyway, and then it moves to the first tier so it is spent before the rest.

When every eligible login is maxed, auto mode stays where it is, says so on the
page and in its log, and switches once one resets.

**No flapping.** A login stays active for at least `dwell`, and auto mode
leaves it only for a pick that scores `margin` better. Neither holds when the
active login is maxed.

**Figures it can trust.** It does not decide on a stale snapshot or one older
than the last switch. A login whose figures carry an error, or that has none,
sits out that round. After a switch it nudges TokenGauge and waits for the new
snapshot.

**Every switch is still a switch.** Auto mode calls the same `ops::switch` as
`remuda use`, never with `discard`: the live login is synced back first, and an
unstored live login makes it refuse and report rather than drop it. remuda
cannot yet tell a session in the middle of a request apart, so it does not
wait for one.

`remuda auto` prints, per provider, the rule in force, each eligible login's
score and why it was or was not picked, so the rules can be checked before they
are switched on. The page shows the same.

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
remuda gains on the way: the refresh timer, the hub's bind address and
satellites. Auto mode brings the config file and the first settings the page
edits; this section grows on what it starts.

The page is already written in Solid under `web/`, which Vite builds into the
one file the binary compiles in, so sections grow there as components.

The guards stay as they are (token, `Host`, `Origin`), and the page sends a
Content-Security-Policy that allows no outside origin.

## Constraints every item keeps

The rules in [CLAUDE.md](CLAUDE.md) hold for all of the above: a switch changes
only the login's keys, the active (or in-use) login is never refreshed, and a
live login is synced back before anything reads the store. Auto-switching must
never drop an unstored live login the way `remuda use --discard` does.
