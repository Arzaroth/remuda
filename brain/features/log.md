# The log

remuda keeps a log of what it did, so "why is this CLI on that login?" has
an answer after the fact, and after a restart. `remuda log` prints it and the
page's Log shows it.

## What is logged

One line per event, each about a provider, a login (`provider/name`) or the
whole store:

| Kind | When |
| --- | --- |
| `switch` | A switch, with who made it (`cli`, `page` or `auto`) and, for auto mode, why. A switch to the login already in use is not one. An auto mode switch that fails is logged against the CLI |
| `auto` | Auto mode's decision for a CLI changed kind or target, or its config cannot be read: the same changes the service's log gets |
| `refresh` | A credential was refreshed, or could not be: by the CLI, the timer or the page, and before a switch or a keep-warm ping (`ops::refresh_saved`) |
| `warm` | A keep-warm ping opened a window, or failed |
| `reset` | A banked reset was spent from the page, or refused |
| `proxy` | The proxy turned a CLI's active login down (signed out, expired, unreadable), and when it spends it again |
| `store` | A login was stored by `import` or a sign-in, renamed or removed |

Labels are not logged. A failed refresh the log already says is not written
again (`logbook::note_once`): a revoked spare the timer fails to refresh every
half hour is one line, not 48 a day. Anything that went right about that login
since (a refresh, a sign-in, a switch) lets the next failure through, and a
failure about a whole CLI is cleared by a success about any of its logins.
`serve` gates its own failures in memory instead, as it does its service log
lines: auto mode's and keep-warm's are logged when their error changes, and
the proxy's when a CLI's refusal changes or ends, since a refusal is checked
on every request.

No line carries a token: messages are the ones the command prints. An error
from a token or usage endpoint can quote up to a few hundred characters of
its answer, as on stderr, which those endpoints never fill with a secret.

## Reading it

`remuda log [NAME] [-n N] [--json]` prints the latest `N` (50) events, oldest
first, in local time. `NAME` narrows to a provider (`claude`), a login
(`claude/work`) or a bare name under any provider (`work`). An event keeps
the name the login had at the time.

The page's Log, in the sidebar, shows the latest 500 (`GET /api/log`), newest
first and grouped by day, narrowed by CLI or to failures. A new line wakes the
page as a store change does.

## Where it is

`<store>/.log.jsonl`, one JSON object per line, 0600: `at` (ms), `kind`,
`provider`, `name`, `by`, `ok`, `message`. At 1 MiB it moves to
`<store>/.log.1.jsonl`, replacing the one there, so the log never holds much
more than two MiB. Appends and reads take a `flock` on `<store>/.log.lock`
(0600) of their own, never the store's, since most appends are made under the
store lock; a read under it never sees a rotation half done. An append cut
short costs that line only. A new line moves `App::fingerprint`, so it wakes
the page, and `serve`'s rounds, as a store change does. Writing is best effort: a log that cannot be written
never fails the command it describes. Nothing is logged when there is no
store, so the log never starts one.

## Sources

- [src/logbook.rs](../../src/logbook.rs)
- [src/ops.rs](../../src/ops.rs) `switch_when`, `refresh_saved`
- [src/commands.rs](../../src/commands.rs) `log`, `refresh`, `import`, `save_login`, `rename`, `remove`
- [src/serve.rs](../../src/serve.rs) `auto_round`, `warm_round`, `spent`, `/api/log`
- [src/proxy/forward.rs](../../src/proxy/forward.rs) `pick`
- [web/src/components/Log.tsx](../../web/src/components/Log.tsx), [web/src/log.ts](../../web/src/log.ts)
