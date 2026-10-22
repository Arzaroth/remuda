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

`main::run`, under the store lock, syncs the live login back
(`ops::sync_live`, which first finishes any session on this CLI whose
processes have all ended), then `session::start`:

1. A session already running on the login is joined: the new terminal holds
   its lock too and runs in the same directory, as two terminals share the
   CLI's own. A directory left by a session that could not be finished
   refuses the run, since its newest tokens are in it.
2. Otherwise the login's lock is taken. If its access token expires within 5
   minutes it is refreshed, as a switch does (`ops::REFRESH_BEFORE_MS`).
3. `<store>/.runs/<cli>/<name>/home` is built (0700). Every entry of the CLI's
   own directory (`Provider::home`) is a symlink, so history, projects,
   settings, sessions and logs stay shared. For Claude Code, the files that
   hold the login are copies, taken without the login's keys:
   `.credentials.json` and `.claude.json`, whose `oauthAccount`,
   `primaryApiKey` and `customApiKeyResponses` stay out. The copies as taken
   are kept in `base` beside `home`. Codex copies nothing: its `auth.json` is
   the login's alone. Then the login is installed with `Provider::install`,
   the same call a switch makes, so no other login's tokens are ever copied.

The store lock is then released, and the CLI runs with `CLAUDE_CONFIG_DIR` or
`CODEX_HOME` set to `home`, its arguments passed through as bytes. Beside it,
`REMUDA_OUTER_CLAUDE_CONFIG_DIR` or `REMUDA_OUTER_CODEX_HOME` holds what the
variable was outside, and `paths` reads that first. So a remuda started from
inside the session (the CLI's Bash tool, a hook, a nested `claude`) acts on
the CLI's own files, never the session's copy.

While the CLI runs, remuda ignores SIGINT, SIGQUIT, SIGHUP and SIGTERM, so
remuda is still there to sync the tokens back. The CLI gets the handling
remuda was started with. When one of those signals killed the CLI, remuda
dies of it too, so a shell loop sees an interrupt; otherwise remuda exits
with the CLI's code (128 and the signal for a crash). A `remuda run` that
`exec`s the CLI as it is, from inside a session, gives it back its own
directory.

## When it ends

Every remuda running a CLI in the session holds its shared lock, and notes
the CLI's pid and start time beside it (`cli-<remuda pid>`). The session lasts
while either says something runs: a remuda, a joined terminal, or a CLI whose
remuda was killed. The CLI's own children (an MCP server, a background job)
hold nothing, so one left running does not keep the session open. When
remuda's CLI exits, it lets go and `session::recover` finishes the session
if nothing else runs in it. A session directory with no lock, left by a
process that kept the session's directory but not its lease, is finished too. A session
whose processes all died is finished by the next command that syncs a live
login, which is nearly every command. `session::finish`, under the store
lock and the session's exclusive lock:

1. **Tokens.** The session's login (`Provider::relocated` reads `home`) goes
   back to the store when it changed, if it is the same login. That holds when
   it still has the refresh token the store has, or when `Provider::identify`
   names the stored account for it. Tokens of another account (a `/login`
   inside the session) are set aside. Tokens older than the store's are set
   aside too, and both are said in the log. Tokens that cannot be identified
   yet, offline say, are an error, and the directory is kept for the next
   command to try. A session that ended signed out is said as well, since
   its CLI may have spent the store's refresh token first.
2. **What the CLI made.** Entries in `home` that are not links go into the
   CLI's own directory, copied when it is on another filesystem: ones it did
   not have, and files the CLI rewrote by rename, which replaced their link.
   A file that also changed outside since the session started is not
   overwritten: the session's copy lands beside it as `<name>.remuda-run`
   (with a time after it when that is taken).
3. **The copied files.** What the session changed in them goes back into the
   CLI's own, through the same compare-and-swap a switch uses, the login's
   keys excepted. The merge goes key by key, one level down
   (`projects.<dir>`, `mcpServers.<name>`, `mcpOAuth.<server>`). Where both
   sides changed a key, the CLI's own copy wins, except for a token both
   still hold: the one with the later `expiresAt` is the one a refresh
   rotated to. What the CLI removed meanwhile stays removed.

   The copy of `.credentials.json` keeps the CLI's MCP logins (`mcpOAuth`), so
   a session can use them. Their refresh tokens are then in two files: an MCP
   server that revokes a reused refresh token may ask both sides to sign in
   to it again.
4. The directory is removed. Its links are unlinked, never followed.

Anything that fails keeps the directory, for the next command to try again.

## In use

`session::in_use` holds while the session's directory exists, whether it is
running or ended without being finished. While it holds:

- the refresh timer and `remuda refresh` leave the login to the session's CLI,
- `remuda use`, the page and auto mode do not switch to it,
- `rm`, `rename`, `import --force` and `login --force` refuse it,
- keep-warm does not ping it, and checks again under the store lock before a
  ping,
- `ls` adds "in a run session" (`running` in `--json`), and the page shows "In
  a session" and disables its Use button.

A lock taken on a file that was removed meanwhile guards nothing, so `still_at`
checks that the locked file is still the one at that path.

A Codex run session is not one of the `codex` processes auto mode waits out
([auto-mode.md](auto-mode.md)): it is told apart by `REMUDA_OUTER_CODEX_HOME`
in its environment beside a `CODEX_HOME` under `.runs`.

## Pins

`.remuda.toml`, one key per CLI:

```toml
claude = "work"
codex = "perso"
```

`remuda run claude` reads the nearest file up from the working directory that
names `claude` (`pins::find`). So a repository can pin Claude and leave Codex
to a file higher up.

A file is read only when it has been allowed as it is now. `<store>/.pins.json`
keeps each allowed file's path and the digest of its contents, as direnv does
with `.envrc`. So a `.remuda.toml` that came with a clone cannot choose which
of your logins spends until you look at it. `remuda pin <name>` writes the pin
into `./.remuda.toml`, keeping the rest of the file as written, and allows
the result. It refuses a file that is a link, belongs to someone else, or was
not allowed already, so it never vouches for pins nobody read. `remuda pin
--remove <cli>` drops one, and `remuda pin --allow` trusts the nearest file as
it is and prints what it holds. `remuda pin` alone shows the pins in force
here.

A file that is not allowed, is a link, belongs to another user or does not
parse is passed over with a warning, so a stray one never stops the CLI from
starting. A pin naming a login that is not stored is an error.

`remuda shell-init bash|zsh|fish` prints one function per CLI that can run,
`claude() { command remuda run claude -- "$@"; }`. Typing `claude` then starts
the pinned login, or plain `claude` where nothing is pinned. Unpinned,
remuda `exec`s the CLI before it reads or writes anything in the store
beyond the allowlist, which it reads only when a `.remuda.toml` is found.

## Sources

- [src/session.rs](../../src/session.rs)
- [src/pins.rs](../../src/pins.rs)
- [src/main.rs](../../src/main.rs) `run`, `shorthand`, `pin`, `shielded`, `exit_as`
- [src/paths.rs](../../src/paths.rs) `outer`
- [src/provider.rs](../../src/provider.rs) `RunSpec`, `run_spec`, `relocated`
- [src/claude.rs](../../src/claude.rs), [src/codex.rs](../../src/codex.rs) `run_spec`
- [src/commands.rs](../../src/commands.rs) `refresh`, `refuse_in_use`
- [src/ops.rs](../../src/ops.rs) `sync_live`, `REFRESH_BEFORE_MS`
- [src/auto.rs](../../src/auto.rs) `running`
- [src/auto.rs](../../src/auto.rs) `Context::in_use`
