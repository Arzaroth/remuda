# The local page

`remuda serve [--port 7429]` binds `127.0.0.1` only (tiny_http), prints
`http://127.0.0.1:<port>/#<token>`, and opens it. The token is 32 random bytes,
base64url. The page reads it from the fragment (fragments never reach a server
log or a `Referer`), keeps it in `sessionStorage`, strips it from the address
bar, and sends it as `X-Remuda-Token`.

## Guards (`App::handle`)

1. `Host` must be `127.0.0.1:<port>` or `localhost:<port>`: defeats DNS
   rebinding.
2. An `Origin`, when sent, must be that same listener: defeats a cross-site
   `fetch` from another tab.
3. Every `/api/` call needs the token. `GET /` is served without it; the page
   holds no data.

Responses carry `Cache-Control: no-store`, `nosniff` and `no-referrer`. Bodies
are capped at 64 KiB. Each request runs on its own thread, because a Codex
`login/finish` holds its request open until the browser calls back.

## API

| Route | Does |
| --- | --- |
| `GET /api/state` | Same JSON as `ls --json`, plus `providers` |
| `POST /api/use` `{name, discard?}` | Switch |
| `POST /api/import` `{provider?, name, force?}` | Store the live login |
| `POST /api/label` `{name, text?}` / `rename` `{name, to}` / `remove` `{name}` | As the commands |
| `POST /api/refresh` `{name?, force?}` | Refresh inactive credentials |
| `POST /api/login` `{provider?, name, force?}` | Begin a sign-in: `{id, url, needsCode}` |
| `POST /api/login/finish` `{id, code?}` | Finish it and store the result |
| `POST /api/login/cancel` `{id}` | Abandon it; a waiting Codex callback stops |

Pending sign-ins live in memory, one per provider; starting another cancels the
previous one. Every mutating route takes the store lock except `login` and
`login/finish`, which lock only to save.

The page is one self-contained file (`include_str!`), no external requests,
light and dark. It reloads the state every minute unless an editor is open.

## Sources

- [src/serve.rs](../../src/serve.rs)
- [src/serve.html](../../src/serve.html)
- [src/main.rs](../../src/main.rs)
