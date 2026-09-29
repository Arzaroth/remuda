# Overview

One binary, one crate, no daemon. Every command reads the CLI's files, the
store, or both, does its work under the store lock, and exits. `serve` is the
only long-running mode.

## Modules

| Module | Role |
| --- | --- |
| `main.rs` | Argument parsing (clap) and wiring: builds the providers, takes the lock, dispatches |
| `commands.rs` | One function per command, writing to a `Write` so tests can read the output; name resolution |
| `ops.rs` | The two operations everything rests on: `sync_live` and `switch`, plus `refresh_entry` |
| `provider.rs` | The `Provider` and `PendingLogin` traits |
| `claude.rs`, `codex.rs` | The two providers, including their OAuth clients |
| `store.rs` | The credential store: entries, sidecars, rename, remove, the lock |
| `serve.rs`, `serve.html` | The local page and its JSON API |
| `http.rs` | The page's HTTP listener: bounded reads, checks before bodies, a connection limit |
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
2. It takes `<store>/.lock` (except `login`, which takes it only to save, and
   `completions`/`update`, which need no store).
3. `ops::sync_live` copies the live login back into its credential, and reports
   the live state: signed out, stored (and whether that was confirmed), or
   unstored.
4. The command runs against that state and prints one line of outcome.

Output is written to a `&mut dyn Write` everywhere below `main`, which is how the
page reuses the same functions and how tests capture what a user would read.

## Sources

- [src/main.rs](../../src/main.rs)
- [src/commands.rs](../../src/commands.rs)
- [src/ops.rs](../../src/ops.rs)
- [src/provider.rs](../../src/provider.rs)
