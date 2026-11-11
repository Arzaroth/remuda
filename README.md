<p align="center"><img src="assets/remuda.svg" width="112" alt=""></p>

# remuda

A remuda is the herd of spare horses a rider picks a fresh mount from each day,
while the rest recover. This one holds Claude Code, Codex, Grok, Kimi Code and
Cursor logins, and GLM, opencode Go, DeepSeek, MiniMax, OpenRouter and Copilot
API keys.

Keep several accounts per CLI and switch the one it uses, without logging out
and back in and without a separate `CLAUDE_CONFIG_DIR` or `CODEX_HOME`.
History, plans, settings and MCP server logins stay shared; only the account
changes.

```
$ remuda ls
Claude Code
* work (Job)  me@work.example  ultra     token in 5h, refresh in 29d
  perso       me@example.com   max       token in 2h, refresh in 12d

Codex
* main        me@example.com   plus      token in 9d

$ remuda use perso
switched Claude Code to perso (me@example.com)
```

## Install

Linux, x86_64 or aarch64:

```sh
curl -fsSL https://raw.githubusercontent.com/Arzaroth/remuda/master/scripts/install.sh | bash
```

This downloads the release archive for your architecture, checks it against
the release's `SHA256SUMS`, installs `remuda` into `~/.local/bin`, enables
`remuda-refresh.timer`, which keeps the inactive logins' tokens fresh every 30
minutes, and installs completions for bash, zsh and fish when it finds them.
Pass `--no-timer`, `--no-completions`, or `--version vX.Y.Z` to pin a release
(0.19.0 or later; `... | bash -s -- --no-timer`). `--serve` also enables
`remuda-serve.service`, which keeps [the page](#the-page) running for
`remuda open`. Afterwards, `remuda --update` replaces the binary
with the latest release and brings the systemd units you installed up to date.

If your shell sets `CLAUDE_CONFIG_DIR`, `CODEX_HOME` or `REMUDA_STORE`, the
installer copies them to `~/.config/environment.d/60-remuda.conf` so the timer
looks where you do. If you change `CLAUDE_CONFIG_DIR` or `CODEX_HOME` later,
the timer notices: it compares its view with where your last `remuda` command
found each CLI, and refuses to refresh one it would look for in the wrong
place, saying what to set in that file (`journalctl --user -u remuda-refresh`).

### Packages

Each release has a `.deb`, an `.rpm` and an Arch package for x86_64 and
aarch64. They install the binary into `/usr/bin`, the completions, and the
units, which are yours to enable:

| Distribution | Install |
|---|---|
| Debian, Ubuntu | `sudo apt install ./remuda_X.Y.Z_amd64.deb` |
| Fedora, RHEL, openSUSE | `sudo dnf install ./remuda-X.Y.Z-1.x86_64.rpm` (`sudo zypper install` on openSUSE) |
| Arch | `sudo pacman -U ./remuda-bin-X.Y.Z-1-x86_64.pkg.tar.zst`, or `makepkg -si` from the PKGBUILDs in `remuda-vX.Y.Z-pkgbuild.tar.gz` (`remuda` builds from source, `remuda-bin` repackages the release) |

```sh
remuda ls                                         # records where each CLI keeps its login
systemctl --user enable --now remuda-refresh.timer
systemctl --user enable --now remuda-serve.service   # optional, for `remuda open`
```

A package does not write `environment.d` for you: if your shell sets
`CLAUDE_CONFIG_DIR`, `CODEX_HOME`, `REMUDA_STORE` or another CLI's directory,
put the same lines in `~/.config/environment.d/60-remuda.conf` before enabling
the timer. After a package upgrade, restart `remuda-serve.service` if it runs.

The `.rpm`s, the Arch packages (a `.sig` beside each) and `SHA256SUMS` (in
`SHA256SUMS.asc`) are signed with the release key
[`packaging/remuda.asc`](packaging/remuda.asc), fingerprint
`0630256DBF85D5A91C9ED7AD18B546E18A0C5512`. Trust it once:

```sh
curl -fsSL https://raw.githubusercontent.com/Arzaroth/remuda/master/packaging/remuda.asc -o remuda.asc
gpg --import remuda.asc && gpg --verify SHA256SUMS.asc SHA256SUMS   # any distribution
sudo rpm --import remuda.asc                                         # Fedora, openSUSE
sudo pacman-key --add remuda.asc && sudo pacman-key --lsign-key 0630256DBF85D5A91C9ED7AD18B546E18A0C5512  # Arch
```

The binary is static, so the packages run on anything from Debian 12 and
Ubuntu 22.04 up. A packaged binary upgrades through its package manager:
`remuda --update` refuses to replace it.

Moving from an `install.sh` install to a package, remove `~/.local/bin/remuda`
and `~/.config/systemd/user/remuda-*` first: both come ahead of the package's
copies, and the units there run `~/.local/bin/remuda`.

## Commands

A credential is addressed by its name when only one CLI has that name, and as
`<cli>/<name>` (`claude/work`, `grok/perso`) otherwise. Commands that create one
take `-p <cli>`: `claude` (the default), `codex`, `copilot`, `cursor`,
`deepseek`, `glm`, `grok`, `kimi`, `minimax`, `opencode` or `openrouter`.

| Command | Does |
| --- | --- |
| `remuda import <name> [-p <cli>]` | Store the login the CLI is signed into right now |
| `remuda login <name> [-p <cli>] [--no-browser] [--browser <id>] [--window private\|profile\|normal] [--paste\|--no-paste]` | Sign another account in through the browser and store it, without touching the CLI. Prints the link first; Enter opens it where `[login]` in config.toml says, unless `--no-browser` is given or there is no display (`DISPLAY` or `WAYLAND_DISPLAY`), when you open it yourself. Claude and Codex call back to this machine; `--paste` lets a browser elsewhere finish, with a code to paste (Claude) or a device code (Codex), the default over SSH or, on Linux, with no display, and `--no-paste` asks for the callback anyway |
| `remuda use <name>` | Make a stored credential the one its CLI uses |
| `remuda export [file] [--move <name>] [--without-s3-keys]` | Send this machine's logins and settings to another one, through wyrmyon or magic-wormhole, or sealed into a file under a passphrase |
| `remuda restore [file] [--force] [--no-sign-in] [--paste\|--no-paste]` | Set this machine up from what `export` sent, then sign in again the logins that cannot be copied |
| `remuda signout <cli>` | Sign the CLI out, keeping its login in the store, so remuda can refresh and sign in again every login |
| `remuda run <name> [-- args]` | Start Claude Code or Codex on a stored login for this session only, without switching; `remuda claude <name>` is the same |
| `remuda pin [name] [--remove <cli>] [--allow]` | Pin a login to this directory in `.remuda.toml`, show the pins in force, or trust a `.remuda.toml` as it is |
| `remuda shell-init <shell>` | Print functions that start `claude` and `codex` on the login pinned where they run |
| `remuda env <name>` | Print the `export` line that selects a stored API key in a shell |
| `remuda` | Show every stored login, the one each CLI uses (`●`), and when its credential file was last written |
| `remuda ls [--json]` | List stored credentials, marking the active ones |
| `remuda refresh [name] [--force] [--within MIN]` | Refresh the inactive credentials expiring within `MIN` minutes (default 60) |
| `remuda label <name> [text]` | Set the label shown beside a credential, or clear it |
| `remuda rename <name> <new>` | Rename a stored credential |
| `remuda rm <name>` | Delete a stored credential |
| `remuda auto` | Say which login auto mode would put each CLI on now, and why |
| `remuda log [name] [-n N] [--json]` | Show what remuda did: switches and who made them, refreshes, auto mode, keep-warm, the proxy |
| `remuda doctor [--json]` | Check what remuda relies on (the service, the refresh timer, TokenGauge, the logins, the CLIs' versions) and say how to fix what is wrong |
| `remuda serve [--port N] [--no-browser]` | Do all of the above from a page in the browser, and run auto mode |
| `remuda open [--no-browser] [--add]` | Open the page a running `serve` shows; `--add` opens it on the add-account sheet |
| `remuda help skill` | Print a SKILL.md that tells Claude Code sessions how to live with remuda, with the artifact hook's settings |
| `remuda completions <shell>` | Print a completion script |
| `remuda --update` | Replace the binary with the latest release |
| `remuda --check-update` | Report whether a newer release exists |

`login` prints the provider's sign-in link first, so you choose the browser.
Enter opens it where `[login]` says (`--browser` and `--window` for one run):
by default a private window of your default browser, so the account you are
signed into there stays put. remuda knows how to open Brave, Chrome, Chromium,
Vivaldi, Edge, Firefox and LibreWolf, not their Snap or Flatpak packages; any
other browser gets the link through `xdg-open`, in a normal window, where it
may already be signed in. Sign in with the account to add. Claude and Codex call back to remuda, so their
browser must be on this machine, and Codex's port 1455 must be free (not while
`codex login` runs); with `--paste`, Claude shows a `code#state` string to
paste back and Codex a device code to type in instead, from any machine's
browser. Over SSH (`SSH_CONNECTION` or `SSH_TTY` set), or on Linux with
neither `DISPLAY` nor `WAYLAND_DISPLAY`, both do so by default; `--no-paste`
asks for the callback anyway, as when port forwarding brings it back. Grok and
Kimi show a code to approve and Cursor a confirmation, and remuda waits for the
provider to say it is done, from any machine's browser. The API key providers
open their key page and ask for the key; Copilot asks for a GitHub token
(`gh auth token` prints yours), MiniMax then for its region (`global` or
`cn`), and OpenRouter for an optional management key, which TokenGauge needs
to show the account balance.

## Another machine

`remuda export` on this machine and `remuda restore` on the new one copy the
logins, `config.toml`, the proxy's settings (without its client keys) and
TokenGauge's fleet, S3 keys included unless `--without-s3-keys`, so the new
machine joins the fleet on its own. That needs TokenGauge on both machines,
with `--sync-export` on this one and `--sync-join` on the new one; when
TokenGauge or its fleet key is missing here, export says so and goes on
without the fleet. With wyrmyon or magic-wormhole installed,
export prints a code and restore asks for it; otherwise `remuda export
<file>` seals everything into a file in age's format, under a passphrase, and
`remuda restore <file>` opens it.

API keys and Cursor logins go across as they are. Claude Code, Codex, Grok and
Kimi logins do not: their refresh tokens are single use, so two machines
sharing one would sign each other out. restore signs each in again instead,
the way `remuda login` does (from a browser on any machine over SSH or, on
Linux, with no display, or with `--paste`), and stores it only if it is the
same account.
`--move <name>` carries a login's tokens anyway and drops it here, for a
machine you are retiring; its tokens are set aside here first, and it stays
when a refresh changed them while the bundle was on its way. restore keeps
the logins already here; `--force` replaces them, but never the one a CLI or
a `remuda run` session uses, nor one it cannot read, and never the proxy's
client keys (proxy settings it cannot read stay as they are). Tokens restore
could not store, and those a login here held when `--force` replaced it, go
to a set-aside file whose path it prints (an API key it keeps out is not set
aside, since the sending machine still has it; one it replaces is). A
login, config.toml or the proxy's settings it cannot write is reported and the
rest restored, the sign-ins included. A bundle received over a wormhole is
kept in the store for another try, as a `.received-<ms>.json` file whose path
restore prints, in two cases only. When restore stops on an error (what was
received is not a bundle it reads, or restoring fails, its lock on the store
not taken for one), it says where the bundle is kept, or that it is lost when
the store cannot hold it, then fails with that error. When tokens it carried
could be neither stored nor set aside, it fails after the sign-ins naming those
logins and where their tokens are: the kept bundle, or, if the store could not
hold it, the sending machine's store or set-aside file. Anything else keeps no
bundle, a config.toml or proxy settings it cannot write included; a file given
to `restore` stays as it is, and is where such tokens are. A login
of a provider the new machine's remuda does not know is skipped without being
set aside there, so keep the set-aside file `--move` printed here until that
login works over there.

## A login per session, and per repository

`remuda run work` starts Claude Code on `work` without switching what every
other terminal uses; `remuda codex perso -- exec "..."` does the same for
Codex. The session gets a `CLAUDE_CONFIG_DIR` (or `CODEX_HOME`) of its own
under the store. Everything in your real one is linked into it, so history,
projects, settings and MCP servers stay shared. Only the files holding the
login are its own: for Claude Code, copies of `.credentials.json` and
`.claude.json` holding that login. A second terminal on the same login joins
the session. When the last one exits, the tokens the CLI refreshed go back to
the store once remuda has confirmed whose they are, and what else it changed
in those copies is merged back into yours. Offline, the session's directory
stays under the store, holding the login, until a later command can confirm
them. While it runs, that
login is left alone: not refreshed, switched to, removed or pinged.

Pin a login to a repository and every `claude` started there uses it:

```bash
cd ~/src/job && remuda pin claude/work      # writes .remuda.toml
eval "$(remuda shell-init zsh)"             # in ~/.zshrc
claude                                      # Claude Code on work, here only
```

Elsewhere, `claude` runs as it always did. A `.remuda.toml` that `remuda pin`
did not write, one that came with a clone say, is ignored until `remuda pin
--allow` trusts it.

## Claude Code artifacts

A switch moves every open Claude Code session to the other account, and an
artifact belongs to the account that published it: after a switch, a session
can no longer update its own page. remuda's artifact hook fixes that. It notes
which login publishes or lists each artifact, and for a call on one that
another login published, it switches Claude Code to that login, then back once
the call returns. Every open session is on that login while the call runs.

```bash
mkdir -p ~/.claude/skills/remuda   # or wherever your skills live
remuda help skill > ~/.claude/skills/remuda/SKILL.md
```

The skill tells sessions what a switch does to them, and holds the hook lines
to add to `~/.claude/settings.json`. `remuda doctor` says whether they are in.

## The page

`remuda serve` listens on `127.0.0.1:7429` and opens a page listing each CLI's
credentials, with use, refresh, label, rename and remove, an import button for
a live login nobody stored yet, and sign-in for a new account. It also shows how
long each credential's tokens have left, what `remuda doctor` finds (the
service, the refresh timer and its last run, the logins that need signing in
again), and, when
[TokenGauge](https://github.com/Arzaroth/TokenGauge) is installed, each login's
usage, read from TokenGauge's snapshot (only the login in use before
TokenGauge 0.37). A Quota view pools every login of a CLI into one
allowance and charts it over two weeks: what was left as TokenGauge recorded
it (0.54 or later), and where it heads at the burn of the last 1, 3 or 6
hours, each reset filling its login back up, with each login's own burn
and when it runs dry beside it. A Versions view sets the installed remuda and
TokenGauge beside their latest releases and updates either. It keeps itself current: a change made from the CLI, by the
refresh timer or by a TokenGauge fetch shows within a couple of seconds.

