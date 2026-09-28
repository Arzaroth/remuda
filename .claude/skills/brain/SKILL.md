---
name: brain
description: Use and maintain the brain/ knowledge base - the committed docs describing how remuda works (architecture, features, decisions, glossary). Invoke when the user asks how/where something works ("how does switching work", "where are credentials stored", "why doesn't it refresh the active login"), when onboarding, or when asked to update, audit, or fix the brain after a change. Read brain/BRAIN.md first; navigate via the indexes, not a whole-tree grep.
---

# The brain

`brain/` is the committed knowledge base: how remuda works, written so you can
answer without reading the source. Use it as the first stop for understanding,
and keep it true to the code.

## Navigating (to answer a question)

1. Open `brain/BRAIN.md` - the entry index. It has a topic table and a **Find by
   question** table.
2. Drill via the sub-indexes: `brain/architecture/index.md` (the how) and
   `brain/features/index.md` (the what). Don't grep the whole tree; the indexes
   are the routing layer.
3. Land on the leaf doc. It is self-contained and ends with a `## Sources` list
   of the real code paths - follow those only if the doc isn't enough or you
   suspect drift.
4. Terms -> `brain/glossary.md`. Rationale / "why" -> `brain/decisions.md`.

Structure:

```
brain/BRAIN.md                 root index + find-by-question
brain/glossary.md  decisions.md
brain/architecture/index.md    overview store providers web distribution testing
brain/features/index.md        one doc per user-facing feature
```

## Maintaining (after a change)

The rule is in `CLAUDE.md` ("The brain"). The code is the source of truth - if a
doc disagrees with reality, fix the doc.

- A change that makes a brain doc wrong fixes that doc on the **same branch**.
- New command or user-facing behaviour -> add or update `brain/features/<name>.md`,
  then its row in `brain/features/index.md` and the Features table in
  `brain/BRAIN.md` (and a Find-by-question row if it answers a new question).
- New provider -> `brain/architecture/providers.md` (files, identity, expiry,
  plan, the OAuth constants and which CLI version they were read from).
- Store layout or sidecar keys changed -> `brain/architecture/store.md` **and**
  TokenGauge's `docs/adr/0003-credentials-under-a-provider.md`: it is a
  contract, and TokenGauge's reader breaks silently otherwise.
- New route or guard in `serve` -> `brain/architecture/web.md`.
- Release, installer, timer or CI changed -> `brain/architecture/distribution.md`.
- New non-obvious decision -> append it to `brain/decisions.md` with its "why".
  New term -> `brain/glossary.md`.
- Style: dense, skimmable, present tense, normal prose. No em-dashes.
  Cross-link siblings with relative markdown links. Cite source paths instead
  of restating code. End each doc with `## Sources`.

Before committing, check every relative link in the touched docs resolves:

```bash
python3 - <<'EOF'
import glob, os, re
for f in glob.glob("brain/**/*.md", recursive=True):
    for m in re.finditer(r"\]\(([^)#]+)", open(f).read()):
        t = os.path.normpath(os.path.join(os.path.dirname(f), m.group(1)))
        if not m.group(1).startswith("http") and not os.path.exists(t):
            print("broken:", f, m.group(1))
EOF
```

## Auditing (drift check on request)

When asked to verify the brain matches reality:

1. Scope it to the relevant leaf doc(s) - don't re-audit everything.
2. For each, read the `## Sources` paths and confirm the doc's claims still hold
   (file layout, sidecar keys, constants, command flags and defaults, API
   routes). The quickest cross-checks: `remuda --help` and each subcommand's
   `--help`, `grep -n 'const ' src/*.rs`, and the `App::api` match in
   `src/serve.rs`.
3. Where they diverge, the code wins: correct the doc. Note what you changed.
4. If a feature shipped or was removed, update the indexes (`BRAIN.md`, the two
   `index.md`) too so navigation stays accurate.

For a broad audit, fan out read-only agents (one per architecture or feature
doc) that each diff their doc against its `## Sources`, then apply the fixes.
