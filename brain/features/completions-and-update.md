# Completions and self-update

`remuda completions <shell>` prints a clap_complete script for bash, zsh, fish,
elvish or PowerShell. `install.sh` writes bash's to
`$XDG_DATA_HOME/bash-completion/completions/remuda`, zsh's to
`$XDG_DATA_HOME/zsh/site-functions/_remuda` (which may need adding to `fpath`)
and fish's to `~/.config/fish/completions/remuda.fish`. The packages carry
the same three, written at release time, in the system directories.

`remuda --update` replaces the binary with the latest GitHub release through
selvedge; `--check-update` only reports whether one is newer. They are flags,
not subcommands, the way TokenGauge and TailGauge spell them, and neither needs
the store. After replacing the binary, `--update` has the new one rewrite the systemd units
already installed, if they changed, and restart `remuda-serve.service` if it
runs. A binary under `/usr` belongs to a package: once a newer release
exists, `--update` refuses it and says to upgrade through the package
manager.
See [architecture/distribution.md](../architecture/distribution.md).

## Sources

- [src/main.rs](../../src/main.rs) `Completions`, `update`, `Cli.update`
- [scripts/install.sh](../../scripts/install.sh)
