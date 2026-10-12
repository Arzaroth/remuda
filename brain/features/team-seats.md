# Team seats

A Claude Team plan sells two seats, Standard and Premium, and TokenGauge weighs
them differently in its combined figure (1.25x and 6.25x a Pro). Nothing
reports the seat by name reliably: a premium seat has been reported with a
`seatTier` of `team_tier_1`, and its token's `rateLimitTier` is the
organisation's (`default_raven`), the same as a standard seat's. What gives it
away is its own allowance, a rate-limit tier naming a multiplier
(`default_claude_max_5x`).

So `claude::seat` reads a login on a Team plan (`on_team`: its tokens'
`subscriptionType` says `team`, or its profile reported a `team_` seat tier)
through `selvedge::plans::team_seat`, the rule TokenGauge uses for its
weights:

1. Premium when a rate-limit tier the login carries names a multiplier (or
   says `premium`): `oauthAccount.userRateLimitTier`, which Claude Code writes
   into `.claude.json`, or the token's `rateLimitTier`, which remuda's own
   sign-in fills from the profile's `organization.rate_limit_tier`.
2. Otherwise `oauthAccount.seatTier`: `team_premium` or `team_standard`.
3. Otherwise no seat. A tier nothing explains is not shown, rather than
   printed raw.

`ls` names the seat after the plan (`team premium`), and a Team plan never
prints the multiplier its token's tier names, which describes the seat, not
the plan. `ls --json` carries `team`, `seat` (`standard`, `premium` or null)
and `plan` as `ls` prints it; the page shows that plan.

The premium evidence on a login imported from Claude Code is
`userRateLimitTier` alone, and a profile does not carry it. A heal that
re-identifies a torn credential lays the profile's block over the one kept for
the same account (`ops::profile_over`), so the key survives; a switch writes
`oauthAccount` back exactly as stored.

## Sources

- [src/claude.rs](../../src/claude.rs) `on_team`, `seat`, `seat_word`, `Claude::plan`
- [src/commands.rs](../../src/commands.rs) `seat_shown`, `plan_shown`, `list_json`
- [src/ops.rs](../../src/ops.rs) `heal`, `profile_over`
