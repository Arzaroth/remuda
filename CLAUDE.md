# remuda

Keeps several Claude Code and Codex logins and switches the one each CLI uses.
Start with `brain/BRAIN.md` to find how anything works.

## The brain

`brain/` is the committed knowledge base: architecture, features, decisions,
glossary. It exists so the tool can be understood without reading the source.
Navigate it through `brain/BRAIN.md` and the two `index.md` files rather than
grepping the tree; the `brain` skill has the full routine.

- Keep it true to the code. A change that makes a brain doc wrong fixes that
  doc in the same branch.
- The code wins: if the brain disagrees with reality, correct the brain.

## Credentials are other people's files

remuda writes into Claude Code's and Codex's own credential files. These rules
follow, and each has tests:

- A switch changes the login's keys and nothing else (`claudeAiOauth`,
  `oauthAccount`; Codex's `auth.json` whole). Never rewrite a file from a
  reconstructed object.
- Never lose a refresh token: sync the live login back before anything reads
  the store, never refresh the active credential, never file tokens under a
  credential whose account was not confirmed (`Provider::identify`, never the
  CLI's files alone), never drop tokens a refresh already rotated.
- Never put a token, state or verifier on a command line; open the browser
  through a private file.

Tests never touch the developer's real files or the network: providers are
built with `::at(dir, Api::local(url))`, and `tests/cli.rs` points the binary
at a mock through `REMUDA_TEST_CLAUDE_API` / `REMUDA_TEST_OPENAI_API`, which
exist in debug builds only. A release build has no way to redirect an OAuth
endpoint; keep it that way. The suite must pass under `unshare -rn` with an
empty `HOME`. Manual smoke tests use `REMUDA_STORE` pointed at a scratch
directory.

## The store is a contract

TokenGauge (ADR 0003) reads `<store>/<provider>/<name>.json` and the sidecar keys
`accountId`, `email`, `label` and `credsDigest` (lowercase hex SHA-256 of the
credential file's bytes). Changing any of them changes that ADR too.

## Conventions

- `CHANGELOG.md` `[Unreleased]` gets an entry with every user-facing change; it
  becomes the GitHub release notes.
- Before finishing: `cargo fmt --all`, `cargo clippy --all-targets -- -D warnings`,
  `cargo test`. CI also shellchecks `scripts/install.sh` and
  `scripts/release.sh`.
- `scripts/coverage.sh` ranks files by uncovered lines; `main.rs`'s process
  surface stays uncovered on purpose.
- Releases go through `scripts/release.sh` (the `release` skill); the tag push
  builds and publishes them.
- The OAuth constants in `claude.rs` and `codex.rs` were read out of Claude Code
  2.1.282 and codex-cli 0.153.4. When a CLI release breaks sign-in or refresh,
  re-read them from the new binary (`strings` on it) rather than guessing.
