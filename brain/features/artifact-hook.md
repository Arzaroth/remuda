# The artifact hook and the skill

A switch reaches every open Claude Code session, the Artifact tools included
([decisions](../decisions.md#a-switch-reaches-every-session-so-artifacts-get-a-hook)).
An artifact belongs to the claude.ai account that published it, so after a
switch a session's own artifacts are refused, or republished under the other
account. `remuda hook artifact`, run by Claude Code's hooks on its artifact
tools, switches to the login that published the artifact for the length of
the call, then back.

## What it reads

Claude Code sends each hook event as JSON on stdin: `hook_event_name`,
`session_id`, `tool_name`, `tool_input`, and on `PostToolUse` the
`tool_response`. Only `Artifact`, `ArtifactComments` and `ArtifactData` are
acted on (`artifacts::TOOLS`); anything else, and any other event, is ignored.
The hook never fails the call: an error goes to stderr and it exits 0. With no
store, it does nothing.

An artifact is named by the last segment of its link
(`claude.ai/artifact/<slug>`, `claude.ai/code/artifact/<id>`), the query and
fragment aside (`artifacts::key`). A republish names no link: it passes the
same `file_path`, so the session's file paths are kept too.

## The ledger

`<store>/.artifacts.json`, outside any provider's directory:

| Key | Holds |
| --- | --- |
| `owners` | An artifact's slug, and its `artifact_id` when a publish gave one, mapped to the `accountId` it was published or listed under |
| `sessions.<id>.paths` | The file each artifact was published from in that session |
| `sessions.<id>.restore` | The login a call was switched to (`on`) and the one to go back to (`back`), while the call runs |
| `sessions.<id>.seen` | Last event; a session unseen for 30 days with nothing to restore is dropped |

Owners are learnt after a successful call (`PostToolUse`), under the account
the call ran on: a publish's `url`, `artifact_id` and `path`, and every row of a
`list` of the account's own artifacts (no `scope`, or `mine`, and no `type`).
A `shared` or `all` listing and a `read` teach nothing, since those can be
another account's. An artifact published before the hook was installed is
learnt the first time its account lists it.

## A call

**PreToolUse.** The call's artifact is its `url`, else the session's file for
its `file_path`. With no owner on record, or the owner's account live, nothing
happens. Otherwise the stored login of that account (verified, matched by
`accountId`) is switched to through `ops::switch_when`, by `By::Hook`, which the
log shows as `[artifact hook]`, and the restore is noted. Consecutive calls
keep the first `back`, so a run of calls returns to the login before the
first; a restore whose `on` is no longer live was left by a call that never
returned, and the live login becomes `back` instead. The hook says what it did as `additionalContext`, so the session knows.

When another session's call is on the login this one must leave (its restore
has `on` equal to the live login), the hook waits, unlocked, for up to 30
seconds, then switches anyway.

An owner remuda does not store, or a switch that is refused (the login is in a
run session, say), is said to the session as context; the call goes ahead and
is likely refused.

**PostToolUse and PostToolUseFailure.** After learning from the result, the
restore is taken and the CLI switched back to `back`, unless it is no longer
on `on`: something else moved it meanwhile, and the hook leaves it there.

A restore no event came for (the session died mid-call) is made by the next
hook of another session once it is 10 minutes old.

Each switch is an ordinary one and starts auto mode's dwell. The switch to the
publishing login also waits for TokenGauge's next fetch, which keeps auto mode
from moving the CLI under the call; the switch back nudges TokenGauge to fetch
(`gauge::nudge`), as `remuda use` does, since the call is over. The other open
sessions run on the publishing login for the call's length.

## Run sessions

Inside `remuda run` (`REMUDA_OUTER_CLAUDE_CONFIG_DIR` set and
`CLAUDE_CONFIG_DIR` at `<store>/.runs/claude/<name>/home`, `main::run_login`)
the session keeps its login whatever the CLI's files hold, so the hook never
switches. It learns owners under the run's login, and says so when a call
names an artifact another login published.

## Installing it, and the skill

`remuda help skill` prints a SKILL.md (`src/skill.md`, compiled in) for Claude
Code sessions: what a switch does to them, what the hook does, the hook lines
for `~/.claude/settings.json` (`PreToolUse`, `PostToolUse` and
`PostToolUseFailure`, matcher `^Artifact`), what to do by hand without it, and
the commands. Its description asks to be loaded before any artifact call. A
skill is loaded at the model's discretion, so the hook is what makes the
switch happen every time; the skill is how a session knows.

`remuda doctor` checks the hook once two Claude logins are stored
([doctor.md](doctor.md)).

## Sources

- [src/artifacts.rs](../../src/artifacts.rs)
- [src/main.rs](../../src/main.rs) `hook`, `run_login`, `help`
- [src/skill.md](../../src/skill.md)
- [src/doctor.rs](../../src/doctor.rs) `artifact_hook`, `hooked`