Its Usage tab shows what was spent, per CLI, model and login, priced by
TokenGauge (0.52 or later, which remuda runs for it), and what the API proxy
answered: requests, outcomes, speed, latency and tokens per model, login and
key. Spend is put to a login from what remuda records as it switches and runs
CLIs, so it starts from when you install this version.
Its Value view turns that into what a week of each login bought: API dollars
per percent of the weekly allowance, week by week, so a plan whose limits were
raised or cut shows as a step (TokenGauge 0.56 or later, which records each
week as it closes).

The URL it prints carries an access token in its fragment. Every request must
send that token back and name this listener in its `Host` and `Origin`, so
another site open in the same browser cannot drive it.

`remuda open` opens the page a running `serve` shows, without the printed
URL. Installed with `--serve`, `remuda-serve.service` keeps one running (or
`open` starts it), and the token never reaches its journal.

## Auto mode

While `remuda serve` runs, auto mode keeps each CLI on the most suitable of the
logins you allow and moves it off one that is full, using the usage figures
[TokenGauge](https://github.com/Arzaroth/TokenGauge) reports. It is off by
default; set it up on the Automation tab of each CLI's page, or in
`~/.config/remuda/config.toml`:

```toml
[schedules.workday]
days  = ["mon", "tue", "wed", "thu", "fri"]
hours = "09:00-18:00"

[auto.claude]
enabled = true

[[auto.claude.rule]]          # during work hours: work, then perso once work is full
when  = "workday"
order = [["work"], ["perso"]]

[[auto.claude.rule]]          # otherwise: perso; work only when its week resets before work hours
when    = "otherwise"
order   = [["perso"], ["work"]]
reserve = { work = "workday" }
```

A strict priority with room to spare before each limit, as a quota balancer
would keep it:

```toml
[auto.claude]
enabled  = true
dwell    = "1m"
ceiling  = 97                        # every window counts as full at 97%
ceilings = { IDC = 93, Axeo = 88 }   # their own, for their 5-hour window
fallback = "perso"                   # when every login is full, the active one too

[[auto.claude.rule]]
when  = "otherwise"
order = [["IDC"], ["Axeo"], ["perso"]]
```

To spend one login only up to a point, give it a ceiling of its own for its
other windows (the week, for Claude), and send the CLI back elsewhere once
it gets there, even onto a login that is maxed:

```toml
[auto.claude]
enabled             = true
long_ceilings       = { perso = 94 }  # perso's week is full at 94%
fallback            = "Axeo"
fallback_when_maxed = true            # go to Axeo even when it is maxed

[[auto.claude.rule]]
when  = "otherwise"
order = [["Axeo"], ["perso"]]
```

Logins are used in the order listed: the first one under its ceiling wins,
and the CLI goes back to an earlier one once it has room again. With
`balance = true`, the logins of a tier are ranked instead by the headroom
they would lose at their next reset, and a switch between them needs a 25%
better pick (`margin`). Unless the login in use is full, a switch waits at
least 30 minutes after the last one (`dwell`). Codex is
switched only while no Codex session runs. `remuda auto` shows what it would
do and why.

## Keep-warm

A 5-hour window only starts at its first message. Keep-warm reopens a login's
window with the smallest request it takes as soon as it closes, so one is always running
and the next reset keeps coming: a 7am ping gives you an hour of the 7-12
window, then a fresh one at noon. Pick the logins on the Automation tab of each
CLI's page, or in `config.toml`:

```toml
[warm]
claude = ["work", "perso"]
codex  = ["work"]
```

It needs `remuda serve` and TokenGauge, and works for any login TokenGauge
reports a 5-hour window for. The login a CLI is on is pinged with the CLI's
own token and never refreshed by remuda.

## The proxy

`remuda serve` can also run an API proxy for any OpenAI or Anthropic client,
switched on from the API proxy page, which also makes its access keys.

| Protocol | Base URL |
| --- | --- |
| OpenAI | `http://127.0.0.1:8317/v1` |
| Claude | `http://127.0.0.1:8317` |

```sh
ANTHROPIC_BASE_URL=http://127.0.0.1:8317 ANTHROPIC_API_KEY=rmd-... claude
```

The page's Shell field copies the matching `export` lines for the protocol
and key it shows.

The model name picks the CLI whose login answers (`claude-*` Claude Code,
`gpt-*` Codex, `grok-*`, `kimi-*`, `glm-*`, or `provider/model`, which opencode
Go needs), and the request spends that CLI's active login: `remuda use`
switches the proxy too. The proxy never refreshes that login, so once its
access token expires, run the CLI once or switch. Any format reaches any CLI:
remuda translates where the CLI's endpoint speaks another, so Claude Code runs
on a Codex login with `--model gpt-5` and an OpenAI SDK on a Claude login. It
listens on `127.0.0.1` unless "Reachable from other machines" is switched on,
which listens on every interface and shows the LAN address in the base URLs.
Traffic is plain HTTP, keys included. When ufw or firewalld blocks the port,
the card shows the command that opens it (`sudo ufw allow in on eth0 to any port 8317 proto tcp`),
after `sudo ufw status numbered` when a deny for some machines may still refuse them.

## How a switch works

For Claude Code, a switch rewrites two keys and nothing else: `claudeAiOauth` in
`~/.claude/.credentials.json` and `oauthAccount` in `~/.claude.json`. Both honour
`CLAUDE_CONFIG_DIR`. Running Claude Code sessions watch those files and adopt
the new login.

For Codex, the whole of `~/.codex/auth.json` belongs to the login, so a switch
replaces it. It honours `CODEX_HOME`. Only ChatGPT sign-ins are stored: an API
key or personal access token has no account to switch between, and `use` asks
for `--discard` before replacing one. A Codex account is a seat: two people in
one ChatGPT Team workspace are two accounts.

For Grok (`~/.grok/auth.json`, honouring `GROK_HOME` and `GROK_AUTH_PATH`),
Kimi (`~/.kimi-code/credentials/kimi-code.json`, honouring `KIMI_CODE_HOME`)
and Cursor (`cursor-agent`'s `~/.config/cursor/auth.json`, honouring
`CURSOR_CONFIG_DIR`), the whole file is stored, as for Codex. A Grok switch
replaces the x.ai sign-in and keeps any API key the file holds beside it.
Cursor's IDE keeps its own login elsewhere, which remuda leaves alone. Kimi's
file names nobody, so remuda asks kimi.com whose login it is; offline, it
cannot tell which stored Kimi login is live, and `refresh` and `rm` leave Kimi
alone until it can.

An API key has no account behind it and never expires. opencode 2 keeps
opencode Go's key in its database, `~/.local/share/opencode/opencode.db`
(under `$XDG_DATA_HOME` when set, or the file `OPENCODE_DB` names), as one of
the accounts `opencode auth list` shows. A switch makes the stored key the active
one there, adding it when opencode does not hold it yet, and touches no other
provider's login. opencode 1, and opencode 2 before it has imported
`auth.json`, keep it in `auth.json` in opencode's data directory, and a switch
rewrites the `opencode-go` entry there and nothing else.
GLM reads `Z_AI_API_KEY` and nothing else, and a program cannot change a
running shell's environment, so `use` says to run
`eval "$(remuda env glm/<name>)"` instead. The same goes for opencode when
`OPENCODE_API_KEY` or `OPENCODE_GO_API_KEY` is set, since it wins over the
stored key.
DeepSeek and MiniMax are switched in opencode's store the same way, in its
`deepseek` entry and in the MiniMax entry of the login's region (`minimax`,
`minimax-cn`, or their `-coding-plan` forms); a switch from one region to the
other moves the key out of the old region's entry. `DEEPSEEK_API_KEY` and
`MINIMAX_API_KEY` win over them. OpenRouter (`OPENROUTER_API_KEY`) and Copilot
(`GH_TOKEN`, which the GitHub and Copilot CLIs read) are switched by
`remuda env` only; Copilot also recognises the token `gh auth token` prints
as the live login, and never writes it.

