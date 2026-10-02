# Testing

`cargo test` runs the unit tests and `tests/cli.rs`. Nothing reaches the
network or touches the developer's own logins.

- **Providers take their endpoints and paths as values.** `claude::Api::local`
  and `codex::Api::local` point at a mockito server, `Claude::at` and
  `Codex::at` at a temp directory.
- **Only debug builds can be redirected.** `Api::claude()` and `Api::openai()`
  read `REMUDA_TEST_CLAUDE_API` / `REMUDA_TEST_OPENAI_API` under
  `#[cfg(debug_assertions)]`, which is how `tests/cli.rs` puts a mock in front
  of the built binary. A release build has no way to send a token elsewhere.
- **Offline is a released ephemeral port** (`ops::testing::OFFLINE`): bound once
  to reserve a number, then dropped, so connecting is refused at once. It
  serves the paths that must behave when a confirmation fails.
- **`ops::testing`** holds the shared fixtures: an `Env` with a temp store and a
  `Claude` pointed at it, `oauth(...)`, `account(...)`, `stored(...)`,
  `sign_in(...)`, and a profile body.
- **Codex sign-in** is completed for real: the test binds the callback listener
  on an ephemeral port, drives it with a raw TCP request, and the exchange hits
  the mock. JWTs are fabricated unsigned.
- **`tests/cli.rs`** runs the built binary with `env_clear()`, a temp `HOME`
  and both providers pointed at a mockito server that knows the profile behind
  each token `sign_in` writes and errors on anything else. `LLVM_PROFILE_FILE`
  passes through so coverage counts the binary.
- **PKCE is checked the way the real endpoints check it**: the mocks recompute
  the challenge from the verifier the exchange sent and compare it with the one
  in the authorize URL, and match the redirect URI.
- The whole suite passes with no network and an empty `HOME`, which is the
  check that nothing reaches the network or a real login:
  `CARGO_HOME=~/.cargo RUSTUP_HOME=~/.rustup HOME=$(mktemp -d) unshare -rn sh -c
  'ip link set lo up && cargo test --locked --offline'`, with rustup's `cargo`
  first on `PATH` (a shim that reads `HOME`, such as mise's, fails there).
  Cargo keeps its registry and toolchain under the real homes and must not
  fetch; loopback must be up because the mock servers listen on 127.0.0.1.
- **`serve`** is tested through `App::handle` with synthetic requests; `http.rs`
  over real sockets, with short limits: refusal before a body, oversized,
  stalled and malformed requests, the connection limit, and the event
  stream: a change to the store or the snapshot, the keepalive, and its slot
  limit. `tests/cli.rs`
  starts the real `serve` on port 0 to check that `open` reaches it, and runs
  it with `INVOCATION_ID` set to check the systemd behaviour.

`scripts/coverage.sh` (cargo-llvm-cov) prints the summary and the files with the
most uncovered lines. Uncovered on purpose: `main.rs`'s process surface
(`update`, stdin in `login`, binding in `serve`, starting the service in
`open`) and `browser.rs` starting a
browser; which browser and flag it picks is tested through `launcher`.

## Sources

- [src/ops.rs](../../src/ops.rs) (`testing` module)
- [tests/cli.rs](../../tests/cli.rs)
- [scripts/coverage.sh](../../scripts/coverage.sh)
