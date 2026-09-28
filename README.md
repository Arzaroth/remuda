# remuda

A remuda is the herd of spare horses a rider picks a fresh mount from each day,
while the rest recover. This one holds Claude Code and Codex logins.

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
(`... | bash -s -- --no-timer`). Afterwards, `remuda update` replaces the binary
with the latest release.

If your shell sets `CLAUDE_CONFIG_DIR`, `CODEX_HOME` or `REMUDA_STORE`, the
installer copies them to `~/.config/environment.d/60-remuda.conf` so the timer
looks where you do. If you change `CLAUDE_CONFIG_DIR` or `CODEX_HOME` later,
the timer notices: it compares its view with where your last `remuda` command
found each CLI, and refuses to refresh one it would look for in the wrong
place, saying what to set in that file (`journalctl --user -u remuda-refresh`).

## Commands

A credential is addressed by its name when only one CLI has that name, and as
`claude/<name>` or `codex/<name>` otherwise. Commands that create one take
`-p codex` for Codex; Claude Code is the default.

| Command | Does |
| --- | --- |
| `remuda import <name> [-p codex]` | Store the login the CLI is signed into right now |
| `remuda login <name> [-p codex] [--no-browser]` | Sign another account in through the browser and store it, without touching the CLI |
| `remuda use <name>` | Make a stored credential the one its CLI uses |
| `remuda ls [--json]` | List stored credentials, marking the active ones |
| `remuda refresh [name] [--force] [--within MIN]` | Refresh the inactive credentials expiring within `MIN` minutes (default 60) |
| `remuda label <name> [text]` | Set the label shown beside a credential, or clear it |
| `remuda rename <name> <new>` | Rename a stored credential |
| `remuda rm <name>` | Delete a stored credential |
| `remuda serve [--port N] [--no-browser]` | Do all of the above from a page in the browser |
| `remuda completions <shell>` | Print a completion script |
| `remuda update [--check]` | Replace the binary with the latest release |

`login` opens the provider's sign-in page. Sign in with the account to add (a
private window keeps the account you are signed into out of the way). Claude
then shows a `code#state` string to paste back; Codex calls back to
`localhost:1455` on its own, so run it on the machine whose browser you use,
and not while `codex login` is running.

## The page

`remuda serve` listens on `127.0.0.1:7429` and opens a page listing each CLI's
credentials, with use, refresh, label, rename and remove, an import button for
a live login nobody stored yet, and sign-in for a new account. Usage meters are
[TokenGauge](https://github.com/Arzaroth/TokenGauge)'s job, not this page's.

The URL it prints carries an access token in its fragment. Every request must
send that token back and name this listener in its `Host` and `Origin`, so
another site open in the same browser cannot drive it.

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
<cli>/<name>.meta.json    accountId, email, capturedAt, label, the
                          credential's credsDigest, and Claude's
                          oauthAccount block
.dirs.json                where the last interactive command found each
                          CLI's login, for the refresh timer
```

Files are 0600 and directories 0700. Other tools read the store: TokenGauge
shows a meter per stored credential (its ADR 0003 describes the contract).

## Caveats

remuda talks to the same OAuth endpoints and clients as Claude Code 2.1.282 and
codex-cli 0.153.4. They are not a public API, and a release of either can
change them.

## Development

```sh
cargo test               # unit tests, plus tests/cli.rs driving the built binary
scripts/coverage.sh      # line coverage via cargo-llvm-cov, worst files first
scripts/coverage.sh --html
```

Nothing in the test suite reaches the network or a real CLI install. The OAuth
calls run against a local mock server, a Codex sign-in is completed through its
callback listener on an ephemeral port, and `tests/cli.rs` runs the binary
against a throwaway `HOME`. What stays uncovered on purpose is the process
surface in `main.rs`: `remuda update` talking to GitHub, `login` reading stdin
and opening a browser, and `serve` binding its port.
