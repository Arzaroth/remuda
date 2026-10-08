<p align="center"><img src="assets/remuda.svg" width="112" alt=""></p>

# remuda

A remuda is the herd of spare horses a rider picks a fresh mount from each day,
while the rest recover. This one holds Claude Code, Codex, Grok, Kimi Code and
Cursor logins, and GLM and opencode Go API keys.

Keep several accounts per CLI and switch the one it uses, without logging out
and back in and without a separate `CLAUDE_CONFIG_DIR` or `CODEX_HOME`.
History, plans, settings and MCP server logins stay shared; only the account
changes.

```
$ remuda ls
Claude Code
* work (Job)  me@work.example  max 20x   token in 5h, refresh in 29d
  perso       me@example.com   max 5x    token in 2h, refresh in 12d

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

This installs `remuda` into `~/.local/bin`, enables `remuda-refresh.timer`,
which keeps the inactive logins' tokens fresh every 30 minutes, and installs
completions for bash, zsh and fish when it finds them. Pass `--no-timer`,
`--no-completions`, or `--version vX.Y.Z` to pin a release
(`... | bash -s -- --no-timer`). `--serve` also enables
`remuda-serve.service`, which keeps [the page](#the-page) running for
`remuda open`. Afterwards, `remuda update` replaces the binary
with the latest release and brings the systemd units you installed up to date.

If your shell sets `CLAUDE_CONFIG_DIR`, `CODEX_HOME` or `REMUDA_STORE`, the
installer copies them to `~/.config/environment.d/60-remuda.conf` so the timer
looks where you do. If you change `CLAUDE_CONFIG_DIR` or `CODEX_HOME` later,
the timer notices: it compares its view with where your last `remuda` command
found each CLI, and refuses to refresh one it would look for in the wrong
place, saying what to set in that file (`journalctl --user -u remuda-refresh`).

## Commands

A credential is addressed by its name when only one CLI has that name, and as
`<cli>/<name>` (`claude/work`, `grok/perso`) otherwise. Commands that create one
take `-p <cli>`: `claude` (the default), `codex`, `cursor`, `glm`, `grok`,
`kimi` or `opencode`.

| Command | Does |
| --- | --- |
| `remuda import <name> [-p <cli>]` | Store the login the CLI is signed into right now |
| `remuda login <name> [-p <cli>] [--no-browser]` | Sign another account in through the browser and store it, without touching the CLI |
| `remuda use <name>` | Make a stored credential the one its CLI uses |
| `remuda env <name>` | Print the `export` line that selects a stored API key in a shell |
| `remuda ls [--json]` | List stored credentials, marking the active ones |
| `remuda refresh [name] [--force] [--within MIN]` | Refresh the inactive credentials expiring within `MIN` minutes (default 60) |
| `remuda label <name> [text]` | Set the label shown beside a credential, or clear it |
| `remuda rename <name> <new>` | Rename a stored credential |
| `remuda rm <name>` | Delete a stored credential |
| `remuda serve [--port N] [--no-browser]` | Do all of the above from a page in the browser |
| `remuda open [--no-browser]` | Open the page a running `serve` shows |
| `remuda completions <shell>` | Print a completion script |
| `remuda update [--check]` | Replace the binary with the latest release |

`login` opens the provider's sign-in page in a private window of your default
browser (Brave, Chrome, Chromium, Vivaldi, Edge, Firefox or LibreWolf, not
their Snap or Flatpak packages), so the account you are signed into there stays
put; with another browser, open it in a private window yourself. Sign in with the account to add. Claude
then shows a `code#state` string to paste back; Codex calls back to
`localhost:1455` on its own, so run it on the machine whose browser you use,
and not while `codex login` is running. Grok and Kimi show a code to approve
and Cursor a confirmation, and remuda waits for the provider to say it is
done, from any machine's browser. GLM and opencode Go open their key page and
ask for the key.

## The page

`remuda serve` listens on `127.0.0.1:7429` and opens a page listing each CLI's
credentials, with use, refresh, label, rename and remove, an import button for
a live login nobody stored yet, and sign-in for a new account. It also shows how
long each credential's tokens have left, whether the refresh timer is running
and what its last run did, and, when
[TokenGauge](https://github.com/Arzaroth/TokenGauge) is installed, each login's
usage, read from TokenGauge's snapshot (only the login in use before
TokenGauge 0.37). It keeps itself current: a change made from the CLI, by the
refresh timer or by a TokenGauge fetch shows within a couple of seconds.

The URL it prints carries an access token in its fragment. Every request must
send that token back and name this listener in its `Host` and `Origin`, so
another site open in the same browser cannot drive it.

`remuda open` opens the page a running `serve` shows, without the printed
URL. Installed with `--serve`, `remuda-serve.service` keeps one running (or
`open` starts it), and the token never reaches its journal.

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
`CURSOR_CONFIG_DIR`), the whole file is the login, as for Codex. Cursor's IDE
keeps its own login elsewhere, which remuda leaves alone.

An API key has no account behind it and never expires. opencode Go's lives in
`~/.local/share/opencode/auth.json` beside every other provider opencode is
connected to, so a switch rewrites its `opencode-go` entry and nothing else.
GLM reads `Z_AI_API_KEY` and nothing else, and a program cannot change a
running shell's environment, so `use` says to run
`eval "$(remuda env glm/<name>)"` instead. The same goes for opencode when
`OPENCODE_API_KEY` is set, since it wins over the file.

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
<cli>/<name>.meta.json    accountId, email, capturedAt, label, the
                          credential's credsDigest, and Claude's
                          oauthAccount block
.dirs.json                where the last interactive command found each
                          CLI's login, for the refresh timer
```

Files are 0600 and directories 0700. Other tools read the store: TokenGauge
shows a meter per stored credential (its ADR 0003 describes the contract).

## Caveats

remuda talks to the same OAuth endpoints and clients as Claude Code 2.1.282,
codex-cli 0.153.4, xAI's grok-build and MoonshotAI's kimi-code. They are not a
public API, and a release of any of them can change them. cursor-agent is
closed source: its refresh endpoint and sign-in are the ones the open-source
Cursor clients agree on, and the first to break.

## Development

```sh
mise install             # the pinned Rust (with llvm-tools), cargo-llvm-cov, shellcheck, Node, pnpm
cargo test               # unit tests, plus tests/cli.rs driving the built binary
scripts/coverage.sh      # line coverage via cargo-llvm-cov, worst files first
scripts/coverage.sh --html
```

The `serve` page is a Solid app under `web/`, built into `src/serve.html`,
which is committed so a Rust build needs no Node. After changing `web/`:

```sh
cd web && pnpm install
pnpm test                # Vitest
pnpm build               # rewrites src/serve.html; commit it with the change
pnpm dev                 # live reload against a running `remuda serve`
```

Nothing in the test suite reaches the network or a real CLI install. The OAuth
calls run against a local mock server, a Codex sign-in is completed through its
callback listener on an ephemeral port, and `tests/cli.rs` runs the binary
against a throwaway `HOME`. What stays uncovered on purpose is the process
surface in `main.rs`: `remuda update` talking to GitHub, `login` reading stdin,
`serve` binding its port, and `browser.rs` starting a browser.
