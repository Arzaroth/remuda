#!/bin/bash
# Usage: curl -fsSL https://raw.githubusercontent.com/Arzaroth/remuda/master/scripts/install.sh | bash
#        ... | bash -s -- [--version vX.Y.Z] [--no-timer] [--serve] [--no-completions]
#
# Installs the latest release into ~/.local/bin, enables the refresh timer and
# installs completions for the shells it finds. --serve also keeps the page
# served by a user service, for `remuda open`. The archive is checked against
# the release's SHA256SUMS before anything runs; REMUDA_DOWNLOAD_BASE (a URL or
# a directory) takes the assets from somewhere else than the GitHub release.

set -euo pipefail

die() {
  echo "install: $*" >&2
  exit 1
}

main() {
  repo="${REMUDA_REPO:-Arzaroth/remuda}"
  bindir="$HOME/.local/bin"
  unitdir="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"
  tag=""
  want_timer=true
  want_serve=false
  want_completions=true

  while (($# > 0)); do
    case "$1" in
    --version)
      tag="${2:?--version needs a tag}"
      [[ $tag == v* ]] || tag="v$tag"
      shift
      ;;
    --no-timer) want_timer=false ;;
    --serve) want_serve=true ;;
    --no-completions) want_completions=false ;;
    -h | --help)
      echo "usage: install.sh [--version vX.Y.Z] [--no-timer] [--serve] [--no-completions]"
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

  base="${REMUDA_DOWNLOAD_BASE:-}"
  if [[ -z $tag && -z $base ]]; then
    latest=$(curl -fsSLI -o /dev/null -w '%{url_effective}' "https://github.com/$repo/releases/latest")
    tag="${latest##*/}"
    [[ $tag == v* ]] || die "could not find the latest release of $repo"
  fi
  case "$base" in
  "") base="https://github.com/$repo/releases/download/$tag" ;;
  /*) base="file://$base" ;;
  esac

  tmp=$(mktemp -d)
  trap 'rm -rf "$tmp"' EXIT

  curl -fsSL "$base/SHA256SUMS" -o "$tmp/SHA256SUMS" ||
    die "no SHA256SUMS in $base to check the download against"
  if [[ -n $tag ]]; then
    asset="remuda-$tag-linux-$arch.tar.gz"
  else
    asset=$(sed -n "s/^[0-9a-f]\{64\}  \(remuda-v[0-9][^ ]*-linux-$arch\.tar\.gz\)$/\1/p" "$tmp/SHA256SUMS")
    [[ -n $asset ]] || die "no linux-$arch archive in $base"
    [[ $(wc -l <<<"$asset") -eq 1 ]] || die "several linux-$arch archives in $base: pick one with --version"
  fi
  expected=$(awk -v f="$asset" '$2 == f { print $1 }' "$tmp/SHA256SUMS")
  [[ -n $expected ]] || die "$asset is not in $base/SHA256SUMS"

  echo "Downloading $asset"
  curl -fsSL "$base/$asset" -o "$tmp/$asset"
  actual=$(sha256sum "$tmp/$asset" | cut -d' ' -f1)
  [[ $expected == "$actual" ]] || die "$asset does not match its SHA256SUMS entry"
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

  if $want_timer || $want_serve; then
    mkdir -p "$unitdir"
    if $want_timer; then
      install -m 644 "$tmp/systemd/remuda-refresh.service" "$tmp/systemd/remuda-refresh.timer" "$unitdir/"
    fi
    if $want_serve; then
      install -m 644 "$tmp/systemd/remuda-serve.service" "$unitdir/"
    fi
    # The units must see the same directories as the shell, or the timer takes
    # the active login for an inactive one and refreshes it.
    envdir="${XDG_CONFIG_HOME:-$HOME/.config}/environment.d"
    envs=()
    for var in CLAUDE_CONFIG_DIR CODEX_HOME GROK_HOME GROK_AUTH_PATH KIMI_CODE_HOME CURSOR_CONFIG_DIR REMUDA_STORE; do
      if [[ -n ${!var:-} ]]; then
        envs+=("$var=${!var}")
      fi
    done
    if ((${#envs[@]} > 0)); then
      mkdir -p "$envdir"
      printf '%s\n' "${envs[@]}" >"$envdir/60-remuda.conf"
      echo "Wrote $envdir/60-remuda.conf: ${envs[*]}"
    fi
    # The units act only where the last interactive command looked; this is
    # that command, run from the installing shell. With no store yet it fails,
    # and the first import or login records instead.
    "$bindir/remuda" ls >/dev/null 2>&1 || true
    stored=false
    if [[ -d ${REMUDA_STORE:-${XDG_DATA_HOME:-$HOME/.local/share}/remuda/credentials} ]]; then
      stored=true
    fi
    if command -v systemctl >/dev/null 2>&1 && systemctl --user show-environment >/dev/null 2>&1; then
      if ((${#envs[@]} > 0)); then
        systemctl --user set-environment "${envs[@]}"
      fi
      systemctl --user daemon-reload
      if $want_timer; then
        systemctl --user enable --now remuda-refresh.timer
        echo "Enabled remuda-refresh.timer"
      fi
      if $want_serve; then
        systemctl --user enable remuda-serve.service
        if $stored; then
          systemctl --user restart remuda-serve.service
          echo "Enabled remuda-serve.service; 'remuda open' opens its page"
        else
          echo "Enabled remuda-serve.service; 'remuda open' starts it once 'remuda import' or 'remuda login' has made the store"
        fi
      fi
    else
      echo "install: no systemd user session; run 'remuda refresh' yourself to keep stored logins fresh, and 'remuda serve' for the page" >&2
    fi
  fi

  # A service installed by an earlier --serve would keep running the old binary.
  if ! $want_serve && command -v systemctl >/dev/null 2>&1 &&
    systemctl --user is-active --quiet remuda-serve.service 2>/dev/null; then
    systemctl --user restart remuda-serve.service
    echo "Restarted remuda-serve.service"
  fi

  if $want_completions; then
    data="${XDG_DATA_HOME:-$HOME/.local/share}"
    if command -v bash >/dev/null 2>&1; then
      mkdir -p "$data/bash-completion/completions"
      "$bindir/remuda" completions bash >"$data/bash-completion/completions/remuda"
    fi
    if command -v zsh >/dev/null 2>&1; then
      mkdir -p "$data/zsh/site-functions"
      "$bindir/remuda" completions zsh >"$data/zsh/site-functions/_remuda"
      echo "zsh completions are in $data/zsh/site-functions; add it to fpath if it is not there yet"
    fi
    if command -v fish >/dev/null 2>&1; then
      fishdir="${XDG_CONFIG_HOME:-$HOME/.config}/fish/completions"
      mkdir -p "$fishdir"
      "$bindir/remuda" completions fish >"$fishdir/remuda.fish"
    fi
  fi

  cat <<'EOF'

Next:
  remuda import main            store the account Claude Code is signed into
  remuda import -p codex main   the same for Codex
  remuda login other            sign another account in through the browser
  remuda use other              switch to it
  remuda serve                  do all of that from a page in the browser
  remuda open                   open the page install.sh --serve keeps running
EOF
}

# Last, so a download cut short runs nothing at all.
main "$@"
