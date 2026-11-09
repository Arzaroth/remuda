# The proxy

The proxy ([src/proxy/](../../src/proxy/)) is a second listener in the
`remuda serve` process, beside the page's own ([web.md](web.md)), and shares
nothing with it but the providers and a few pieces of `http.rs` (`read_head`,
`Slot`).

## Control (`mod.rs`)

`App` holds a `proxy::Control` and a weak reference to itself, set by
`App::start` once `main` has put it in an `Arc`. `App::apply_proxy` reads
`<store>/.proxy.json` and hands it to `Control::apply`, which starts, moves or
stops the listener to match `enabled`, `port` and `lan`; a port it cannot bind is kept
as the error the page shows, and `/api/proxy/settings` answers it as an error.
It runs at start, after every `/api/proxy/settings`, and from `serve::watch`
whenever the store's files change, so a hand edit takes effect within seconds.
A file that cannot be read leaves the listener as it was and shows as the
card's error, without its content (serde's message would quote a key). The listener sees the app through the `proxy::Backend` trait
(settings, providers, `whose` login a credential is, `record` for the request
log), which tests implement on their own.

## The listener (`listen.rs`)

Bound to `127.0.0.1`, or to `0.0.0.0` when `lan` is set (IPv4 only). The
key is the guard either way; `lan` adds nothing to it. The accept loop polls a stop flag every 50 ms, so
turning the proxy off frees the port. At most 32 connections at once; each
runs on its own thread.

1. The head is read within 10 s and 16 KiB.
2. The key, `Authorization: Bearer` or `x-api-key`, is checked against every
   key in `.proxy.json` in full, before any body is read
   (`Settings::matching`, which also says which key it was, for the log). No `Host` or
   `Origin` check: the key is the guard, the listener serves nothing but the
   proxy, and a browser cannot send either header cross-site without a
   preflight the listener never answers.
3. `Expect: 100-continue` is answered, then the body is read, by
   `Content-Length` or chunked (at most 64 trailer lines), up to 32 MiB,
   with 30 s allowed between reads and 5 minutes in all, and must be a JSON
   object.

Refusals go out through the page's `close_with` and `write_head`, so a client
still sending a body reads the refusal rather than a reset. A client that
stops reading the answer is dropped after a minute of blocked writes.

Errors are written in the client's shape: Anthropic's
`{type: "error", error: {type, message}}` on `/v1/messages`, OpenAI's
`{error: {message, type, code}}` elsewhere.

## Reaching it from other machines (`mod.rs`, `firewall.rs`)

With `lan` set, the base URLs the page shows use `proxy::lan_address`, the
source address of the default route, found by connecting a UDP socket, which
sends nothing. With no route `host` is null, the URLs say `127.0.0.1`, and
the card and the settings answer say the LAN address was not found.
`firewall::check` then says whether something on this machine keeps other
machines out, reading only what needs no root:

