# Sign-in

`remuda login <name> [-p <cli>] [--force] [--no-browser] [--browser <id>]
[--window private|profile|normal] [--paste|--no-paste]` adds an account without touching the CLI's
live login. It checks the name first (`commands::begin_login`), then prints the
provider's authorize URL before opening anything, saying whether any browser
will do or only one on this machine, for a sign-in that calls back to a local
port (`PendingLogin::local_only`: Codex, and Claude unless pasting). Pressing
Enter opens it here, in the
place the prompt names; it is still waited for meanwhile, and a pasted code
still finishes it. `--no-browser` prints the link and offers nothing.
`--force` signs a stored login in again under its name, for one whose refresh
token was rejected or whose sign-in ran out; when the new tokens are the same
account's, its label is kept (`commands::save_login`).

## Where it opens

`[login]` in config.toml says where ([config.md](../architecture/config.md)),
and `--browser` and `--window` override it for one run (`browser::choose`):

- **browser**: one of the browsers remuda knows (`browser::BROWSERS`, by id:
  `brave`, `chrome`, `chromium`, `vivaldi`, `edge`, `firefox`, `librewolf` and
  their beta, dev, nightly, ESR or developer channels), when it is on `PATH`.
  Otherwise the desktop's default browser, which `xdg-settings get
  default-web-browser` names (waited for two seconds at most); its desktop
  entry must match one of the table's exactly, so each release channel runs
  its own executable and never the stable one. A browser set to one that is
  not installed falls back to the default the same way, so naming a known
  browser is also how to sign in when the default one is not in the table.
- **window**:
  - `private` (the default): `--incognito` (Chromium family), `--inprivate`
    (Edge), `--private-window` (Firefox family). Signing in as another
    account in the browser's normal profile would replace the claude.ai or
    ChatGPT session already there.
  - `profile`: the login's own browser profile,
    `<store>/.browser/<cli>/<name>/` (`browser::profile_dir`), created 0700,
    passed as `--user-data-dir=` with `--no-first-run
    --no-default-browser-check` (Chromium family) or `--new-instance
    --profile` (Firefox family). The profile keeps that account signed in, so
    signing it in again later can take a click rather than an emailed link.
    It holds the account's web session, so it stays under the store; `rename`
    moves it and `rm` deletes it.
  - `normal`: the browser as it is, in its own profile.

The browser is handed a private redirect file, never the URL, whose state
would otherwise be on a command line. With a browser not in the table (Snap
and Flatpak ones included), no such executable on `PATH` or no answer in time,
it falls back to `xdg-open`, which cannot be asked for a window, and the
prompt says "the default browser". With no display (neither `DISPLAY` nor
`WAYLAND_DISPLAY`, `browser::displayed`) nothing is opened: `login` does not
offer Enter, and the link is to be opened by hand.

## Per CLI

- **Claude Code**: the browser calls back to `localhost` on a free port, as
  Claude Code's own sign-in does, so the command waits (up to 10 minutes) and
  the browser must be on this machine; the prompt says so, and the page names
  the machine remuda runs on (its hostname, `machine` in `POST /api/login`). Once
  stored, the browser lands on Claude's success page. Other requests to the
  port, and callbacks with another attempt's state, are answered and ignored;
  a refusal ends it. With `--paste`, `[login] paste = true` or the page's
  checkbox, the sign-in ends on Claude's page that shows `code#state` instead,
  to paste, so the browser can be on any machine; a pasted state that does not
  match refuses the login. `login` also pastes when the session is evidently
  remote (`browser::remote`: `SSH_CONNECTION` or `SSH_TTY` set, or on Linux
  neither `DISPLAY` nor `WAYLAND_DISPLAY`), where a callback would never
  arrive; `--no-paste` asks for the callback anyway, over SSH or against
  `paste = true`. The page decides by its checkbox alone, since the server's
  environment says nothing about where the page is viewed. Either way the code is exchanged, the profile
  fetched, and the credential is built the way Claude Code builds its own
  (`subscriptionType`, `rateLimitTier`, the `oauthAccount` block).
- **Codex**: the browser calls back to `localhost:1455` by itself, so the
  command just waits (up to 10 minutes). Port 1455 must be free, so not while
  `codex login` runs, and the browser must be on the same machine. Stray
  connections and callbacks from other attempts are answered and ignored; a
  denied consent with this attempt's state ends it. The stored account is the
  seat the new tokens name.

- **Grok and Kimi**: a device code ([device.rs](../../src/device.rs)). The
  URL carries the code to approve, and remuda polls the token endpoint until
  the user approves or refuses, the code expires, or the page cancels. Nothing
  listens locally, so the browser can be anywhere. Grok's file is built the
  way the CLI writes one; Kimi's account is whoever `/me` says.
- **Cursor**: `cursor.com/loginDeepControl` with a PKCE challenge and a uuid;
  remuda polls `api2.cursor.sh/auth/poll` with the verifier until the tokens
  are there, backing off from 1 s to 10 s.
- **GLM and opencode Go**: the key page opens and the pasted key is the login.

Saving takes the store lock and re-checks: a name taken meanwhile, or an
account already stored under another name, refuses the result.

The page offers the same flow in two calls; see [web-page.md](web-page.md).

## Sources

- [src/browser.rs](../../src/browser.rs) `choose`, `Choice`, `BROWSERS`, `profile_dir`
- [src/commands.rs](../../src/commands.rs) `login`, `save_login`
- [src/claude.rs](../../src/claude.rs) `begin_login`, `start_login`, `finish_login`, `ClaudePending`
- [src/callback.rs](../../src/callback.rs) `Callback`
- [src/codex.rs](../../src/codex.rs) `begin_login`, `CodexPending`
- [src/device.rs](../../src/device.rs) `DeviceFlow`, `DevicePending`
- [src/cursor.rs](../../src/cursor.rs) `CursorPending`
- [src/apikey.rs](../../src/apikey.rs) `KeyPending`
