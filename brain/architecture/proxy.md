# The proxy

The proxy ([src/proxy/](../../src/proxy/)) is a second listener in the
`remuda serve` process, beside the page's own ([web.md](web.md)), and shares
nothing with it but the providers and a few pieces of `http.rs` (`read_head`,
`Slot`).

## Control (`mod.rs`)

`App` holds a `proxy::Control` and a weak reference to itself, set by
`App::start` once `main` has put it in an `Arc`. `App::apply_proxy` reads
`proxy.json` and hands it to `Control::apply`, which starts, moves or stops the
listener to match `enabled` and `port`; a port it cannot bind is kept as the
error the page shows. It runs at start, after every `/api/proxy/settings`, and
on every `/api/state`, so a hand edit of the file takes effect on the next
page load. The listener sees the app through the `proxy::Backend` trait
(settings, providers), which tests implement on their own.

## The listener (`listen.rs`)

Bound to `127.0.0.1` only. The accept loop polls a stop flag every 50 ms, so
turning the proxy off frees the port. At most 32 connections at once; each
runs on its own thread.

1. The head is read within 10 s and 16 KiB.
2. The key, `Authorization: Bearer` or `x-api-key`, is checked against every
   key in `proxy.json` in full, before any body is read. No `Host` or
   `Origin` check: the key is the guard, and the listener serves nothing but
   the proxy.
3. The body is read, by `Content-Length` or chunked, up to 32 MiB, with 30 s
   allowed between reads, and must be a JSON object.

Errors are written in the client's shape: Anthropic's
`{type: "error", error: {type, message}}` on `/v1/messages`, OpenAI's
`{error: {message, type, code}}` elsewhere.

## Forwarding (`forward.rs`, `route.rs`)

`route::route` turns the model into a provider and the model name to send.
`forward` then takes the provider's live login (`Provider::live`), refuses an
expired one, and asks the provider for its side of the request through three
`Provider` methods:

| Method | Says |
| --- | --- |
| `formats(model)` | The formats the endpoint takes for that model |
| `upstream(creds, format, model)` | Its URL and headers, token included |
| `shape(format, body, user_agent)` | Edits the body to what the endpoint accepts, or refuses it |

What each provider sends:

- Claude: `api.anthropic.com/v1/messages?beta=true` with the bearer token,
  `anthropic-beta: claude-code-20250219,oauth-2025-04-20,interleaved-thinking-2025-05-14`,
  `x-app: cli` and Claude Code's user agent. A client that is not Claude Code
  itself (`User-Agent: claude-cli/...`) gets Claude Code's identity line as
  its first system block, its own system prompt after it.
- Codex: `chatgpt.com/backend-api/codex/responses` with the bearer token,
  `chatgpt-account-id` from `tokens.account_id` (the workspace) and
  `originator: codex_cli_rs`. The body is forced to `store: false`, gets
  `instructions: ""` when it has none, and loses the API's sampling and
  storage fields; a request without `stream: true` is refused.
- Grok: `cli-chat-proxy.grok.com/v1/responses` with the bearer token.
- Kimi: `api.kimi.com/coding/v1/{chat/completions,responses,messages}`, with
  the `X-Msh-*` device headers its OAuth calls already send.
- GLM: `api.z.ai/api/anthropic/v1/messages` and
  `api.z.ai/api/coding/paas/v4/chat/completions`, the paths the coding plan's
  quota draws on.
- opencode Go: `opencode.ai/inference/go/{anthropic,openai}/v1/...` by model
  family: `claude-*` Anthropic messages, `gpt-*` and `grok-*` responses, the
  rest chat completions.

On Anthropic requests the client's `anthropic-beta` values are added to the
provider's, each once, and `anthropic-version` defaults to `2023-06-01`.
`count_tokens` goes to the provider's messages URL with `/count_tokens`
added, or answers 501.

The upstream client has a 15 s connect timeout and no total one, since a
stream lasts as long as the model writes. The upstream status, content type,
`retry-after` and request id come back as they are, then the body, copied as
it arrives and flushed after every read, with `Connection: close`. A client
that goes away ends the copy and drops the upstream connection.

`GET /v1/models` lists `route::models` for every provider with a live login.
Those are examples; any name with the provider's prefix routes the same way.

Tests run the listener on port 0 against `mockito` upstreams, through each
provider's `Api::local`; `ApiKey::inferring_at` does the same for GLM and
opencode Go.

## Sources

- [src/proxy/mod.rs](../../src/proxy/mod.rs)
- [src/proxy/listen.rs](../../src/proxy/listen.rs)
- [src/proxy/forward.rs](../../src/proxy/forward.rs)
- [src/proxy/route.rs](../../src/proxy/route.rs)
- [src/proxy/settings.rs](../../src/proxy/settings.rs)
- [src/provider.rs](../../src/provider.rs)
