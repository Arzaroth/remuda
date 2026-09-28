# Glossary

**Provider**: one coding CLI remuda keeps logins for (`claude`, `codex`). The
`Provider` trait is everything remuda needs to know about it. Its id is the
directory name in the store and the prefix in `provider/name`.

**Credential**: one stored login: a credentials file and its sidecar, under a
name. Called an *entry* in the code (`store::Entry`).

**Live login**: what the CLI is signed into right now, read from the CLI's own
files (`Provider::live`). It is not stored until someone imports it.

**Active credential**: the stored credential the live login belongs to. `ls`
marks it with `*`; the page marks it "active".

**Unstored**: a live login no stored credential matches. Switching away from it
would lose it, so `use` refuses unless given `--discard`.

**Sync back**: copying the live login's current tokens into the credential it
belongs to (`ops::sync_live`). Every command does it first.

**Confirm**: asking which account a set of tokens really belongs to
(`Provider::confirm`), used before trusting a match made by account id alone.

**Sidecar**: `<name>.meta.json` beside a credential: account id, email, capture
time, label, and for Claude the `oauthAccount` block.

**Account id**: the stable identity of a login. Claude's `accountUuid`, Codex's
ChatGPT `account_id`. Tokens rotate; this does not.

**Pending login**: a sign-in the browser has been sent to and that has not
finished (`provider::PendingLogin`). Claude's finishes with a pasted code,
Codex's with a callback.

**Store**: the directory of credentials, `$REMUDA_STORE` or
`$XDG_DATA_HOME/remuda/credentials`.

**Qualified name**: `provider/name`, e.g. `codex/work`.

## Sources

- [src/provider.rs](../src/provider.rs)
- [src/store.rs](../src/store.rs)
- [src/ops.rs](../src/ops.rs)
