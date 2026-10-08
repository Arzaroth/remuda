# Overview

One binary, one crate, no daemon of its own. Every command reads the CLI's
files, the store, or both, does its work under the store lock, and exits.
`serve` is the only long-running mode; `remuda-serve.service` can keep it
running.

## Modules

| Module | Role |
| --- | --- |
| `main.rs` | Argument parsing (clap) and wiring: builds the providers, takes the lock, dispatches |
| `browser.rs` | Opening a URL through a redirect file, in a private window when the default browser allows |
| `commands.rs` | One function per command, writing to a `Write` so tests can read the output; name resolution |
| `ops.rs` | The two operations everything rests on: `sync_live` and `switch`, plus `refresh_entry` |
| `provider.rs` | The `Provider` and `PendingLogin` traits |
| `claude.rs`, `codex.rs` | The two providers, including their OAuth clients |
| `store.rs` | The credential store: entries, sidecars, rename, remove, the lock |
| `serve.rs`, `serve.html` | The local page and its JSON API |
| `served.rs` | The files a running page announces itself in, and finding it again for `open` |
| `units.rs` | The systemd units the binary carries, and rewriting installed ones after an update |
| `http.rs` | The page's HTTP listener: bounded reads, checks before bodies, a connection limit, the event stream |
| `oauth.rs` | The HTTP client and token requests both providers share |
| `dirs.rs` | The directories interactive commands used, for the scheduled refresh to check |
| `runs.rs` | The last scheduled refresh's record, for the page |
| `gauge.rs` | Reads TokenGauge's usage snapshot for the page |
| `fsx.rs` | Atomic 0600 writes, compare-and-swap JSON edits, the file lock, timestamps |
| `paths.rs` | Where each CLI's files and the store live, honouring their env vars |
| `pkce.rs` | PKCE verifier/state generation and the S256 challenge |
| `project.rs` | The selvedge project declaration and the release-contract tests |

## How a command flows

1. `main` builds `Claude` and `Codex` and resolves the name (`commands::resolve`)
   or the `-p` flag (`commands::find`) to one provider.
2. A missing store stops every command but `import` and `login`
   (`Cmd::needs_store`), which create it (0700) through the directories
   record and the lock. Then it
   takes `<store>/.lock` (except `login`, which takes it only to save, and
   `completions`/`update`/`open`, which need no store).
3. `ops::sync_live` copies the live login back into its credential, and reports
   the live state: signed out, stored (and whether that was confirmed), or
   unstored.
4. The command runs against that state and prints one line of outcome.

Output is written to a `&mut dyn Write` everywhere below `main`, which is how the
page reuses the same functions and how tests capture what a user would read.

## Sources

- [src/browser.rs](../../src/browser.rs)
- [src/main.rs](../../src/main.rs)
- [src/commands.rs](../../src/commands.rs)
- [src/ops.rs](../../src/ops.rs)
- [src/provider.rs](../../src/provider.rs)
