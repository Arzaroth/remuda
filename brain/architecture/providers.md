# Providers

`Provider` ([provider.rs](../../src/provider.rs)) is everything remuda needs
from a CLI. `creds` is always the store's shape for that CLI.

| Method | Meaning |
| --- | --- |
| `id`, `name` | `claude` / "Claude Code", `codex` / "Codex", `grok`, `kimi`, `cursor`, `glm`, `opencode` |
| `home` | The directory the CLI keeps its login in, as this process resolves it |
| `access_token`, `refresh_token`, `expires_at`, `refresh_expires_at`, `plan` | Read from `creds` |
| `live` | The login the CLI is signed into, in store shape, or none |
| `foreign_login` | A live login `live` skips because it cannot be stored (an API key), described |
| `live_identity` | Who the CLI's own files say that login is (`Identity`) |
| `identify` | Whose tokens these are, per the tokens or the provider, never a file beside them |
| `install` | Make an entry the live login |
| `lock_live` | A lock other writers of the live login share, held by a switch (none by default) |
| `refresh` | Rotate the tokens in place; returns the account the endpoint answered for |
| `renews` | False for an API key, which `refresh` skips (true by default) |
| `env_line` | The `export` line `remuda env` prints, for a key read from the environment |
| `begin_login` | Start a sign-in, returning a `PendingLogin` |

Another CLI is one more implementation plus a line in `main.rs` (the
`PROVIDERS` list for `-p` and the provider arrays) and in `serve`'s list. Token
requests go through [oauth.rs](../../src/oauth.rs), which also turns a refused
refresh token into "sign this credential in again".

## Claude Code

- Files: `.credentials.json` (`claudeAiOauth`) and `.claude.json`
  (`oauthAccount`), both under `CLAUDE_CONFIG_DIR` when set, else `~/.claude`
  and `~/.claude.json`.
- Identity: `oauthAccount.accountUuid` in `.claude.json`, which is a different
  file from the tokens and can lag them, hence `identify` calls the profile
  endpoint.
- Plan: `subscriptionType` plus the `Nx` suffix of `rateLimitTier` ("max 20x").
- OAuth (from Claude Code 2.1.282): client id `9d1c250a-e61b-44d9-88ed-5944d1962f5e`,
  authorize at `claude.com/cai/oauth/authorize`, token at
  `platform.claude.com/v1/oauth/token` (JSON body), profile at
  `api.anthropic.com/api/oauth/profile`. Sign-in redirects to
  `platform.claude.com/oauth/code/callback`, which shows a `code#state` to paste.
- A login builds `oauthAccount` from the profile the way Claude Code does
  (`oauth_account_from_profile`) and maps `organization_type` onto
  `subscriptionType`.
- `install(entry, outgoing)` checks both files are JSON objects before
  writing either, then edits `.claude.json`, then `.credentials.json`. Each
  edit is a compare-and-swap on the file's content (`fsx::update_json`), so
  nothing Claude Code saved meanwhile is reverted, and the credentials are
  written only while `claudeAiOauth` is still `outgoing`, the login the switch
  synced; otherwise nothing is written (`fsx::Changed`) and `.claude.json` is
  put back. Codex's `install` holds `auth.json` to the same check.

## Codex

- File: `auth.json` under `CODEX_HOME`, else `~/.codex`. The whole file is the
  login. Only ChatGPT sign-ins (a file with OAuth tokens) count as live; an API
  key or personal access token is reported by `foreign_login`, so a switch
  asks for `--discard` before replacing it.
- Identity: the seat, one person in one workspace: the access token's
  `chatgpt_account_user_id` claim, else the id token's `chatgpt_user_id` and
  `chatgpt_account_id` joined with `__`. Not `tokens.account_id`, which names
  only the workspace and is shared by every seat of a Team plan (it stays in
  the file, as Codex needs it). Read from the tokens themselves, so `identify`
  needs no network. Email from the id token.
- `lock_live` takes `auth.json.lock`, the lock TokenGauge refreshes the live
  file under.
- Expiry: the access token's `exp` claim. Plan: `chatgpt_plan_type` claim.
- OAuth (from codex-cli 0.153.4): client id `app_EMoamEEZ73f0CkXaXp7hrann`,
  issuer `auth.openai.com`. Sign-in is PKCE with a callback to
  `localhost:1455/auth/callback`; the code exchange is form-encoded. Refresh is
  a JSON body with scope `openid profile email`, keeping any token the answer
  leaves out, and stamps `last_refresh`.
