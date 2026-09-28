# Sign-in

`remuda login <name> [-p codex] [--force] [--no-browser]` adds an account
without touching the CLI's live login. It prints the provider's authorize URL
and opens it with `xdg-open`; a private window keeps the signed-in account out
of the way.

- **Claude Code**: after sign-in, the callback page shows `code#state`; paste it.
  A pasted state that does not match refuses the login. The code is exchanged,
  the profile fetched, and the credential is built the way Claude Code builds
  its own (`subscriptionType`, `rateLimitTier`, the `oauthAccount` block).
- **Codex**: the browser calls back to `localhost:1455` by itself, so the
  command just waits (up to 10 minutes). Port 1455 must be free, so not while
  `codex login` runs, and the browser must be on the same machine. A denied
  consent or a foreign state is shown in the browser and refused.

Saving takes the store lock and re-checks: a name taken meanwhile, or an
account already stored under another name, refuses the result.

The page offers the same flow in two calls; see [web-page.md](web-page.md).

## Sources

- [src/commands.rs](../../src/commands.rs) `login`, `save_login`
- [src/claude.rs](../../src/claude.rs) `start_login`, `finish_login`
- [src/codex.rs](../../src/codex.rs) `begin_login`, `CodexPending`
