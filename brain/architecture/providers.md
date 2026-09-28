# Providers

`Provider` ([provider.rs](../../src/provider.rs)) is everything remuda needs
from a CLI. `creds` is always the store's shape for that CLI.

| Method | Meaning |
| --- | --- |
| `id`, `name` | `claude` / "Claude Code", `codex` / "Codex" |
| `home` | The directory the CLI keeps its login in, as this process resolves it |
| `access_token`, `refresh_token`, `expires_at`, `refresh_expires_at`, `plan` | Read from `creds` |
| `live` | The login the CLI is signed into, in store shape, or none |
| `foreign_login` | A live login `live` skips because it cannot be stored (an API key), described |
| `live_identity` | Who the CLI's own files say that login is (`Identity`) |
| `identify` | Whose tokens these are, per the tokens or the provider, never a file beside them |
| `install` | Make an entry the live login |
| `lock_live` | A lock other writers of the live login share, held by a switch (none by default) |
| `refresh` | Rotate the tokens in place; returns the account the endpoint answered for |
| `begin_login` | Start a sign-in, returning a `PendingLogin` |

A third CLI is one more implementation plus a line in `main.rs` (the
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
- `install` checks both files are JSON objects before writing either, then
  edits `.claude.json` first. Claude Code rewrites both files on its own and
  takes no lock, so each edit is a compare-and-swap (`fsx::update_json`): the
  file's inode, size and mtime are checked again just before the rename, and
  a file that moved meanwhile is read and edited again, so nothing Claude Code
  saved is reverted.

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

## Sources

- [src/provider.rs](../../src/provider.rs)
- [src/claude.rs](../../src/claude.rs)
- [src/codex.rs](../../src/codex.rs)
- [src/pkce.rs](../../src/pkce.rs)
- [src/oauth.rs](../../src/oauth.rs)
- [src/paths.rs](../../src/paths.rs)
