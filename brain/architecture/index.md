# Architecture

| Doc | Covers |
| --- | --- |
| [overview.md](overview.md) | The modules and how a command flows through them |
| [store.md](store.md) | The credential store on disk, its lock, and the TokenGauge contract |
| [providers.md](providers.md) | The `Provider` trait, Claude Code and Codex, their OAuth details |
| [config.md](config.md) | `config.toml`: auto mode's settings, their checks, writes that keep comments |
| [web.md](web.md) | `remuda serve` and `open`: the listener, its guards, the API the page calls, the service |
| [distribution.md](distribution.md) | Release workflow, installer, refresh timer, serve service, self-update |
| [testing.md](testing.md) | How the suite stays off the network and off real logins |
