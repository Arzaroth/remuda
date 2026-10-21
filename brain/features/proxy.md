# The proxy

`remuda serve` can also run an API proxy: one local endpoint that any OpenAI
or Anthropic client can point at, answered by whichever CLI login the model
name picks. It is off until it is switched on, from the page's API proxy card
or by setting `enabled` in its settings file.

## What a client sets

| Protocol | Base URL | Key sent as |
| --- | --- | --- |
| OpenAI | `http://127.0.0.1:8317/v1` | `Authorization: Bearer <key>` |
| Claude (Anthropic) | `http://127.0.0.1:8317` | `x-api-key: <key>` or a bearer token |

There is no endpoint per client: the path says the format and the model says
the provider. `ANTHROPIC_BASE_URL=http://127.0.0.1:8317` with
`ANTHROPIC_API_KEY=<key>` points Claude Code at it; an OpenAI SDK takes
`base_url` and `api_key`.

| Path | Format |
| --- | --- |
| `POST /v1/chat/completions` | OpenAI chat completions |
| `POST /v1/responses` | OpenAI responses |
| `POST /v1/messages`, `/v1/messages/count_tokens` | Anthropic messages |
| `GET /v1/models` | Anthropic's list shape when `anthropic-version` is sent, else OpenAI's |

## Which login answers

The model's prefix picks the provider: `claude-*` Claude Code, `gpt-*` and
`codex-*` Codex, `grok-*` Grok, `kimi-*` Kimi, `glm-*` GLM. `provider/model`
names the provider outright and is taken off before the request goes on, which
is the only way to reach opencode Go, whose models carry other makers' names
(`opencode/glm-4.6`). Cursor's API is not one the proxy speaks.

The request spends that provider's active login, the one `remuda use` made
live, read again on every request, so a switch switches the proxy too. The
proxy never refreshes it ([decisions](../decisions.md)): an expired access
token answers 401 and says to run the CLI once or switch, and a signed-out
CLI answers 503.

Each provider takes the formats its own endpoint speaks:

| Provider | Formats |
| --- | --- |
| Claude Code | Anthropic messages |
| Codex | OpenAI responses, streamed only |
| Grok | OpenAI responses |
| Kimi | all three |
| GLM | Anthropic messages, OpenAI chat completions |
| opencode Go | the format of the model's maker |

A request in a format the provider does not take is refused with a 400 that
names the ones it does.

## Access keys

The card makes keys (`rmd-` and 32 random bytes), shows one masked with a
button to reveal or copy it, and removes them. With no key, every client is
refused. Keys, `enabled` and `port` live in `$XDG_DATA_HOME/remuda/proxy.json`
(0600), beside the store rather than in it. The page follows the file like the
store, and switching the proxy on or off, or changing its port, takes effect at
once.

## Sources

- [src/proxy/](../../src/proxy/)
- [web/src/components/Proxy.tsx](../../web/src/components/Proxy.tsx)
