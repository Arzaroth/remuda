# TODO

Small, concrete work: fixes, chores, decisions to make before a roadmap item
starts. Features go in [ROADMAP.md](ROADMAP.md). Delete an entry when it is
done.

## Decisions before roadmap work

- [ ] Live page: WebSocket or server-sent events, given what selvedge supports.
- [ ] Auto-switch: what to do when every stored login is maxed.
- [ ] Auto-switch: which login to pick (most headroom, soonest reset, a fixed
      order).
- [ ] Auto-switch by rule: which criteria besides time ranges, and where the
      rules are configured.
- [ ] `remuda run`: flock or process check to mark a login in use, and how a
      stale mark from a crashed session gets cleared.
- [ ] `remuda run`: whether Codex gets the same through `CODEX_HOME`.
