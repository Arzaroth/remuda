# remuda brain

How remuda works, written so a developer or an agent can answer questions
without reading the source. Navigate through the indexes; every leaf doc ends
with the source paths it describes. The code is the source of truth: if a doc
disagrees with it, fix the doc.

remuda keeps several logins per coding CLI (Claude Code, Codex) and switches the
one each CLI uses, by rewriting only the CLI's own credential files. It never
changes anything else the CLI keeps, so history, settings and MCP logins stay
shared.

## Topics

| Doc | What it covers |
| --- | --- |
| [architecture/index.md](architecture/index.md) | How it is built: modules, the store, providers, the page, distribution, tests |
| [features/index.md](features/index.md) | What it does, one doc per user-facing feature |
| [glossary.md](glossary.md) | The terms the code and the docs use |
| [decisions.md](decisions.md) | Non-obvious choices and why they were made |

## Find by question

| Question | Go to |
| --- | --- |
| What happens on `remuda use`? | [features/switching.md](features/switching.md) |
| How does a new account get in? | [features/sign-in.md](features/sign-in.md) |
| Why would a stored login stop working? | [features/refresh.md](features/refresh.md), [decisions.md](decisions.md) |
| Where are credentials kept, and in what shape? | [architecture/store.md](architecture/store.md) |
| What does TokenGauge rely on? | [architecture/store.md](architecture/store.md) |
| How is Claude Code different from Codex? | [architecture/providers.md](architecture/providers.md) |
| How do I add a third CLI? | [architecture/providers.md](architecture/providers.md) |
| Which OAuth endpoints and client ids, and where did they come from? | [architecture/providers.md](architecture/providers.md) |
| Is the local page safe to leave running? | [architecture/web.md](architecture/web.md) |
| How is a release built, installed and updated? | [architecture/distribution.md](architecture/distribution.md) |
| How do tests avoid real logins and the network? | [architecture/testing.md](architecture/testing.md) |
| What does `claude/work` mean vs `work`? | [features/names-and-labels.md](features/names-and-labels.md) |

## Features

| Feature | Doc |
| --- | --- |
| Import, list and remove | [features/store-commands.md](features/store-commands.md) |
| Switching | [features/switching.md](features/switching.md) |
| Sign-in | [features/sign-in.md](features/sign-in.md) |
| Token refresh | [features/refresh.md](features/refresh.md) |
| Names and labels | [features/names-and-labels.md](features/names-and-labels.md) |
| The local page | [features/web-page.md](features/web-page.md) |
| Completions and self-update | [features/completions-and-update.md](features/completions-and-update.md) |
