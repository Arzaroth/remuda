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
- [ ] `remuda run`: which files under `CODEX_HOME` Codex writes during a
      session (sessions, history, logs) and whether symlinking them is safe.
- [ ] Hub and satellites: hand over credentials or proxy the API. Check
      first whether Claude Code and Codex accept an OAuth login behind a custom
      base URL, and how each copes with a credential that has no refresh token.
- [ ] Hub and satellites: pairing and per-satellite tokens, TLS on a plain LAN.
- [ ] Banked resets: check Codex's credit fields against a real answer, and
      agree with TokenGauge on adding grants and credits to its snapshot.
- [ ] Management page: the config file format and location.
- [ ] More providers: whether `cursor-agent` honours a config-dir override,
      which `remuda run` needs.
- [ ] More providers: check Grok, Kimi and Cursor sign-in, refresh and
      identity against real accounts. They are built from the CLIs' source
      (Grok, Kimi) and from open-source clients (Cursor), and only the mocks
      have run them.
