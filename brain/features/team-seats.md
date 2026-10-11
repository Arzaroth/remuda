# Team seats

A Claude Team plan sells two seats, Standard and Premium, and TokenGauge weighs
them differently in its combined figure (1.25x and 6.25x a Pro). It goes by the
`seatTier` Anthropic's profile reports, which remuda captures in
`oauthAccount`. When that is wrong or missing, the user says which seat a
login holds:

`remuda seat <name> standard|premium|auto` records the choice as the
sidecar's top-level `seatTier` (`team_standard`, `team_premium`); `auto`
clears it, so the profile's word stands again. It refuses to set a seat on a
login that is not on a Team plan (its tokens' `subscriptionType` says `team`,
or its profile reported a `team_` seat), one whose sidecar does not match its
tokens, and any provider but Claude. `auto` is always allowed, so a login that
left its Team plan can drop a seat it no longer holds; the page keeps the
picker on such a card for the same reason.

The choice describes an account. A login stored again under its name for the
same account keeps it (`keep_seat`); a heal that finds the tokens belong to
another account drops it with `oauthAccount`. A `seatTier` that is not text is
read as no choice rather than a malformed sidecar.

The choice stays out of `oauthAccount` on purpose: a switch restores that block
into Claude Code's `.claude.json`, and a seat the user picked is not something
Claude Code said.

`ls` names the seat beside a Team plan, `(set)` when it was chosen. `ls
--json` carries `team`, `seat` (the choice) and `reportedSeat` (the profile's),
and the page draws a seat picker on a Team login's card from them, posting to
`/api/seat`.

## Sources

- [src/claude.rs](../../src/claude.rs) `on_team`, `reported_seat`, `chosen_seat`, `seat_name`
- [src/commands.rs](../../src/commands.rs) `seat`, `plan_shown`, `keep_seat`
- [src/ops.rs](../../src/ops.rs) `heal`
- [web/src/usage.ts](../../web/src/usage.ts) `seatChoices`, `seatChoice`
