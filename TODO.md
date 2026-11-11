# TODO

Small, concrete work: fixes, chores, decisions to make before a roadmap item
starts. Features go in [ROADMAP.md](ROADMAP.md). Delete an entry when it is
done.

## Decisions before roadmap work

- [ ] `remuda run`: check against real Claude Code and Codex sessions that
      what they write under their directory goes through the links or comes
      back at the end, and that two terminals joined in one session refresh
      without signing each other out.
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
- [ ] Another machine: check against a real Cursor login that a
      copied refresh token keeps refreshing on both machines. That it does not
      rotate comes from open-source clients, not from cursor-agent.
- [ ] Another machine: whether a browser profile kept per login
      (Chromium's `--user-data-dir` under remuda's data directory, instead of
      a private window) turns a later sign-in into one click. Sign a Claude
      login in through one, sign it in again days later, and see whether
      claude.ai still asks for the emailed link; the same for ChatGPT.
- [ ] Banked resets: check Codex's credit fields against a real answer, and
      agree with TokenGauge on adding the credits to its snapshot.
- [ ] More providers: whether `cursor-agent` honours a config-dir override,
      which `remuda run` needs.
- [ ] More providers: check Grok, Kimi and Cursor sign-in, refresh and
      identity against real accounts. They are built from the CLIs' source
      (Grok, Kimi) and from open-source clients (Cursor), and only the mocks
      have run them.

## Proxy LAN firewall check

Left from the review of PR #77 (`src/proxy/firewall.rs`, `web/src/proxy.ts`).

- [ ] ufw: match interface wildcards (`-i en+`) when deciding whether a rule
      is on the LAN interface; today such a rule is skipped as another
      interface's.
- [ ] firewalld: tell a D-Bus or management error from `firewall-cmd --state`
      apart from a stopped daemon, and give a null verdict for it rather than
      no firewall.
- [ ] firewalld: run the fallback `--get-zone-of-interface` lookup with
      `sudo`, so it works for a user without the permission.
- [ ] firewalld: when a zone is bound to sources, ask for the zone that
      covers the LAN instead of suggesting the interface's.
- [ ] firewalld: when the zone has rich rules, point at them
      (`--list-rich-rules`) in the notice.
- [ ] Card notice: call the commands suggestions rather than promising they
      open the port, name the interface they apply to, tell a failed check
      apart from rules that limit some machines, and test the whole message.
- [ ] brain/architecture/proxy.md: an `ACCEPT` default policy with a limited
      deny matching gives a null verdict; say so in the ufw row.
- [ ] CHANGELOG: describe the firewall commands as suggestions.
- [ ] Tests: a firewalld fixture where the default zone and the interface's
      zone differ, so a regression asking the wrong zone fails.
