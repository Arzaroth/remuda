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
| How do I leave a CLI with no login, so remuda keeps them all? | [features/switching.md](features/switching.md#signing-out) |
| How does a new account get in? | [features/sign-in.md](features/sign-in.md) |
| Which browser and window does a sign-in open in? | [features/sign-in.md](features/sign-in.md#where-it-opens) |
| How do I give a new machine the same logins, and why are some signed in again? | [features/another-machine.md](features/another-machine.md) |
| Why would a stored login stop working? | [features/refresh.md](features/refresh.md), [decisions.md](decisions.md) |
| Where are credentials kept, and in what shape? | [architecture/store.md](architecture/store.md) |
| Where are remuda's settings, and how does the page edit them? | [architecture/config.md](architecture/config.md) |
| Why did auto mode switch, or not? | [features/auto-mode.md](features/auto-mode.md) |
| Why was a login pinged, or not? | [features/keep-warm.md](features/keep-warm.md) |
| How long will a CLI's logins last at this pace? | [features/web-page.md](features/web-page.md#quota) |
| Who switched this CLI, when, and why? | [features/log.md](features/log.md) |
| Which login spent a request, for TokenGauge? | [features/switching.md](features/switching.md#whose-spend-it-is) |
| Why is the Status dot amber, and what does `remuda doctor` check? | [features/doctor.md](features/doctor.md) |
| How do I start a CLI on a login without switching, or pin one per repo? | [features/run.md](features/run.md) |
| What does TokenGauge rely on? | [architecture/store.md](architecture/store.md) |
| How is Claude Code different from Codex? | [architecture/providers.md](architecture/providers.md) |
| How do I add another CLI, or another API key provider? | [architecture/providers.md](architecture/providers.md) |
| Which OAuth endpoints and client ids, and where did they come from? | [architecture/providers.md](architecture/providers.md) |
| Is the local page safe to leave running? | [architecture/web.md](architecture/web.md) |
| How does `remuda open` find the page, and what runs it as a service? | [architecture/web.md](architecture/web.md) |
| How do I point an OpenAI or Anthropic client at my logins? | [features/proxy.md](features/proxy.md) |
| Which login does a proxied request spend, and what is it sent with? | [architecture/proxy.md](architecture/proxy.md) |
| Which remuda and TokenGauge run here, and how does the page update them? | [features/web-page.md](features/web-page.md), [architecture/web.md](architecture/web.md#restarting-on-an-update) |
| What differs on macOS, and how do I set a Mac up from Linux? | [architecture/macos.md](architecture/macos.md), [features/another-machine.md](features/another-machine.md) |
| How is a release built, installed and updated? | [architecture/distribution.md](architecture/distribution.md) |
| How do tests avoid real logins and the network? | [architecture/testing.md](architecture/testing.md) |
| What does `claude/work` mean vs `work`? | [features/names-and-labels.md](features/names-and-labels.md) |

## Features

| Feature | Doc |
| --- | --- |
| Import, list and remove | [features/store-commands.md](features/store-commands.md) |
| Switching | [features/switching.md](features/switching.md) |
| Sign-in | [features/sign-in.md](features/sign-in.md) |
| Setting up another machine | [features/another-machine.md](features/another-machine.md) |
| Token refresh | [features/refresh.md](features/refresh.md) |
| Names and labels | [features/names-and-labels.md](features/names-and-labels.md) |
| Auto mode | [features/auto-mode.md](features/auto-mode.md) |
| Keep-warm | [features/keep-warm.md](features/keep-warm.md) |
| The local page | [features/web-page.md](features/web-page.md) |
| The proxy | [features/proxy.md](features/proxy.md) |
| The log | [features/log.md](features/log.md) |
| The doctor | [features/doctor.md](features/doctor.md) |
| Running without switching, and pins | [features/run.md](features/run.md) |
| Completions and self-update | [features/completions-and-update.md](features/completions-and-update.md) |
