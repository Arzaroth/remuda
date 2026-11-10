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
  refresh) and Cursor (its refresh answers with an access token, and has not
  been seen to return a new refresh token; one that does is kept). The
  credential file goes across byte for byte (`Store::files`,
  `Store::put_files`), so `credsDigest` still holds; the sidecar goes across
  as JSON and is written again, keeping `accountId`, and one without
  `credsDigest` is given one on the way in.
- **Listed, to sign in again**: Claude Code, Codex, Grok and Kimi, whose
  refresh is single use. Two machines holding one refresh token race, and the
  first refresh signs the other out; Grok's server treats a second use after
  about a minute as theft and revokes both. The list holds the provider, name,
  account id, email and label, and no token.
- **`--move NAME`** carries a login's tokens instead, for a machine being
  retired. The login a CLI uses, or one in a `remuda run` session, is refused
  before anything is sent. The bundle is read under the store lock; once it
  has gone, `export::drop_moved` takes the lock again, sets the login's tokens
  aside (`Store::set_aside`, path printed) and removes it, unless its
  credential file is no longer the one sent: a running `serve` may have
  refreshed it while the other side was typing the code, leaving the bundle
  with a spent refresh token, so the login stays here with a warning.

It also carries `config.toml` as text, the proxy's `.proxy.json` with its
client keys emptied (a client points at one host and gets a new key there), and
the TokenGauge fleet: whatever `tokengauge --sync-export` prints, which is the
fleet key, the `[sync]` transport and the S3 keys, never the device id or
label. `--without-s3-keys` drops the S3 keys. Without TokenGauge, or without a
fleet key, export says so and goes on.

Never carried: the log, the attribution ledger, the proxy's request log,
`.dirs.json`, `.pins.json`, `.runs/`, the `.last-*` records, `.received-*`
and `.set-aside-*` files.

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
  line of `--passphrase-file`. The terminal's settings come back when the
  prompt ends, and when a signal (Ctrl-C) ends remuda there. The file is
  created 0600 and never written over. Without FILE and without a wormhole
  client, export refuses and says to give one.

## Restoring

`export::restore` writes what this machine lacks and keeps what it has unless
`--force`:

- Carried logins are stored, except one whose name is taken (kept) or whose
  account is stored here under another name (skipped). Either way, when the
  provider renews tokens and the file here is not the same, the tokens carried
  are set aside and the path printed: for a moved login they are the only
  copy. A login of a provider this remuda does not know is skipped and
  nothing of it is kept here. It stays on the sending machine: a copied one
  in the store there, a moved one in the set-aside file export printed, or in
  the store there when a refresh changed its tokens after the bundle read
  them (`export::drop_moved`).
- With `--force`, a login stored here is replaced only as a switch or a remove
  would allow it: the live login is read back first (`ops::sync_live`), and
  the one the CLI uses, one in a `remuda run` session, or any when which one
  the CLI uses is unknown, is kept, the tokens carried set aside. What a
  replaced login held is set aside, an API key as well as tokens: the key
  here may be its only copy.
- A login stored here that cannot be read (its sidecar missing, not JSON or
  incomplete, or its credential file not JSON) is kept as it is, even with
  `--force`, the tokens carried set aside, and the rest goes on; a listed
  login under such a name is skipped instead of offered.
- `config.toml` is checked (`Config::check`) against the logins stored here plus
  those listed to sign in, since names do not change, and written only when it
  passes; problems are printed and the config here stays.
- The proxy's settings are written when there are none; `--force` replaces
  them but keeps the client keys here, since the bundle carries none. Settings
  here that cannot be read are kept as they are, even with `--force`, and
  the rest goes on.
- The fleet goes to `tokengauge --sync-join -` on stdin (`--sync-force` with
  `--force`), then `tokengauge --sync-test` runs. A TokenGauge that cannot run,
  fails, or exits before reading the fleet, is reported and the rest stays
  restored, the sign-ins included.
  TokenGauge refuses a machine already in another fleet unless forced, and
  writes its own files: remuda never touches them.

A bundle received over a wormhole that cannot be restored (another format, or
an error partway) is kept in the store as `.received-<ms>.json`, 0600, and the
path printed; `remuda restore <that file>` reads it unsealed and removes it
once it succeeds.

Then each listed login not stored here yet, and whose account is not stored
here under another name (that is printed), is offered in turn ("Sign
claude/work in again, as me@example.com? [Y/n]"), through the same sign-in as
`remuda login`, which pastes the way it does: by default over SSH or, on
Linux, with no display (`browser::remote`), with `--paste` or `[login] paste =
true`, never with `--no-paste`; Enter opens the link only where a window can
be shown (`browser::displayed`). Pasting lets the browser be anywhere: Claude's code to
paste, Codex's, Grok's and Kimi's device codes. The login is filed only when
its tokens name the account the list says (`Prompt::expect`); another account
is refused, nothing is stored, and its tokens are set aside, as a refresh that
answers for another account does. Its label comes back with it. Logins skipped, refused, or
left when the input ends, and all of them with `--no-sign-in`, are printed as
the `remuda login NAME -p CLI --paste` lines to run later.

## Sources

- [src/export.rs](../../src/export.rs) `build`, `drop_moved`, `fleet`, `restore`, `keep_received`, `Via`, `Private`
- [src/seal.rs](../../src/seal.rs) `Quiet`
- [src/main.rs](../../src/main.rs) `export`, `restore`
- [src/store.rs](../../src/store.rs) `files`, `put_files`, `set_aside`
- [src/provider.rs](../../src/provider.rs) `portable`
- [src/commands.rs](../../src/commands.rs) `Prompt::expect`
