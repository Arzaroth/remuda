# Keep-warm

A subscription's 5-hour window starts at its first message. While none is open,
the next reset is not coming any closer. Keep-warm sends the smallest request a
chosen login takes as soon as its window closes, so one is always running: a
ping at 7am gives an hour of the 7-12 window to a day that starts at 11, then a
fresh window at noon. Nothing is lost by opening a window early.

Logins opt in by name in `config.toml`'s `[warm]` table
([architecture/config.md](../architecture/config.md)), or with the "Keep the
5-hour window open" chips on the Automation tab of each CLI's page.
It is independent of auto mode: either works with the other off.

## Who runs it

`App::warm_round`, right after auto mode's round in `serve::watch`: when the
store, TokenGauge's snapshot or the config changes, and at least once a minute.
No `serve`, no keep-warm. `remuda auto` prints a line per warm login (due now,
open until its reset, or why not) and pings nothing.

## When a login is due

`warm::due`, from TokenGauge's snapshot read as auto mode reads it
(`auto::figures`, so a stale or missing snapshot holds every login):

- its figures have a 5-hour (300-minute) window that is not a model's own
  limit; that, not a list, is what "a provider with a 5-hour window" means;
- the window has no reset in the future (a passed `resetsAt` counts as closed,
  as `auto::current` has it);
- if remuda pinged it before, TokenGauge has asked about this login since that
  ping (its own `askedAt`, or the snapshot's time when TokenGauge gives none),
  and at least 5 minutes have passed. TokenGauge carries a login the CLI is not
  on between asks, so a newer snapshot alone could still show the window
  closed, and every round would ping again;
- after a failed ping, 10 minutes have passed. The failure stays on the page
  until then.

A login with no figures, an error in its figures, or no 5-hour window is not
pinged, and the page says why. Neither is one a `remuda run` session holds
([run.md](run.md)): its tokens are the session's to refresh.

## The ping

`proxy::ping` reuses the proxy's per-provider plumbing, so any provider the
proxy can reach can be kept warm with no code of its own: the cheapest model
(`route::cheapest`), the provider's first format, a one-word prompt with one
output token (Responses has no such cap, so its reasoning is set low), the
provider's `shape` and `upstream`, a 60 s deadline. It succeeds on a 2xx whose
stream reaches its end (`translate::finished`). Then TokenGauge is nudged so it
sees the new window; a failed ping nudges nothing. A provider with no model to
ping (Cursor) cannot be kept warm: the page offers it no chips and the config
check refuses it.

Which credentials (`warm::creds`):

- the login the CLI is on: the CLI's live file, never refreshed; once its
  access token has expired the ping fails until the CLI renews it;
- any other: the stored copy, refreshed and saved first when it has under five
  minutes left, under the store lock. The lock is never held across the ping.

Which login is live is decided again under the lock that reads the
credentials (`ops::sync_live`), so a switch made during an earlier ping is
seen. A CLI whose live login remuda cannot tie to a stored one gets no pings:
it could be any of them. A stored login that does not match its sidecar is
neither spent nor refreshed.

Pings run one at a time on the watcher's thread, so a slow upstream holds auto
mode's next round for up to the ping's deadline.

## What the page shows

`/api/state`'s `warm` has, per `provider/name`, the last round's outcome:
`pinged` (when, which model), `failed` (the error), `open` (until when) or
`waiting` (why). A login's row carries a "Kept warm" pill and its details
show the outcome as one line. The service's log gets a
line for each ping that went through, and one for a failure each time its
error changes.

## Sources

- [src/warm.rs](../../src/warm.rs)
- [src/proxy/ping.rs](../../src/proxy/ping.rs)
- [src/serve.rs](../../src/serve.rs) `warm_round`
- [src/config.rs](../../src/config.rs) `warm`
- [web/src/components/AutoMode.tsx](../../web/src/components/AutoMode.tsx), [web/src/warm.ts](../../web/src/warm.ts)
