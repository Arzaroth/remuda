# Auto mode

remuda keeps each CLI on the most suitable of the logins the user allowed, and
moves it off one that is maxed. Each CLI is decided apart from the others. Its
settings are `[auto.<cli>]` sections of `config.toml`
([architecture/config.md](../architecture/config.md)), edited on the page or by
hand, and off by default.

## Who runs it

A thread in `remuda serve` (`serve::watch`). It runs a round when the store,
TokenGauge's snapshot or the config changes (looked at every 5 s), and at least
once a minute so a schedule takes effect on time. Auto mode works only while
`serve` runs; `remuda-serve.service` keeps it running.

`remuda auto` prints, for every CLI the config sets up, what a round would
decide now and why, and switches nothing.

## A round

For each CLI with a section (`App::auto_round`), under the store lock:

1. Sync the live login back (`ops::sync_live`). A live login remuda could not
   keep makes the round hold: one not stored, an API key or other login it
   cannot store, one it could not confirm, unreadable files.
2. Read the figures from TokenGauge's snapshot (`auto::figures`). It holds when
   there is no snapshot, none for this CLI, or the snapshot is older than the
   CLI's last switch (by hand or by auto mode): it waits for TokenGauge to
   fetch again. A login whose figures carry an error, are stale or are missing
   cannot be picked this round. A snapshot that names no credential (before
   TokenGauge 0.37) holds the active login's figures only.
3. Decide (`auto::decide`, below).
4. If auto mode is on and the decision is a switch, switch through
   `ops::switch` exactly as `remuda use` does, never discarding, then nudge
   TokenGauge. For Codex, a switch waits while any `codex` process runs
   (`auto::sessions_open`): Codex never takes up a new login while it runs, and
   a refresh it has in flight writes the old account's tokens into the new
   file.

What each CLI's round decided, and the last switch auto mode made, are kept in
memory and sent to the page with the state. The service's log gets a line for
each switch, each failed switch (once per error), and each change of decision.

## The decision

1. **The rule in force** is the first whose schedule holds now; `otherwise`
   holds any time. A schedule's days are local, and a range that ends before
   it starts runs past midnight and belongs to the day it started. With no rule
   in force, the `logins` list is the only tier; with none set, the round holds.
2. **Reservations.** Outside its schedule, a reserved login is kept for it,
   unless its longest window resets before the schedule next starts: then its
   headroom would be lost anyway, so it moves to the first tier and is spent
   first. Inside its schedule a reservation does nothing.
3. **Tiers.** The first tier holding a usable login wins. A login is usable
   when it has figures and none of its windows is at 100%.
4. **The score**, within a tier: the headroom left in the login's longest
   window divided by the hours until it resets (at least one). A login close to
   its weekly reset with headroom left beats one whose week has just started.
5. **Against the active login.**
   - Already on the best pick: stay.
   - The active login is maxed: switch now, whatever the dwell.
   - The active login has no figures: stay, rather than move blind.
   - Switched less than `dwell` ago (default 30 min): stay.
   - The active login is not one the rule allows now: switch.
   - The best pick is in an earlier tier: switch.
   - Same tier: switch only when the best pick scores `margin` (default 25%)
     more.
   - No usable login at all: stay, and switch once one resets.
   - Nothing signed in: switch to the best pick.

remuda cannot tell a session in the middle of a request, so a switch does not
wait for one. Claude Code takes up the new login on its next request.

## Sources

- [src/auto.rs](../../src/auto.rs)
- [src/serve.rs](../../src/serve.rs) `watch`, `App::auto_round`
- [src/commands.rs](../../src/commands.rs) `auto`
- [src/config.rs](../../src/config.rs)
- [web/src/components/AutoMode.tsx](../../web/src/components/AutoMode.tsx)
