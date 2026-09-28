# The local page

`remuda serve` opens a page listing each provider's credentials with Use,
Refresh, Label, Rename and Remove (Remove asks for a second click; Use,
Refresh and Remove are disabled on the active one). When a CLI's live login is
not stored, its section offers an Import box. "Refresh tokens" refreshes every
inactive credential due within the hour.

"Add an account" picks a provider and a name and starts a sign-in: the
provider's page opens in a new tab, then Claude shows a box for the pasted code
while Codex waits for its callback. Cancel abandons it, and for Codex frees port
1455 immediately.

It shows no usage meters: that is TokenGauge's job. How it is guarded and the
API it calls are in [architecture/web.md](../architecture/web.md).

## Sources

- [src/serve.html](../../src/serve.html)
- [src/serve.rs](../../src/serve.rs)
