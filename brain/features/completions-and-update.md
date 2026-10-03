# Completions and self-update

`remuda completions <shell>` prints a clap_complete script for bash, zsh, fish,
elvish or PowerShell. `install.sh` writes bash's to
`$XDG_DATA_HOME/bash-completion/completions/remuda`, zsh's to
`$XDG_DATA_HOME/zsh/site-functions/_remuda` (which may need adding to `fpath`)
and fish's to `~/.config/fish/completions/remuda.fish`.

`remuda update` replaces the binary with the latest GitHub release through
selvedge; `--check` only reports whether one is newer. Neither needs the store.
After replacing the binary, `update` has the new one rewrite the systemd units
already installed, if they changed, and restart `remuda-serve.service` if it
runs.
See [architecture/distribution.md](../architecture/distribution.md).

## Sources

- [src/main.rs](../../src/main.rs) `Completions`, `update`
- [scripts/install.sh](../../scripts/install.sh)
