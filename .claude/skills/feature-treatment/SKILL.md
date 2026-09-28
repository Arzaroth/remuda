---
name: feature-treatment
description: Ship a finished remuda feature branch the full way - rebase onto main, run a max-effort multi-agent code review, fix everything confirmed, update the brain/ knowledge base, pass the gate and CI, merge keeping the layered commits, and cut a release. Use when the user says "feature treatment", "treat this branch", "review and merge this feature", or names a worktree/branch to ship.
---

# Feature treatment

The pipeline a finished feature branch goes through before it lands: **rebase ->
max review -> fix -> update brain -> gate -> merge -> release -> clean up**.
Nothing merges without an adversarial review and a green gate.

The branch comes from the argument (a branch or worktree name). If none is
given, find it: `wt list` (or `git worktree list`) and `git branch` - it is the
non-`main` one; worktrees live in `~/repos/remuda.worktrees/<branch>`. Confirm
which one if ambiguous. Branch names follow `feature/...`, `fix/...`; every
commit subject starts with `[<branch>]`.

## 1. Rebase onto main

```bash
cd ~/repos/remuda.worktrees/<branch>     # or the main checkout if the branch is there
git fetch origin && git rebase origin/main
```

Resolve conflicts if any (the **resolving-merge-conflicts** skill). The branch
must sit directly on `main` so the review and the merge see only this feature.
Check the size: `git diff --name-only origin/main..HEAD | wc -l` must stay under
149; split along a seam if it does not.

## 2. Max-effort review (parallel finders)

The diff: `git diff origin/main...HEAD` (exclude `Cargo.lock` from reading; a
dependency change is reviewed from `Cargo.toml`).

Spawn **independent finder subagents in parallel** (one Agent tool call with
several invocations), each over the same diff with a different lens. Scale the
count to the feature (4-6 is typical):

- **Correctness** - line by line: inverted conditions, off-by-one, `unwrap` on
  data from disk or the network, error paths that leave a half-written state,
  JSON shape vs what the CLI actually writes, clap flag defaults and conflicts.
- **Credential safety** - the rules in `CLAUDE.md`: can any path lose a refresh
  token (sync-back skipped, the active credential refreshed, tokens filed under
  an unconfirmed account, a failed refresh saved)? Does a write touch keys
  outside the login's? Is the store lock held for every read-modify-write and
  never across a wait on the user? Are new files 0600 and dirs 0700?
- **Security** - `serve`: every `/api/` route behind the token, `Host` and
  `Origin` checks, no data on `GET /`, body caps, output escaped in the page
  (`esc()` for anything from the store). OAuth: state checked, PKCE verifier
  never logged, no token in an error message, URL or log line. Name validation
  on every path that builds a filename. Shell scripts quoted.
- **Contracts** - the store layout and sidecar keys against TokenGauge's ADR
  0003; the release asset names against `src/project.rs`'s tests; OAuth
  constants still matching the CLI versions the README names; `ls --json`
  field names (the page and other tools read them).
- **Reuse / simplify** - does new code re-implement something in `ops.rs`,
  `commands.rs`, `fsx.rs` or `pkce.rs`? Provider-specific logic leaking out of
  `claude.rs`/`codex.rs` into shared code? A special case where the `Provider`
  trait should grow a method?
- **Tests** - do the tests cover the new branches: refusals, offline
  confirmation, the other provider, a corrupt file, a cancelled sign-in? Do any
  tests reach the network or the developer's real files (they must not; see
  `brain/architecture/testing.md`)?

Each finder returns findings as JSON objects `{file, line, severity, summary,
failure_scenario}`, verified (quote the line), most-severe first. Tell them NOT
to fix anything. Then optionally run one **sweep** finder that has the merged
list and hunts only for gaps.

## 3. Fix what's confirmed

Triage the findings. Re-verify a claim before fixing it - finders surface
plausible-but-wrong items too. Fix every confirmed correctness, credential-safety
and security issue, and the worthwhile quality ones, as new commits on top
(`[<branch>] fix(<area>): ...`), never by rewriting the reviewed layers. For
anything real but out of scope, open a GitHub issue (`gh issue create`) rather
than dropping it.

## 4. Update the brain

Bring `brain/` in line with what the feature changed - it ships in the SAME
merge, not as a follow-up. Apply the **brain** skill (it has the checklist); the
essentials:

- New command or behaviour -> `brain/features/<name>.md` plus the rows in
  `brain/features/index.md` and `brain/BRAIN.md`.
- Changed a provider, the store, the page or distribution -> the matching
  `brain/architecture/*.md`.
- Changed the store layout or sidecar keys -> also TokenGauge's ADR 0003, on a
  TokenGauge branch, and say so in the PR.
- New non-obvious decision -> `brain/decisions.md` with its why. New term ->
  `brain/glossary.md`.
- `CHANGELOG.md` `[Unreleased]` has an entry for every user-facing change;
  README's commands table matches `remuda --help`.

Commit as `[<branch>] docs: ...`, after checking the brain's relative links
resolve.

## 5. Gate (must be green before merge)

```bash
cargo fmt --all --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
scripts/coverage.sh                  # look for a logic file drifting down the list
```

Every commit must pass on its own, not just the tip; check from clean exports
(a stale `target/` hides a broken layer):

```bash
w=$(mktemp -d); export CARGO_TARGET_DIR="$w-target"
for c in $(git rev-list --reverse origin/main..HEAD); do
  rm -rf "$w"/* && git archive "$c" | tar -x -m -C "$w"
  (cd "$w" && cargo clippy -q --all-targets --locked -- -D warnings && cargo test -q --locked) \
    >/dev/null 2>&1 && echo "ok   $(git log -1 --format=%s "$c")" || echo "FAIL $(git log -1 --format=%s "$c")"
done
rm -rf "$w" "$w-target"; unset CARGO_TARGET_DIR
```

Then smoke-test by hand against a scratch store
(`REMUDA_STORE=$(mktemp -d) target/debug/remuda ...`); your real logins are
read, never written, as long as you do not run `use`. If the feature touched
`serve`, open the page and exercise it (the **claude-in-chrome** skill).

## 6. Push and let CI run

```bash
git push --force-with-lease origin <branch>   # the rebase rewrote it
gh pr create ... || gh pr edit ...            # CI runs on pull requests
gh pr checks --watch
```

CI runs fmt, clippy and tests on x86_64 and aarch64, plus shellcheck. It must be
green.

## 7. Merge (keep the layers)

Fast-forward `main` onto the branch rather than squashing, so the layers survive:

```bash
cd ~/repos/remuda
git switch main && git pull --ff-only
git merge --ff-only <branch>
git push origin main                          # GitHub marks the PR merged
```

## 8. Release

Run the **release** skill. Minor for a user-facing feature, patch for fix-only.

## 9. Clean up

```bash
wt remove <branch>                            # worktree and branch
git push origin --delete <branch>
```

## Done when

The feature is on `main`, the brain reflects it, every layer and CI were green,
a tag is released, and the branch and worktree are gone. Report the version and
a one-line summary of what the review fixed.
