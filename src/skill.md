---
name: remuda
description: remuda switches which login Claude Code, Codex and other CLIs are signed into, sometimes on its own (auto mode), and every open Claude Code session follows the switch. Load this skill before any Artifact, ArtifactComments or ArtifactData call (publish, read, update, comment, list, delete), because an artifact belongs to the claude.ai account that published it and that account may no longer be the one signed in. Also load it when asked which account or login is in use, why usage or quota moved, or to switch, pin or run a login.
---

# remuda

remuda keeps several logins per CLI and switches the one each CLI uses. A
switch rewrites Claude Code's credential file, and Claude Code reads that file
before every request, so **every open session moves to the new account at
once**: model calls, and the Artifact tools too. Auto mode (`remuda serve`)
switches on its own when a login fills up or a schedule says so.

## Artifacts

An artifact belongs to the claude.ai account that published it. After a
switch, a call on an artifact this session published earlier runs as the other
account: it is refused (not found, no write access), and a republish without a
`url` would make a new artifact under the wrong account.

remuda's artifact hook handles this on its own. It notes which account
publishes or lists each artifact, and before a call on one that another stored
login published, it switches Claude Code to that login, then back once the
call returns. Its notes come back to you as additional context, starting with
`remuda`. The other open sessions are on that login for the length of the call.

Check that the hook is installed: `remuda doctor` says so under "Claude Code
artifact hook". When it is not, install it (below) or, before each artifact
call:

1. Run `remuda ls --json` and note the Claude login whose `active` is true.
2. When you publish, say in your reply which login (name and email) published
   the artifact, beside its link.
3. Before a later call on that artifact, check the active login again. If it
   changed, run `remuda use claude/<the publishing login>`, make the call, then
   `remuda use claude/<the login that was active>`, and tell the user you did.
4. An artifact you cannot place (made in an earlier conversation) that comes
   back not found or without write access was most likely published by the
   other login: say so instead of publishing a copy.

Inside a `remuda run` session (`REMUDA_OUTER_CLAUDE_CONFIG_DIR` is set) the
session keeps its own login whatever happens to the CLI's: never run
`remuda use` for an artifact there.

### Installing the hook

Add to `~/.claude/settings.json` (merge with any `hooks` already there):

```json
{
  "hooks": {
    "PreToolUse": [
      { "matcher": "^Artifact", "hooks": [{ "type": "command", "command": "remuda hook artifact" }] }
    ],
    "PostToolUse": [
      { "matcher": "^Artifact", "hooks": [{ "type": "command", "command": "remuda hook artifact" }] }
    ],
    "PostToolUseFailure": [
      { "matcher": "^Artifact", "hooks": [{ "type": "command", "command": "remuda hook artifact" }] }
    ]
  }
}
```

Artifacts published before the hook was installed are learnt the first time
the account that owns them lists them (`Artifact` with `action: "list"`).

## Commands

- `remuda` or `remuda ls [--json]`: the stored logins and which one each CLI
  uses.
- `remuda use [cli/]name`: switch a CLI to a stored login. It moves every open
  session of that CLI.
- `remuda signout cli`: take the login out of a CLI's own files and keep it
  stored. Every open session of that CLI but a `remuda run` one is left signed
  out until a `remuda use`, or until auto mode, when it is on, puts a login
  back.
- `remuda auto`: which login auto mode wants each CLI on, and why.
- `remuda run cli/name -- args`: start one session on a login without switching
  anyone else; `remuda pin name` makes a directory start on it.
- `remuda log`: what remuda did, switches and who made them included.
- `remuda doctor`: what remuda relies on, this hook included.

Never print, copy or pass on tokens from remuda's store or the CLIs'
credential files.
