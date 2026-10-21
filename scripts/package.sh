#!/bin/bash
# Usage: scripts/package.sh <remuda-vX.Y.Z-linux-<arch>.tar.gz> <dist>
#
# Builds the .deb and the .rpm for a release archive into <dist>, from the
# archive itself, so a package carries the bytes `--update` would install.
# Needs nfpm on PATH.

set -euo pipefail

die() {
  echo "package: $*" >&2
  exit 1
}

archive="${1:-}"
dist="${2:-}"
if [[ ! -f $archive || -z $dist ]]; then
  echo "usage: scripts/package.sh <remuda-vX.Y.Z-linux-<arch>.tar.gz> <dist>" >&2
  exit 2
fi
[[ $(basename "$archive") =~ ^remuda-v([0-9]+\.[0-9]+\.[0-9]+)-linux-(x86_64|aarch64)\.tar\.gz$ ]] ||
  die "$archive is not a linux release archive"
version="${BASH_REMATCH[1]}"
case "${BASH_REMATCH[2]}" in
x86_64) arch=amd64 ;;
aarch64) arch=arm64 ;;
esac
command -v nfpm >/dev/null 2>&1 || die "nfpm is required - https://nfpm.goreleaser.com"

root="$(cd "$(dirname "$(readlink -f "$0")")/.." && pwd)"
archive="$(readlink -f "$archive")"
mkdir -p "$dist"
dist="$(readlink -f "$dist")"
stage="$(mktemp -d)"
trap 'rm -rf "$stage"' EXIT

mkdir -p "$stage/archive" "$stage/systemd"
tar -xzf "$archive" -C "$stage/archive"
cp "$root/README.md" "$root/CHANGELOG.md" "$stage/archive/"

# The units run the binary from ~/.local/bin, where install.sh puts it.
for unit in "$stage/archive/systemd/"*; do
  out="$stage/systemd/$(basename "$unit")"
  sed 's|^ExecStart=%h/\.local/bin/remuda |ExecStart=/usr/bin/remuda |' "$unit" >"$out"
  if grep -q '^ExecStart=' "$out" && ! grep -q '^ExecStart=/usr/bin/remuda ' "$out"; then
    die "$(basename "$unit")'s ExecStart is not the one this rewrites"
  fi
done

for format in deb rpm; do
  (cd "$stage" && ARCH="$arch" VERSION="$version" \
    nfpm package --config "$root/packaging/nfpm.yaml" --packager "$format" --target "$dist/")
done
