# Names and labels

A credential is `provider/name`. Commands that take an existing credential
accept the bare name when exactly one provider has it, and ask for the
qualified form when several do (`commands::resolve`). Commands that create one
take `-p <provider>`, defaulting to `claude`.

`remuda label <name> [text]` sets a free-text label, shown as `work (Job)` in
`ls`, in the page, and in `ls --json` as `label`; TokenGauge reads it from the
sidecar. No text, or only whitespace, clears it.

`remuda rename <name> <new>` moves the credential and its sidecar, refusing a
name that is taken or invalid. Renaming the active credential is fine: the
match is by token and account, not by name.

## Sources

- [src/commands.rs](../../src/commands.rs) `resolve`, `label`, `rename`
- [src/store.rs](../../src/store.rs) `rename`, `validate_name`