Each CLI rotates the refresh token of the account it is signed into, so before
every command remuda copies the live login back into the credential it belongs
to. It matches on the tokens first and falls back to the account id. Claude's
account id lives in a different file from its tokens, so remuda confirms it
with the provider before trusting it; Codex keeps both in `auth.json`. remuda
refuses to switch away from a login that is not stored (`--discard` overrides),
and it never refreshes an active credential: the CLI owns that one.

## The store

`$REMUDA_STORE`, else `$XDG_DATA_HOME/remuda/credentials`, else
`~/.local/share/remuda/credentials`:

```
claude/<name>.json        same shape as .credentials.json, claudeAiOauth only
codex/<name>.json         same shape as auth.json
grok/<name>.json          same shape as auth.json
kimi/<name>.json          same shape as kimi-code.json
cursor/<name>.json        same shape as cursor-agent's auth.json
glm/<name>.json           {"key": ...}
opencode/<name>.json      {"key": ...}
deepseek/<name>.json      {"key": ...}
minimax/<name>.json       {"key": ..., "region": "global" or "cn"}
openrouter/<name>.json    {"key": ..., "management": ...}, management optional
copilot/<name>.json       {"key": ...}, a GitHub token
<cli>/<name>.meta.json    accountId, email, capturedAt, label, the
                          credential's credsDigest, and Claude's
                          oauthAccount block
.dirs.json                where the last interactive command found each
                          CLI's login, for the refresh timer
.runs/<cli>/<name>/       a `remuda run` session's directory, until its
                          tokens are back in the store
.pins.json                the .remuda.toml files allowed, with their digests
.log.jsonl                what remuda did, one event per line, for
                          `remuda log` and the page's Log; it moves to
                          .log.1.jsonl at 1 MiB, .log.lock guards both
```

