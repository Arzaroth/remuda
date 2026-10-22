# Running a login without switching, and pins

`remuda run` starts a CLI on one stored login for that session only. Every
other session, the page and auto mode keep using the login the CLI is
switched to. A `.remuda.toml` pins a login per directory, and `remuda
shell-init` makes typing `claude` or `codex` there start the pinned one.

## What a run starts

`remuda run <target> [-- args]`, where the target is one of:

| Target | Starts |
| --- | --- |
| `claude/work`, or `work` when only one CLI has it | A session on that login |
| `claude` | The login the nearest `.remuda.toml` pins, else the CLI as it is |
| The login the CLI already uses | The CLI as it is: there is nothing to keep apart |

`remuda claude work -- -p hi` is `remuda run claude/work -- -p hi`, and
`remuda claude -p hi` is `remuda run claude -- -p hi`. The word after `claude`
or `codex` is taken as a login only when that CLI has one stored by that name,
so the CLI's own arguments reach it as typed. These two words are read before
clap, which knows nothing of them.

Claude Code and Codex can run, through `Provider::run_spec`. The other CLIs
cannot yet ([ROADMAP.md](../../ROADMAP.md)).

## The session's directory

`session::start`, under the store lock:

1. Finish any session on this CLI whose process died (below), and sync the
   live login back.
2. Take the login's lease (below). A login already in a session is refused.
3. Refresh the login if its access token expires within 5 minutes, as a
   switch does (`ops::refresh_saved`).
4. Build `<store>/.runs/<cli>/<name>/home` (0700). Every entry of the CLI's
   own directory (`Provider::home`) is a symlink, so history, projects,
   settings, sessions and logs stay shared. The files that hold the login
   are copies: `.credentials.json` and `.claude.json` for Claude Code,
   `auth.json` for Codex. The copies are taken without the login's keys, the
   login is installed into them with `Provider::install`, the same call a
   switch makes, and the copies as taken are kept in `base` beside `home`.
   No other login's tokens are ever copied.

The store lock is then released, and the CLI runs with `CLAUDE_CONFIG_DIR` or
`CODEX_HOME` set to `home`, its arguments passed through. Its exit code is
remuda's.

While it runs, remuda ignores SIGINT, SIGQUIT, SIGHUP and SIGTERM, so a Ctrl-C
or a closed terminal ends the CLI but not remuda, which is still there to sync
the tokens back. The CLI gets those signals back to their defaults before it
starts.

## When it ends

`session::finish`, under the store lock again:

1. **Tokens.** The session's login (`Provider::relocated` reads `home`) goes
   back to the store when it changed, if it is the same login. That holds when
   it still has the refresh token the session started with, or when
   `Provider::identify` names the stored account for it. Tokens of another
   account (a `/login` inside the session), or ones that cannot be
   identified offline, are set aside. A copy older than the store's is
   dropped.
2. **The copied files.** What the session changed in them goes back into the
   CLI's own, through the same compare-and-swap a switch uses. The login's
   own keys are never merged back (`claudeAiOauth`, `oauthAccount`). The
   merge goes key by key, one level down (`projects.<dir>`,
   `mcpServers.<name>`, `mcpOAuth.<server>`), and where both sides changed a
   key, the CLI's own copy wins.
3. The directory is removed. Its links are unlinked, never followed.

A directory whose tokens could not even be read is left for the next command
to try again.

## In use

A session holds `flock` on `<store>/.runs/<cli>/<name>/lock` for as long as it
runs, and the kernel drops it when the process dies, however it dies. While a
login is held (`session::in_use`):

- the refresh timer and `remuda refresh` leave it to the session's CLI,
- `remuda use`, the page and auto mode do not switch to it, and `rm` and
  `rename` refuse it,
- keep-warm does not ping it,
- `ls` adds "in a run session" (`running` in `--json`), and the page shows "In
  a session" and disables its Use button.

The lock lives inside the directory and is removed with it. So a lock taken on
a file that was removed meanwhile is worth nothing, and `grab` checks that the
file it locked is still the one at that path.

A session whose process died (`kill -9`, a crash) leaves a directory with no
lock held. `commands::sync_all` and every `remuda run` finish such sessions
first (`session::recover`), and the log says so.

Codex has one more limit: auto mode holds a Codex switch while any `codex`
process runs ([auto-mode.md](auto-mode.md)), and that includes a session's.

## Pins

`.remuda.toml`, one key per CLI:

```toml
claude = "work"
codex = "perso"
```

`remuda run claude` reads the nearest file up from the working directory that
names `claude` (`pins::find`). So a repository can pin Claude and leave Codex
to a file higher up. A file another user owns is skipped with a warning, as
git does with `safe.directory`, so a clone cannot choose which of your logins
spends. A pin naming a login that is not stored is an error, not a silent
fallback.

`remuda pin <name>` writes the pin into `./.remuda.toml`, keeping the rest of
the file as written. `remuda pin --remove <cli>` drops one, and `remuda pin`
alone shows the pins in force here.

`remuda shell-init bash|zsh|fish` prints one function per CLI that can run,
`claude() { command remuda run claude -- "$@"; }`. Typing `claude` then starts
the pinned login, or plain `claude` where nothing is pinned. Unpinned, remuda
`exec`s the CLI at once, reading neither the store nor the network, and needs
no store at all.

## Sources

- [src/session.rs](../../src/session.rs)
- [src/pins.rs](../../src/pins.rs)
- [src/main.rs](../../src/main.rs) `run`, `shorthand`, `pin`, `shielded`
- [src/provider.rs](../../src/provider.rs) `RunSpec`, `run_spec`, `relocated`
- [src/claude.rs](../../src/claude.rs), [src/codex.rs](../../src/codex.rs) `run_spec`
- [src/commands.rs](../../src/commands.rs) `sync_all`, `refresh`, `refuse_in_use`
- [src/auto.rs](../../src/auto.rs) `Context::in_use`
