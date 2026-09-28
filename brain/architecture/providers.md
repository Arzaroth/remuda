# Providers

`Provider` ([provider.rs](../../src/provider.rs)) is everything remuda needs
from a CLI. `creds` is always the store's shape for that CLI.

| Method | Meaning |
| --- | --- |
| `id`, `name` | `claude` / "Claude Code", `codex` / "Codex" |
| `access_token`, `refresh_token`, `expires_at`, `refresh_expires_at`, `plan` | Read from `creds` |
| `live` | The login the CLI is signed into, in store shape, or none |
| `live_identity` | Who the CLI's own files say that login is (`Identity`) |
| `confirm` | Which account the tokens belong to, for when the files may disagree |
| `install` | Make an entry the live login |
| `refresh` | Rotate the tokens in place; returns the account the endpoint answered for |
| `begin_login` | Start a sign-in, returning a `PendingLogin` |

A third CLI is one more implementation plus a line in `main.rs` (the
`PROVIDERS` list for `-p` and the provider arrays) and in `serve`'s list.

## Claude Code

- Files: `.credentials.json` (`claudeAiOauth`) and `.claude.json`
  (`oauthAccount`), both under `CLAUDE_CONFIG_DIR` when set, else `~/.claude`
  and `~/.claude.json`.
- Identity: `oauthAccount.accountUuid` in `.claude.json`, which is a different
  file from the tokens, hence `confirm` calls the profile endpoint.
- Plan: `subscriptionType` plus the `Nx` suffix of `rateLimitTier` ("max 20x").
- OAuth (from Claude Code 2.1.282): client id `9d1c250a-e61b-44d9-88ed-5944d1962f5e`,
  authorize at `claude.com/cai/oauth/authorize`, token at
  `platform.claude.com/v1/oauth/token` (JSON body), profile at
  `api.anthropic.com/api/oauth/profile`. Sign-in redirects to
  `platform.claude.com/oauth/code/callback`, which shows a `code#state` to paste.
- A login builds `oauthAccount` from the profile the way Claude Code does
  (`oauth_account_from_profile`) and maps `organization_type` onto
  `subscriptionType`.

## Codex

- File: `auth.json` under `CODEX_HOME`, else `~/.codex`. The whole file is the
  login. Only ChatGPT sign-ins (a file with OAuth tokens) count as live.
- Identity: `tokens.account_id`, falling back to the id token's
  `chatgpt_account_id` claim; email from the id token. Same file as the
  tokens, so `confirm` needs no network.
- Expiry: the access token's `exp` claim. Plan: `chatgpt_plan_type` claim.
- OAuth (from codex-cli 0.153.4): client id `app_EMoamEEZ73f0CkXaXp7hrann`,
  issuer `auth.openai.com`. Sign-in is PKCE with a callback to
  `localhost:1455/auth/callback`; the code exchange is form-encoded. Refresh is
  a JSON body with scope `openid profile email`, keeping any token the answer
  leaves out, and stamps `last_refresh`.
- A sign-in listens on 127.0.0.1:1455 for up to 10 minutes, answers anything
  but the callback with a 404, and can be cancelled from another thread (the
  page's cancel button).

## Sources

- [src/provider.rs](../../src/provider.rs)
- [src/claude.rs](../../src/claude.rs)
- [src/codex.rs](../../src/codex.rs)
- [src/pkce.rs](../../src/pkce.rs)
- [src/paths.rs](../../src/paths.rs)
