---
name: release
description: Cut a remuda release, the whole shebang - docs sweep (CHANGELOG, README, brain, the TokenGauge store contract), then scripts/release.sh (changelog promotion, version bump, gate, commit, annotated tag, push), then watch the Release workflow until the GitHub release has both archives and the installer can fetch it. Use when the user says "cut a release", "release", "tag vX.Y.Z", or "the whole release shebang".
---

# Release shebang

Everything between "the code is on main" and "users can install it". The
mechanics live in `scripts/release.sh`; the value of this skill is the docs
sweep before it and the verification after it. Pushing the tag is what builds
and publishes the release, so there is no undo once step 3 runs: a bad tag means
a new patch release.

## 1. Pre-flight

- On `main`, working tree clean, level with `origin/main` (the script refuses
  otherwise). Stash unrelated dirty files and restore them afterwards.
- Anything still on a feature branch is not in this release: finish it first
  (the **feature-treatment** skill) or leave it out on purpose.
- Last version: `git describe --tags --abbrev=0` (tags are `vX.Y.Z`; before the
  first release there is none). Pick the next semver: minor for a new command,
  provider or user-visible behaviour, patch for fix-only. While on 0.x, a
  breaking change to the store layout is a minor bump that says so in the
  changelog.

## 2. Docs sweep (review against `git diff v<last>..HEAD`)

- **CHANGELOG.md `[Unreleased]`** covers every user-visible change since the
  last tag. Entries are added per branch, so this is a completeness check
  against `git log v<last>..HEAD --oneline`. That section becomes the GitHub
  release notes verbatim.
- **README.md**: the commands table, the install flags, the store layout, and
  the Caveats line naming the Claude Code / codex-cli versions the OAuth
  constants came from.
- **brain/**: every doc a change made wrong is fixed (the **brain** skill has
  the checklist). The features and architecture indexes list what shipped.
- **The TokenGauge contract**: if the store layout or sidecar keys changed,
  TokenGauge's `docs/adr/0003-credentials-under-a-provider.md` must say the
  same. A mismatch there breaks TokenGauge's reader without any error.
- **Dependencies**: `git diff v<last>..HEAD -- Cargo.toml`. A new dependency
  needs no docs entry, but check it builds on both release runners (anything
  linking C is the risk; rustls is fine, native-tls is not).
- Commit the sweep as `[main] docs(release): ...` (the script needs a clean
  tree).

## 3. Cut it

```bash
scripts/release.sh <x.y.z> --dry-run   # prints the [Unreleased] block it will promote
scripts/release.sh <x.y.z>
```

The script moves `[Unreleased]` into `## [x.y.z] - <UTC date>`, sets the
version in `Cargo.toml` and `Cargo.lock`, runs fmt, clippy `-D warnings`, the
tests and a `--version` check, then commits `[main] chore(release): x.y.z`,
creates annotated tag `vx.y.z` and pushes both. If the gate fails it restores
the three files and exits; fix the cause on a branch and start again.

## 4. Watch the Release workflow

```bash
run=$(gh run list -R Arzaroth/remuda --workflow Release --limit 1 --json databaseId --jq '.[0].databaseId')
gh run watch "$run" -R Arzaroth/remuda --exit-status
gh release view v<x.y.z> -R Arzaroth/remuda --json assets --jq '.assets[].name'
```

Expect `remuda-v<x.y.z>-linux-x86_64.tar.gz` and `...-linux-aarch64.tar.gz`. A
failed run leaves a pushed tag without a release: read the failing job
(`gh run view "$run" --log-failed`), fix on `main`, and re-run the workflow
for the same tag with
`gh workflow run Release -R Arzaroth/remuda -f tag=v<x.y.z>` rather than
moving the tag.

## 5. Verify it installs

Install into a throwaway home, so nothing touches the real `~/.local/bin` or
timers:

```bash
tmp=$(mktemp -d)
curl -fsSL https://raw.githubusercontent.com/Arzaroth/remuda/main/scripts/install.sh \
  | HOME="$tmp" bash -s -- --no-timer --no-completions
"$tmp/.local/bin/remuda" --version       # remuda <x.y.z>
rm -rf "$tmp"
```

If the user wants their own install upgraded: `remuda update`, then
`remuda --version`.

## Done when

The tag is pushed, the release has both archives, the throwaway install reports
the new version, and `CHANGELOG.md` has a dated section for it. Report the
version and the release URL.