- A sign-in listens on 127.0.0.1:1455 and, when the machine has it, [::1]:1455
  (the redirect says `localhost`), for up to 10 minutes. A port an attempt
  being cancelled still holds gets two seconds to free up. It reads each
  connection's request line with a 2 s timeout and an 8 KiB cap, drops what it
  cannot read, answers other paths with a 404 and a callback with another
  state with a 400, and keeps waiting; only a callback with this attempt's
  state ends it, as a code or a refusal. It can be cancelled from another
  thread (the page's cancel button).

## Grok

- File: `auth.json` under `GROK_HOME`, else `~/.grok`, or `GROK_AUTH_PATH`
  itself. An object keyed by `<issuer>::<client id>`; the login is the entry
  whose key starts `https://auth.x.ai::` and has a `key`. The whole file is
  stored. An entry under any other scope with a `key` and no OIDC entry is an
  API key, reported by `foreign_login`.
- Identity: the access token's own `sub`. Email from the entry. Expiry: the
  token's `exp`.
- `lock_live` takes `auth.json.lock`, the flock the CLI refreshes under.
- OAuth (xai-org/grok-build, `xai-grok-login`): issuer `auth.x.ai`, client id
  `b1a00492-073a-47ea-816f-4c329264a828`. Refresh is a form POST to
  `/oauth2/token`; the refresh token rotates and is single use. Sign-in is a
  device code (`/oauth2/device/code`), and the stored file is built the way
  the CLI writes one.

## Kimi Code

- File: `credentials/kimi-code.json` under `KIMI_CODE_HOME`, else
  `~/.kimi-code`, stored whole. A signed-out CLI keeps the file with its tokens
  emptied, which reads as signed out. `expires_at` is epoch seconds, possibly
  fractional.
- Identity: the file names nobody, so `identify` asks
  `api.kimi.com/coding/v1/me` and `live_identity` is always none.
- OAuth (MoonshotAI/kimi-code, `packages/oauth`): host `auth.kimi.com`, client
  id `17e5f671-d194-4dfb-9706-5516cb48c098`. Every call carries the CLI's
  `X-Msh-*` device headers with the CLI's own `device_id`, never a new one.
  Refresh is a form POST to `/api/oauth/token` and rotates; sign-in is a device
  code (`/api/oauth/device_authorization`).
- The CLI's refresh lock is a lock directory (`oauth/kimi-code.lock`), not a
  flock, so `lock_live` takes nothing; the switch's compare-and-swap is what
  guards it.

## Cursor

- File: `cursor-agent`'s `auth.json` under `CURSOR_CONFIG_DIR`, else
  `$XDG_CONFIG_HOME/cursor`: `{accessToken, refreshToken, apiKey}`, stored
  whole. A file with only an `apiKey` is a foreign login.
- Identity: the user id after the `|` in the access token's `sub`.
- cursor-agent is closed source. Refresh is a JSON POST to
  `api2.cursor.sh/oauth/token` with client id `KbZUR41cY7W6zRSdpSUJ7I7mLYBKOCmB`;
  it usually returns only an access token, and a session Cursor will not renew
  answers 200 with `shouldLogout`, which is a refused refresh. Sign-in opens
  `cursor.com/loginDeepControl` with a PKCE challenge and a uuid and polls
  `api2.cursor.sh/auth/poll` until it returns the tokens.

## API keys: GLM and opencode Go

`ApiKey` ([apikey.rs](../../src/apikey.rs)) is both. A credential is
`{"key": ...}`, filed under `key-` and the first 16 hex digits of the key's
SHA-256: there is no account to confirm, and the key itself never reaches a
sidecar. `renews` is false, so `refresh` skips them. Sign-in opens the key
page and takes the pasted key (`asks_for`: "the API key").

- GLM reads `Z_AI_API_KEY` (legacy `ZAI_API_TOKEN`) and nothing else, so
  `install` refuses and points at `remuda env`.
- opencode Go reads `OPENCODE_API_KEY`, else the `opencode-go` entry of
  `$XDG_DATA_HOME/opencode/auth.json`, which holds every provider opencode is
  connected to. `install` rewrites that entry only, compared and swapped on the
  key it replaces, and refuses when the variable is set, since it would win.

## Sign-in by device code

[device.rs](../../src/device.rs) is RFC 8628 for Grok and Kimi: ask for a
device code, hand the verification URL to the browser, poll the token endpoint
at the interval given (slower on `slow_down`) until it answers, the user
refuses, the code expires, or the page cancels. Nothing listens locally, so
the browser can be on another machine.

## Sources

- [src/provider.rs](../../src/provider.rs)
- [src/claude.rs](../../src/claude.rs)
- [src/codex.rs](../../src/codex.rs)
- [src/grok.rs](../../src/grok.rs)
- [src/kimi.rs](../../src/kimi.rs)
- [src/cursor.rs](../../src/cursor.rs)
- [src/apikey.rs](../../src/apikey.rs)
- [src/device.rs](../../src/device.rs)
- [src/pkce.rs](../../src/pkce.rs)
- [src/oauth.rs](../../src/oauth.rs)
- [src/paths.rs](../../src/paths.rs)
