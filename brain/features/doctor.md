# The doctor

remuda's automations fail quietly: auto mode holds on a stale snapshot,
keep-warm and the proxy stop with `serve`, a spare login whose refresh token
was refused just sits there. `remuda doctor` checks what they rely on and says
what to do about it, and the page's Status shows the same checks.

## The vitals

Each check is a vital: an `id`, a `level` (`ok`, `info`, `warn` or `bad`), a
title, a detail, and when there is one a `fix` and a list of `items`.
`doctor::vitals` makes them, in this order:

| Vital | Says |
| --- | --- |
| `config` | Only when `config.toml` cannot be read: bad, since the rest is then checked as if it were empty |
| `serve` | Whether `remuda serve` runs, whether a remuda unit runs it, and whether its version is this binary's. Not running is bad when auto mode is on for a CLI, `[warm]` lists a login, or the proxy is on, since they run only inside it; info otherwise. Running by hand is a warning in that case too, since it stops with its terminal. The fix restarts the service when it runs an older remuda, and points at `remuda --update` when this binary is the older one |
| `timer` | `remuda-refresh.timer`: enabled, disabled, masked or absent, from systemd's user unit search path (`units::state`), each but enabled a warning with the command that fixes it |
| `refresh` | The last scheduled refresh (`<store>/.last-refresh.json`): bad when it reported problems, which are its items; a warning when the timer is on and it last ran over 75 minutes ago; info when it never ran |
| `tokengauge` | TokenGauge's snapshot: not found (a warning when auto mode or keep-warm needs it, info otherwise), unreadable, over 30 minutes old (when auto mode holds), and the errors it carries per login as items |
| `logins` | Every stored login: one whose refresh token has expired is bad, an unverified one a warning, each an item with the `remuda login ... --force` that signs it in again |
| `live` | Each CLI's live login: unreadable files are bad; a login remuda has not stored, one it cannot store, or one it could not confirm are items, warnings when auto mode is on for that CLI (it holds) and info otherwise |
| `version-<cli>` | For Claude Code and Codex when found on `$PATH`: the installed release (`<cli> --version`) against the one remuda's sign-in was read from (`Provider::mirrors`). A different release is info, not a warning: it only matters once sign-in or refresh breaks. A version is asked once per binary and kept an hour, so the page can ask on every state |
| `pin` | Only for the CLI, when the nearest `.remuda.toml` above the current directory is not trusted (`pins::untrusted`), with `remuda pin --allow` as the fix |
| `artifact-hook` | Only once two Claude logins are stored: whether Claude Code's `settings.json` runs `remuda hook artifact` on `PreToolUse`, `PostToolUse` and `PostToolUseFailure` for its artifact tools (a matcher that is empty, `*`, or names `Artifact`). Missing events are a warning, with `remuda help skill` as the fix ([artifact-hook.md](artifact-hook.md)) |

A detail that says how long ago something happened holds `{since}` and the
vital carries `since`, the time it stands for, so the page words it from its
own clock rather than from when the state was sent.

## `remuda doctor`

Prints one line per vital, its level, title and detail, then its items and
fix indented below. `--json` prints the vitals as they are. Either way it
exits non-zero when a vital is bad. It syncs the live logins back and takes
the store lock, as `ls` does, and finds a running `serve` from the files it
announces itself in ([web.md](../architecture/web.md)), reading the unit it
runs in from `/proc/<pid>/cgroup`.

## On the page

`/api/state`'s `health.vitals` is the same list, made with this process as
the running `serve` and no current directory, so there is no `pin` vital.
Status shows one row per vital, with its fix and a toggle for its items, and
the sidebar's Status dot takes the worst level: amber for a warning, red for
a bad one ([web-page.md](web-page.md)).

## Sources

- [src/doctor.rs](../../src/doctor.rs)
- [src/units.rs](../../src/units.rs) `state`
- [src/served.rs](../../src/served.rs) `serving`
- [src/pins.rs](../../src/pins.rs) `untrusted`
- [src/doctor.rs](../../src/doctor.rs) `artifact_hook`
- [src/serve.rs](../../src/serve.rs) `App::health`
- [web/src/components/Health.tsx](../../web/src/components/Health.tsx)
- [web/src/vitals.ts](../../web/src/vitals.ts)
