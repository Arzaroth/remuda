# Sign-in

`remuda login <name> [-p codex] [--force] [--no-browser]` adds an account
without touching the CLI's live login. It checks the name first
(`commands::begin_login`), prints the provider's authorize URL and opens it
through a private redirect file, never as a command-line argument.

It opens in a private window of the default browser (`browser::open_private`),
because signing in as another account in the browser's normal profile replaces
the claude.ai or ChatGPT session already there. `xdg-settings get
default-web-browser` names the browser, and remuda waits two seconds for it at
most. Brave, Chrome, Chromium and Vivaldi get `--incognito`, Edge
`--inprivate`, Firefox and LibreWolf `--private-window`. The desktop entry must
match exactly, so each release channel (beta, dev, nightly, ESR, developer
edition) runs its own executable and never the stable one. With no display, a
browser not in the table (Snap and Flatpak ones included), no such executable
on `PATH` or no answer in time, it falls back to `xdg-open` and says a private
window avoids the account you are signed into.

- **Claude Code**: after sign-in, the callback page shows `code#state`; paste it.
  A pasted state that does not match refuses the login. The code is exchanged,
  the profile fetched, and the credential is built the way Claude Code builds
  its own (`subscriptionType`, `rateLimitTier`, the `oauthAccount` block).
- **Codex**: the browser calls back to `localhost:1455` by itself, so the
  command just waits (up to 10 minutes). Port 1455 must be free, so not while
  `codex login` runs, and the browser must be on the same machine. Stray
  connections and callbacks from other attempts are answered and ignored; a
  denied consent with this attempt's state ends it. The stored account is the
  seat the new tokens name.

Saving takes the store lock and re-checks: a name taken meanwhile, or an
account already stored under another name, refuses the result.

The page offers the same flow in two calls; see [web-page.md](web-page.md).

## Sources

- [src/browser.rs](../../src/browser.rs) `open_private`, `launcher`
- [src/commands.rs](../../src/commands.rs) `login`, `save_login`
- [src/claude.rs](../../src/claude.rs) `start_login`, `finish_login`
- [src/codex.rs](../../src/codex.rs) `begin_login`, `CodexPending`
