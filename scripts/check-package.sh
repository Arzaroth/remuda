#!/bin/sh
# Usage: scripts/check-package.sh
#
# Run on a system with a remuda package installed: fails unless the package
# put the binary, the units and the completions where each is looked for.

set -eu

fail() {
  echo "check-package: $*" >&2
  exit 1
}

remuda --version
for unit in remuda-refresh.service remuda-refresh.timer remuda-serve.service; do
  [ -f "/usr/lib/systemd/user/$unit" ] || fail "no /usr/lib/systemd/user/$unit"
done
grep -qx 'ExecStart=/usr/bin/remuda refresh --scheduled' /usr/lib/systemd/user/remuda-refresh.service ||
  fail "remuda-refresh.service does not run /usr/bin/remuda"
grep -qx 'ExecStart=/usr/bin/remuda serve' /usr/lib/systemd/user/remuda-serve.service ||
  fail "remuda-serve.service does not run /usr/bin/remuda"
[ -f /usr/share/bash-completion/completions/remuda ] || fail "no bash completions"
[ -f /usr/share/fish/vendor_completions.d/remuda.fish ] || fail "no fish completions"
[ -f /usr/share/zsh/vendor-completions/_remuda ] || [ -f /usr/share/zsh/site-functions/_remuda ] ||
  fail "no zsh completions"
echo "check-package: everything is in place"