| Firewall | On when | Port open when | Opened by |
| --- | --- | --- | --- |
| ufw | `ENABLED=yes` in `/etc/ufw/ufw.conf` | the first `ufw-user-input` rule in `/etc/ufw/user.rules` for TCP on the port that is not limited (`-s`, `-d`, a negated `! -i`, or `-i` naming another interface than the default route's) accepts it and no limited rule before it refuses some machines, or, when no such rule matches, `DEFAULT_INPUT_POLICY` in `/etc/default/ufw` is `ACCEPT` | `sudo ufw allow <rule>`, or `sudo ufw insert 1 allow <rule>` when a deny rule for every machine matched, since an appended allow would come after it. `<rule>` is `in on <interface> to any port <port> proto tcp` the interface being the default route's, so a deny on another interface stays; when `/proc/net/route` has no default route it reads `IFACE`, with a trailing comment saying to put the LAN's interface there, rather than opening every interface. A deny limited to some machines is left in place, with `sudo ufw status numbered` first to show it, its trailing comment saying such a deny still refuses those machines. When one comes before the deny for every machine, or after it on the same interface (where an allow inserted first would overtake it), the allow is `sudo ufw insert N allow <rule>`, the comment naming N as the first `DENY` or `REJECT` for the port to Anywhere from Anywhere, on that interface or on none, and asking for any deny or `limit` for some machines to be moved above it. A `limit` rule for some machines before the deny for every machine takes the same path, so the allow does not overtake its rate limit. Rules that cannot be read get `sudo ufw status numbered` too |
| firewalld | `firewall-cmd --state` succeeds | `firewall-cmd --zone=<zone> --query-port=<port>/tcp` says `yes`, the zone being that of the default route's interface (`proxy::default_interface`, from `/proc/net/route`), or the default zone when firewall-cmd says `no zone`, and `--get-active-zones` names no zone bound to `sources:`, since such a zone takes the machines it names away from the interface's | `sudo firewall-cmd --permanent --zone=<zone> --add-port=<port>/tcp`, then `--reload`; when the zone lookup failed, `--zone=$(firewall-cmd --get-zone-of-interface=<interface>)`, so the shell asks again |

On a machine with several interfaces, the one used is the default route's, for the LAN address and the ufw rule alike; a LAN on another interface needs its address and rule put in by hand. The two can still differ: the address is whatever the kernel's routing picks for `192.0.2.1`, policy rules and split routes included (a full-tunnel VPN gives the tunnel's), while the interface is the `00000000` row of the main table in `/proc/net/route`.

ufw is looked at first. `open` is null when ufw's rules or policy cannot be
read, when limited rules let some machines in that the verdict would keep out
(or the reverse), when the interface's zone or the active zones cannot be
learnt, when an active zone is bound to sources, or when firewall-cmd's
answer is neither yes nor no. A port opened by a firewalld service counts as closed;
nftables or iptables set by hand are not read.

`/api/state` runs under the store's lock, so it never asks: `App::firewall`
answers the last verdict for the port and, once that is 10 s old, starts
`App::ask_firewall` on its own thread (one at a time). A verdict that changes
moves an entry of `App::fingerprint`, so `/api/events` tells the page to load
again. It asks only while the proxy is on with `lan`. The places are
`Places::etc`, `Places::search`, `Places::route` and `Places::lan_address`:
a temporary directory, none, a missing file and no address in tests.

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
  `x-app: cli`, `anthropic-dangerous-direct-browser-access: true` and Claude
  Code's user agent. A client that is not Claude Code
  itself (`User-Agent: claude-cli/...`) gets Claude Code's identity line as
  its first system block, its own system prompt after it.
- Codex: `chatgpt.com/backend-api/codex/responses` with the bearer token,
  `chatgpt-account-id` from `tokens.account_id` (the workspace) and
  `originator: codex_cli_rs`. The body is forced to `stream: true` and
  `store: false`, gets `instructions: ""` when it has none, and loses the
  API's sampling and storage fields.
- Grok: `cli-chat-proxy.grok.com/v1/responses` with the bearer token and the
  grok CLI's identity headers (`x-xai-token-auth`, `x-grok-client-version`,
  `x-grok-client-identifier`, `x-authenticateresponse`, its user agent), as
  grok 1.0.46 sends them: the chat proxy answers 426 to an older or unnamed
  client.
- Kimi: `api.kimi.com/coding/v1/{chat/completions,responses,messages}`, with
  the `X-Msh-*` device headers its OAuth calls already send.
- GLM: `api.z.ai/api/anthropic/v1/messages` and
  `api.z.ai/api/coding/paas/v4/chat/completions`, the paths the coding plan's
  quota draws on.
- opencode Go: `opencode.ai/inference/go/{anthropic,openai}/v1/...` by model
  family: `claude-*` Anthropic messages, `gpt-*` and `grok-*` responses,
  `gemini-*` none (opencode serves them only in Google's format), the rest
  chat completions.

On Anthropic requests the client's `anthropic-beta` values are added to the
provider's, each once, and `anthropic-version` defaults to `2023-06-01`. Of
the client's other headers only `anthropic-version`, `openai-beta`, `accept`
and the session headers Codex and its kind send (`session_id`, `session-id`,
`thread-id`, `version`, `x-client-request-id`) are passed on, and never over
one the provider sets.
`count_tokens` goes to the provider's messages URL with `/count_tokens`
added, or answers 501.

The upstream client has a 15 s connect timeout and no total one, since a
stream lasts as long as the model writes, and follows no redirect: one to
another host would carry an API key in `x-api-key`. The upstream status, content type,
`retry-after` and request id come back as they are, then the body, copied as
it arrives and flushed after every read, with `Connection: close`. A client
that goes away ends the copy and drops the upstream connection.

`GET /v1/models` lists `route::models` for every provider with a live login,
to every client, since translation lets any of them reach any model. Those
are examples; any name with the provider's prefix routes the same way.

Tests run the listener on port 0 against `mockito` upstreams, through each
provider's `Api::local`; `ApiKey::inferring_at` does the same for GLM and
opencode Go.

## Translation (`translate/`)

When the client's format is not one the provider takes, `forward` sends the
provider's first format instead and puts `translate` on both sides.

- **Request**: `parse` reads the client's body into a `Conversation` (system
  text, user and assistant turns of `Part`s: text, images, tool calls, tool
  results, thinking; tools, tool choice, limits, reasoning effort, whether it
  streams), and `emit` writes it out as an Anthropic, a Responses or a chat
  request, always streamed (chat asks for its usage chunk). Turns of the same
  role are merged; tool results lead their Anthropic turn, become
  `function_call_output` items for Responses and `tool` messages for chat.
  Anthropic's server tools and their calls, which carry no schema, are
  dropped.
- **Thinking**: an Anthropic thinking budget (under 4096 tokens low, under
  16384 medium, else high) or adaptive effort and an OpenAI reasoning effort
  map onto each other. Bound for Claude, a model from 4.6 on (read off the
  name, and any name without a version) gets `thinking: {type: "adaptive"}`
  and `output_config.effort`, which is all 4.7 and later take; an older one a
  budget of 2048, 8192 or 24576 with `max_tokens` raised past it. Thinking is
  left off when Claude would refuse it: a forced tool choice, or a last
  assistant turn with a tool call that does not open with Claude's own
  signed thinking.
- **Tool names**: Codex takes at most 64 characters. A longer name is cut and
  tagged with 8 hex digits of its SHA-256 (`Names`), and the answer's calls
  get the original back.
- **Signatures**: each provider refuses the other's, so a signature handed to
  a client in another format is tagged with its maker. An OpenAI reasoning
  item's `encrypted_content` reaches an Anthropic client as a signature
  prefixed `remuda-reasoning:`; Claude's reaches a Responses client as
  `encrypted_content` prefixed `remuda-claude:`. Sent back, each goes only to
  its maker, untagged (to a Responses endpoint with
  `include: ["reasoning.encrypted_content"]`), and is dropped anywhere else.
  A chat client has no field for either.
- **Answer**: `decode` reads the upstream's server-sent events, Anthropic,
  Responses or chat chunks, into `Event`s (start, text, thinking, signature,
  tool call, its arguments, usage, stop, error), and `encode` writes them in
  the client's format as they arrive: Anthropic's `message_start` /
  `content_block_*` / `message_delta` events, chat completion chunks ending in
  `[DONE]` (and a usage chunk when `stream_options.include_usage` asked for
  one), or the Responses events with their sequence numbers. A client that
  did not ask to stream gets one JSON body built from the same events. Usage
  counts every prompt token as input on the OpenAI side and splits off cache
  reads and writes on Anthropic's. Stop reasons map end turn and stop
  sequence to `stop`, max tokens to `length`, tool use to `tool_calls`; a
  Responses answer stops on tool use when it made a call. A summary part
  after the first opens with a blank line. A line of more than 16 MiB ends
  the stream.
- **Errors**: an upstream error status comes back with the provider's message,
  at most 500 characters, in the client's error shape. A stream that fails, or
  ends before saying how the answer stopped, ends with an error event in the
  client's format; for a whole answer, with the status its kind would have had
  (429 for a rate limit, 529 overloaded, 400 a bad request, else 502).

A client that asked Codex for a whole answer in Codex's own format is not
translated: Codex is sent a stream, and `translate::collect` returns the
completed response with the items Codex streamed, as it sent them, since the
completed event's own `output` can be empty. The client's session headers go
with it.

## The request log (`journal.rs`)

`forward` wraps the work in a `journal::Trace`, filled as the request goes:
the provider and model once `pick` has them, the login from `Backend::whose`
(the stored, verified entry whose refresh or access token the live login
holds), the upstream's status, the time to its headers and its
`request-id`, then what the client was told (`translate::Seen`). After it,
`Trace::request` turns that into a `selvedge::usage::requests::Request` for
`Backend::record`, which `serve` appends to the store.

What the client was told depends on the path:

- **Passed through, streamed:** `translate::relay` writes each upstream read
  on to the client as it arrives and decodes the same bytes for the id and
  the last usage event, so nothing waits on the parsing.
- **Passed through, whole:** the body is kept, up to 8 MiB, and read once
  written (`translate::decode::whole`).
- **Translated:** `respond` notes the events it encodes, and the id becomes
  the one the encoder writes (`msg_`, `chatcmpl-` or `resp_` before the
  upstream's), which is what the client, and Claude Code's transcript, keep.
- **Collected Codex responses:** read from the completed response.

A write to the client that fails marks it `canceled`. Usage is logged with
input fresh, as the store format counts it: the decoders' input includes
cached tokens, so the cache reads and writes are taken out. A cache write
counts as the 5-minute kind.

## Sources

- [src/proxy/mod.rs](../../src/proxy/mod.rs)
- [src/proxy/journal.rs](../../src/proxy/journal.rs)
- [src/proxy/listen.rs](../../src/proxy/listen.rs)
- [src/proxy/forward.rs](../../src/proxy/forward.rs)
- [src/proxy/route.rs](../../src/proxy/route.rs)
- [src/proxy/settings.rs](../../src/proxy/settings.rs)
- [src/proxy/firewall.rs](../../src/proxy/firewall.rs)
- [src/proxy/translate/](../../src/proxy/translate/)
- [src/provider.rs](../../src/provider.rs)
