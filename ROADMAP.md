# Roadmap

Features remuda may grow next. None is started; order is not priority.
Small fixes and chores go in [TODO.md](TODO.md). When a feature ships, its entry
leaves this file and a doc for it lands in `brain/features/`.

## Auto-switch when a limit is maxed

When the active login's usage window is used up, switch the CLI to a stored
login with headroom left. Needs a watcher (the refresh timer, or `serve`) that
reads TokenGauge's figures, and a rule for which login to pick.

## Auto-switch by rule

Switch on criteria set in advance, e.g. a login per time range (work hours,
evenings). Other criteria are still open.

## `remuda run`

`remuda run <profile>` (or `remuda claude <profile>`) starts a Claude Code
session on one login without switching the one every other session uses. It
builds a temporary `CLAUDE_CONFIG_DIR` whose entries are symlinks to the real
config, so history, settings and MCP logins stay shared, except for the
credentials, which are that login's own.

While the session runs, its login is in use (flock, or a check that the process
is alive) and remuda treats it like the active credential: never refreshed, and
its tokens synced back to the store when the session ends, since Claude Code
may have rotated them.

## Constraints every item keeps

The rules in [CLAUDE.md](CLAUDE.md) hold for all of the above: a switch changes
only the login's keys, the active (or in-use) login is never refreshed, and a
live login is synced back before anything reads the store. Auto-switching must
never drop an unstored live login the way `remuda use --discard` does.
