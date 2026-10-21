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

Any client format reaches any provider. Each provider's endpoint speaks
some formats; a request in another is translated on the way in and its answer
on the way out:

| Provider | Formats |
| --- | --- |
| Claude Code | Anthropic messages |
| Codex | OpenAI responses, streamed only |
| Grok | OpenAI responses |
| Kimi | all three |
| GLM | Anthropic messages, OpenAI chat completions |
| opencode Go | the format of the model's maker |

So Claude Code can run on a Codex login with `--model gpt-5`, and an OpenAI
SDK can use a Claude login. Text, images, tools and their results, reasoning,
stop reasons and token counts carry over, streamed or not. Claude models
from 4.6 on think adaptively at the effort the client asked for, older ones
on a token budget. What does not carry over: Anthropic's server tools (web
search and the like) are dropped on the way to an OpenAI endpoint; reasoning
from one provider is not handed to another (each refuses the other's
signatures), and a chat client, with no field for Claude's signatures,
loses Claude's thinking on the turn after a tool call; and `count_tokens`
works only on a provider that takes Anthropic requests. A request that Codex gets without
`stream: true` is streamed from Codex all the same and answered whole.

## Access keys

The card makes keys (`rmd-` and 32 random bytes), shows one masked with a
button to reveal or copy it, and removes them. With no key, every client is
refused. Keys, `enabled` and `port` live in `<store>/.proxy.json` (0600),
beside `.last-refresh.json`, outside any provider's directory. Switching the
proxy on or off, or changing its port, takes effect at once, from the page or
from a hand edit of the file; a port that cannot be bound is said on the card.

## Sources

- [src/proxy/](../../src/proxy/)
- [web/src/components/Proxy.tsx](../../web/src/components/Proxy.tsx)
