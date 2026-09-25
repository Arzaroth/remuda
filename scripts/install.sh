#!/bin/bash
# Usage: curl -fsSL https://raw.githubusercontent.com/Arzaroth/remuda/main/scripts/install.sh | bash
#        ... | bash -s -- [--version vX.Y.Z] [--no-timer]
#
# Installs the latest release into ~/.local/bin and enables the refresh timer.

set -euo pipefail

repo="${REMUDA_REPO:-Arzaroth/remuda}"
bindir="$HOME/.local/bin"
unitdir="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"
tag=""
want_timer=true

while (($# > 0)); do
  case "$1" in
  --version)
    tag="${2:?--version needs a tag}"
    shift
    ;;
  --no-timer) want_timer=false ;;
  -h | --help)
    echo "usage: install.sh [--version vX.Y.Z] [--no-timer]"
    exit 0
    ;;
  *)
    echo "install: unknown option $1" >&2
    exit 2
    ;;
  esac
  shift
done

case "$(uname -s)" in
Linux) ;;
*)
  echo "install: remuda runs on Linux only" >&2
  exit 1
  ;;
esac

case "$(uname -m)" in
x86_64 | amd64) arch=x86_64 ;;
aarch64 | arm64) arch=aarch64 ;;
*)
  echo "install: no release for $(uname -m)" >&2
  exit 1
  ;;
esac

if [[ -z $tag ]]; then
  latest=$(curl -fsSLI -o /dev/null -w '%{url_effective}' "https://github.com/$repo/releases/latest")
  tag="${latest##*/}"
  if [[ $tag != v* ]]; then
    echo "install: could not find the latest release of $repo" >&2
    exit 1
  fi
fi

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

asset="remuda-$tag-linux-$arch.tar.gz"
echo "Downloading remuda $tag for $arch"
curl -fsSL "https://github.com/$repo/releases/download/$tag/$asset" -o "$tmp/$asset"
tar -xzf "$tmp/$asset" -C "$tmp"

if ! "$tmp/remuda" --version >/dev/null 2>&1; then
  echo "install: the downloaded binary does not run on this system" >&2
  exit 1
fi

mkdir -p "$bindir"
install -m 755 "$tmp/remuda" "$bindir/remuda.new"
mv -f "$bindir/remuda.new" "$bindir/remuda"
echo "Installed $("$bindir/remuda" --version) into $bindir"

case ":$PATH:" in
*":$bindir:"*) ;;
*) echo "install: warning - $bindir is not on your PATH" >&2 ;;
esac

if $want_timer; then
  mkdir -p "$unitdir"
  install -m 644 "$tmp/systemd/remuda-refresh.service" "$tmp/systemd/remuda-refresh.timer" "$unitdir/"
  if command -v systemctl >/dev/null 2>&1 && systemctl --user show-environment >/dev/null 2>&1; then
    systemctl --user daemon-reload
    systemctl --user enable --now remuda-refresh.timer
    echo "Enabled remuda-refresh.timer"
  else
    echo "install: no systemd user session; run 'remuda refresh' yourself to keep stored logins fresh" >&2
  fi
fi

cat <<'EOF'

Next:
  remuda import main      store the account Claude Code is signed into
  remuda login other      sign another account in through the browser
  remuda use other        switch Claude Code to it
EOF
