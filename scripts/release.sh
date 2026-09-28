#!/bin/bash
# Usage: scripts/release.sh <x.y.z> [--dry-run]
#
# Moves CHANGELOG.md's [Unreleased] into a dated section, bumps Cargo.toml and
# Cargo.lock, runs the gate, commits, tags vX.Y.Z and pushes. The tag push is
# what starts .github/workflows/release.yml.

set -euo pipefail

version="${1:-}"
dry_run=false
[[ ${2:-} == --dry-run ]] && dry_run=true

if [[ ! $version =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "usage: scripts/release.sh <x.y.z> [--dry-run]" >&2
  exit 2
fi

cd "$(dirname "$(readlink -f "$0")")/.."

branch=$(git branch --show-current)
if [[ $branch != main ]]; then
  echo "release: on $branch, not main" >&2
  exit 1
fi
if [[ -n $(git status --porcelain) ]]; then
  echo "release: the working tree is not clean" >&2
  exit 1
fi
git fetch -q origin main
if [[ $(git rev-parse HEAD) != $(git rev-parse origin/main) ]]; then
  echo "release: main is not level with origin/main" >&2
  exit 1
fi
if git rev-parse -q --verify "refs/tags/v$version" >/dev/null; then
  echo "release: v$version already exists" >&2
  exit 1
fi

pending=$(awk '
  /^## \[Unreleased\]/ { found = 1; next }
  found && /^## / { exit }
  found { print }
' CHANGELOG.md | sed '/./,$!d')
if [[ -z ${pending//[[:space:]]/} ]]; then
  echo "release: CHANGELOG.md has nothing under [Unreleased]" >&2
  exit 1
fi

date=$(date -u +%F)
printf 'releasing v%s (%s) with:\n\n%s\n\n' "$version" "$date" "$pending"
if $dry_run; then
  echo "dry run: nothing written"
  exit 0
fi

restore() {
  git checkout -- CHANGELOG.md Cargo.toml Cargo.lock
  echo "release: the gate failed, the bump was undone" >&2
}
trap restore ERR

awk -v hdr="## [$version] - $date" '
  /^## \[Unreleased\]/ { print; print ""; print hdr; next }
  { print }
' CHANGELOG.md >CHANGELOG.md.new
mv CHANGELOG.md.new CHANGELOG.md
sed -i "0,/^version = \".*\"/s//version = \"$version\"/" Cargo.toml
cargo update -q -p remuda --offline

cargo fmt --check
cargo clippy -q --all-targets --locked -- -D warnings
cargo test -q --locked
printed=$(cargo run -q --locked -- --version)
if [[ $printed != "remuda $version" ]]; then
  echo "release: the binary reports '$printed'" >&2
  false
fi
trap - ERR

git add CHANGELOG.md Cargo.toml Cargo.lock
git commit -q -m "[main] chore(release): $version"
git tag -a "v$version" -m "remuda $version"
git push -q origin main "v$version"
echo "released v$version: the Release workflow is building it now"
