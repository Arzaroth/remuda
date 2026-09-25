# remuda

A remuda is the herd of spare horses a rider picks a fresh mount from each day,
while the rest recover. This one holds Claude Code logins.

Keep several Claude accounts and switch the one Claude Code uses, without
logging out and back in and without a separate `CLAUDE_CONFIG_DIR`. History,
plans, settings and MCP server logins stay shared; only the account changes.

```
$ remuda ls
* work   me@work.example   max 20x   token in 5h, refresh in 29d
  perso  me@example.com    max 5x    token in 2h, refresh in 12d

$ remuda use perso
switched to perso (me@example.com)
```

## Install

Linux, x86_64 or aarch64:

```sh
curl -fsSL https://raw.githubusercontent.com/Arzaroth/remuda/main/scripts/install.sh | bash
```

This installs `remuda` into `~/.local/bin` and enables `remuda-refresh.timer`,
which keeps the inactive logins' tokens fresh every 30 minutes. Pass
`--no-timer` to skip the timer or `--version vX.Y.Z` to pin a release
(`... | bash -s -- --no-timer`). Afterwards, `remuda update` replaces the binary
with the latest release.

## Commands

| Command | Does |
| --- | --- |
| `remuda import <name>` | Store the login Claude Code is signed into right now |
| `remuda login <name>` | Sign another account in through the browser and store it, without touching Claude Code |
| `remuda use <name>` | Make a stored credential the one Claude Code uses |
| `remuda ls [--json]` | List stored credentials, marking the active one |
| `remuda refresh [name] [--force] [--within MIN]` | Refresh the inactive credentials expiring within `MIN` minutes (default 60) |
| `remuda rm <name>` | Delete a stored credential |
| `remuda update [--check]` | Replace the binary with the latest release |

`login` prints an authorize URL and opens it. Sign in with the account to add
(a private window keeps the account you are signed into out of the way), then
paste the `code#state` string the page shows.

## How a switch works

A switch rewrites two keys and nothing else: `claudeAiOauth` in
`~/.claude/.credentials.json` and `oauthAccount` in `~/.claude.json`. Both honour
`CLAUDE_CONFIG_DIR`. Running Claude Code sessions watch those files and adopt
the new login.

Claude Code rotates the refresh token of the account it is signed into, so
before every command remuda copies the live login back into the credential it
belongs to. It matches on the tokens first and falls back to the account id,
which it confirms with the provider before trusting. It refuses to switch away
from a login that is not stored (`--discard` overrides), and it never refreshes
the active credential: Claude Code owns that one.

The inactive credentials' access tokens expire within hours, so the installer
enables a user timer that runs `remuda refresh` every 30 minutes. Without it,
run `remuda refresh` before anything reads the store.

## The store

`$REMUDA_STORE`, else `$XDG_DATA_HOME/remuda/credentials`, else
`~/.local/share/remuda/credentials`:

```
claude/<name>.json        same shape as .credentials.json, claudeAiOauth only
claude/<name>.meta.json   accountUuid, email, capturedAt, the oauthAccount block
```

Files are 0600 and directories 0700. Other tools read the store: TokenGauge
shows a meter per stored credential (its ADR 0003 describes the contract).

## Caveats

remuda talks to the same OAuth endpoints and client as Claude Code 2.1.282. They
are not a public API, and a Claude Code release can change them.

## Development

```sh
cargo test               # unit tests, plus tests/cli.rs driving the built binary
scripts/coverage.sh      # line coverage via cargo-llvm-cov, worst files first
scripts/coverage.sh --html
```

Nothing in the test suite reaches the network or a real Claude Code install.
The OAuth calls run against a local mock server, and `tests/cli.rs` runs the
binary against a throwaway `HOME`. What stays uncovered on purpose is the
process surface in `main.rs`: `remuda update` talking to GitHub, and `login`
reading stdin and opening a browser.
