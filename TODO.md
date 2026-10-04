# TODO

Small, concrete work: fixes, chores, decisions to make before a roadmap item
starts. Features go in [ROADMAP.md](ROADMAP.md). Delete an entry when it is
done.

## Decisions before roadmap work

- [ ] Auto-switch: what to do when every stored login is maxed.
- [ ] Auto-switch: which login to pick (most headroom, soonest reset, a fixed
      order), and how the auto-mode score weighs headroom against time until
      reset.
- [ ] Auto-switch: where eligibility lives (sidecar flag or config list), and
      what keeps auto mode from flapping.
- [ ] Auto-switch by rule: which criteria besides time ranges, and where the
      rules are configured.
- [ ] `remuda run`: flock or process check to mark a login in use, and how a
      stale mark from a crashed session gets cleared.
- [ ] `remuda run`: whether Codex gets the same through `CODEX_HOME`.
- [ ] Hub and satellites: hand over credentials or proxy the API. Check
      first whether Claude Code and Codex accept an OAuth login behind a custom
      base URL, and how each copes with a credential that has no refresh token.
- [ ] Hub and satellites: pairing and per-satellite tokens, TLS on a plain LAN.
- [ ] Banked resets: find the endpoints that list and spend them for Claude and
      Codex, and whether TokenGauge should list them first.
- [ ] Management page: pick the framework (vendored Preact+htm, built
      Svelte/Solid, or vanilla), and the config file format and location.
