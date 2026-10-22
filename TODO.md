# TODO

Small, concrete work: fixes, chores, decisions to make before a roadmap item
starts. Features go in [ROADMAP.md](ROADMAP.md). Delete an entry when it is
done.

## Decisions before roadmap work

- [ ] `remuda run`: check against a real Codex session that what it writes
      under `CODEX_HOME` (sessions, history, logs) goes through the links, and
      that a setting it rewrites by rename is not lost with the session's
      directory.
- [ ] Auto mode: a Codex `remuda run` session counts as a running `codex`, so
      it holds the global switch too; tell the two apart by `CODEX_HOME` in
      the process's environment.
- [ ] Auto-switch: whether a session in the middle of a request can be told
      apart, so a switch can wait for it. Until then auto mode does not try.
- [ ] Hub and satellites: hand over credentials or proxy the API. The local
      proxy exists; check how each CLI copes with a credential that has no
      refresh token before ruling the first way out.
- [ ] Proxy: check against real logins that Claude's endpoint takes a
      subscription token with Claude Code's identity line alone (CLIProxyAPI
      also sends a `x-anthropic-billing-header` block and moves the caller's
      system prompt out of `system`, saying a top-level one trips Anthropic's
      third-party classifier), that
      Grok's chat proxy takes the grok 1.0.46 headers, and that
      opencode Go serves each model family on the path the proxy picks.
- [ ] Proxy: a client that asked for a whole answer is written to only at the
      end, so one that gave up is not noticed and the provider keeps
      generating; and an upstream that stalls holds its slot. Check the client
      while collecting, and time out a silent upstream.
- [ ] Proxy: a chat upstream that interleaves parallel tool calls' argument
      deltas by index is read as if each call came whole, in turn.
- [ ] Hub and satellites: pairing and per-satellite tokens, TLS on a plain LAN.
- [ ] Banked resets: check Codex's credit fields against a real answer, and
      agree with TokenGauge on adding the credits to its snapshot.
- [ ] More providers: whether `cursor-agent` honours a config-dir override,
      which `remuda run` needs.
- [ ] More providers: check Grok, Kimi and Cursor sign-in, refresh and
      identity against real accounts. They are built from the CLIs' source
      (Grok, Kimi) and from open-source clients (Cursor), and only the mocks
      have run them.
