# Testing

`cargo test` runs the unit tests and `tests/cli.rs`. Nothing reaches the
network or touches the developer's own logins.

- **Providers take their endpoints and paths as values.** `claude::Api::local`
  and `codex::Api::local` point at a mockito server, `Claude::at` and
  `Codex::at` at a temp directory. Production constructors are the only ones
  that name the real endpoints; there is deliberately no env var that could
  redirect a token request.
- **Offline means `http://127.0.0.1:9`** (`ops::testing::OFFLINE`), which
  refuses connections, for the paths that must behave when confirmation fails.
- **`ops::testing`** holds the shared fixtures: an `Env` with a temp store and a
  `Claude` pointed at it, `oauth(...)`, `account(...)`, `stored(...)`,
  `sign_in(...)`, and a profile body.
- **Codex sign-in** is completed for real: the test binds the callback listener
  on an ephemeral port, drives it with a raw TCP request, and the exchange hits
  the mock. JWTs are fabricated unsigned.
- **`tests/cli.rs`** runs the built binary with `env_clear()` and a temp `HOME`,
  passing `LLVM_PROFILE_FILE` through so coverage counts it. Its scenarios stay
  offline by matching live logins to stored ones by token.
- **`serve`** is tested through `App::handle` with synthetic requests, plus one
  real socket round trip.

`scripts/coverage.sh` (cargo-llvm-cov) prints the summary and the files with the
most uncovered lines. Uncovered on purpose: `main.rs`'s process surface
(`update`, stdin and `xdg-open` in `login`, binding in `serve`).

## Sources

- [src/ops.rs](../../src/ops.rs) (`testing` module)
- [tests/cli.rs](../../tests/cli.rs)
- [scripts/coverage.sh](../../scripts/coverage.sh)