Files are 0600 and directories 0700. Other tools read the store: TokenGauge
shows a meter per stored credential (its ADR 0003 describes the contract).

## Caveats

remuda talks to the same OAuth endpoints and clients as Claude Code 2.1.322,
codex-cli 0.167.0, xAI's grok-build and MoonshotAI's kimi-code. They are not a
public API, and a release of any of them can change them. cursor-agent is
closed source: its refresh endpoint and sign-in are the ones the open-source
Cursor clients agree on, and the first to break. Claude's banked resets are
listed only to a client sending Claude Code's own user agent, which remuda and
TokenGauge do; Anthropic can stop that at any time.

The proxy sends another client's requests on a Claude subscription as if
Claude Code had made them, and a ChatGPT subscription's as Codex. Both
providers' consumer terms allow their subscriptions only in their own CLIs,
and either can refuse such traffic or act on the account. Using it is your
call.

Keep-warm is automated traffic on a subscription too, around the clock. It is
the smallest request the provider takes, but it is not you typing.

## Development

```sh
mise install             # the pinned Rust (with llvm-tools), cargo-llvm-cov, shellcheck, Node, pnpm
(cd web && pnpm install) # the page's dependencies, which cargo builds into the binary
cargo test               # unit tests, plus tests/cli.rs driving the built binary
scripts/coverage.sh      # line coverage via cargo-llvm-cov, worst files first
scripts/coverage.sh --html
```

The `serve` page is a Solid app under `web/`. In `web/`:

```sh
pnpm test                # Vitest
pnpm build               # type-checks and builds the page into web/dist
pnpm dev                 # live reload against a running `remuda serve`
```

Nothing in the test suite reaches the network or a real CLI install. The OAuth
calls run against a local mock server, a Codex sign-in is completed through its
callback listener on an ephemeral port, and `tests/cli.rs` runs the binary
against a throwaway `HOME`. What stays uncovered on purpose is the process
surface in `main.rs`: `remuda --update` talking to GitHub, `login` reading stdin,
`serve` binding its port, and `browser.rs` starting a browser.
