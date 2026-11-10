# Setting up another machine

`remuda export [FILE] [--move NAME]... [--without-s3-keys] [--passphrase-file F]`
on this machine and `remuda restore [FILE] [--force] [--passphrase-file F]
[--no-sign-in] [--paste | --no-paste]` on the new one give it the same logins and settings, and enrol
it in TokenGauge's fleet, without a browser on the new machine or a password
sent to it. Keeping machines in step afterwards is not this: that is hub and
satellites, still on the roadmap.

## The bundle

`export::build` reads the live logins back first (`ops::sync_live`), then
sorts every stored login:

- **Carried as they are**: those whose `Provider::portable` holds, meaning their
  tokens work from two machines at once. That is the API keys (nothing to
  refresh) and Cursor (its refresh answers with an access token and never a new
  refresh token). Both files go across byte for byte (`Store::files`,
  `Store::put_files`), so `credsDigest` and `accountId` still hold.
- **Listed, to sign in again**: Claude Code, Codex, Grok and Kimi, whose
  refresh is single use. Two machines holding one refresh token race, and the
  first refresh signs the other out; Grok's server treats a second use after
  about a minute as theft and revokes both. The list holds the provider, name,
  account id, email and label, and no token.
- **`--move NAME`** carries a login's tokens instead, for a machine being
  retired, and removes it here once the bundle has gone. The login a CLI uses,
  or one in a `remuda run` session, is refused before anything is sent.

It also carries `config.toml` as text, the proxy's `.proxy.json` with its
client keys emptied (a client points at one host and gets a new key there), and
the TokenGauge fleet: whatever `tokengauge --sync-export` prints, which is the
fleet key, the `[sync]` transport and the S3 keys, never the device id or
label. `--without-s3-keys` drops the S3 keys. Without TokenGauge, or without a
fleet key, export says so and goes on.

Never carried: the log, the attribution ledger, the proxy's request log,
`.dirs.json`, `.pins.json`, `.runs/`, the `.last-*` records and `.set-aside-*`
files.

## How it travels

- **wyrmyon (`wyrm`), then magic-wormhole (`wormhole`)**, whichever is on
  `PATH` (`export::Via`). export writes the bundle into a private directory
  (`export::Private`, 0700, removed afterwards) and runs `send --hide-progress
  <file>` with the terminal attached, so the tool prints the code and waits.
  restore runs `receive --accept-file --hide-progress -o <file>` the same way,
  so the tool asks for the code itself and it never reaches a command line or
  remuda. The wormhole seals the transfer end to end under the code.
- **A file**, when FILE is given: sealed in age's scrypt format
  ([seal.rs](../../src/seal.rs), the `age` crate), so `age -d` opens it too. The
  passphrase is typed twice at `/dev/tty` with echo off, or read from the first
  line of `--passphrase-file`. The file is created 0600 and never written over.
  Without FILE and without a wormhole client, export refuses and says to give
  one.

## Restoring

`export::restore` writes what this machine lacks and keeps what it has unless
`--force`:

- Carried logins are stored, except one whose name is taken (kept) or whose
  account is stored here under another name (skipped).
- `config.toml` is checked (`Config::check`) against the logins stored here plus
  those listed to sign in, since names do not change, and written only when it
  passes; problems are printed and the config here stays.
- The proxy's settings are written when there are none.
- The fleet goes to `tokengauge --sync-join -` on stdin (`--sync-force` with
  `--force`), then `tokengauge --sync-test` runs. TokenGauge refuses a machine
  already in another fleet unless forced, and writes its own files: remuda
  never touches them.

Then each listed login not stored here yet is offered in turn ("Sign claude/work
in again, as me@example.com? [Y/n]"), through the same sign-in as `remuda
login`, which pastes the way it does: by default over SSH or, on Linux, with no
display (`browser::remote`), with `--paste` or `[login] paste = true`, never
with `--no-paste`; Enter opens the link only where a window can be shown
(`browser::displayed`). Pasting lets the browser be anywhere: Claude's code to paste,
Codex's, Grok's and Kimi's device codes. The login is filed only when its tokens name the
account the list says (`Prompt::expect`); another account is refused and
nothing is stored. Its label comes back with it. Logins skipped, refused, or
left when the input ends, and all of them with `--no-sign-in`, are printed as
the `remuda login NAME -p CLI --paste` lines to run later.

## Sources

- [src/export.rs](../../src/export.rs) `build`, `fleet`, `restore`, `Via`, `Private`
- [src/seal.rs](../../src/seal.rs)
- [src/main.rs](../../src/main.rs) `export`, `restore`
- [src/store.rs](../../src/store.rs) `files`, `put_files`
- [src/provider.rs](../../src/provider.rs) `portable`
- [src/commands.rs](../../src/commands.rs) `Prompt::expect`
