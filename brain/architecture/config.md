# The config file

## Location

`$REMUDA_CONFIG`, else `$XDG_CONFIG_HOME/remuda/config.toml`, else
`~/.config/remuda/config.toml` ([paths.rs](../../src/paths.rs) `config_file`).
A missing file is an empty config.

## What it holds

Auto mode's settings ([features/auto-mode.md](../features/auto-mode.md)): named
schedules, shared by every provider, and one `[auto.<provider>]` section per
provider.

```toml
[schedules.workday]
days  = ["mon", "tue", "wed", "thu", "fri"]   # none: every day
hours = "09:00-18:00"                         # local; none: all day; may wrap midnight

[auto.claude]
enabled = true      # default false
dwell   = "30m"     # default; h, m, s; 1 minute to 7 days
margin  = 0.25      # default
logins  = ["work", "perso"]   # the one tier used when no rule is in force
ceiling = 97        # a window is full at this %; default 100
ceilings = { work = 93 }      # per login, for the shortest of several windows
fallback = "perso"  # where to go when every login, the active one too, is full

[[auto.claude.rule]]
when    = "workday"           # a schedule, or "otherwise"
order   = [["work"], ["perso"]]
reserve = { work = "workday" }
```

Keep-warm's logins ([features/keep-warm.md](../features/keep-warm.md)) are one
list per provider, apart from auto mode:

```toml
[warm]
claude = ["work", "perso"]
codex  = ["work"]
```

Unknown keys are refused. A key at its default is left out when remuda writes
it anew, and kept when the file already sets it.

## Checks

`Config::check` lists what auto mode cannot act on, each with where it is
(`auto.claude.rule[2].when`): a provider remuda does not have, a login not
stored, a schedule that does not exist, `otherwise` used as a schedule name, a
day named twice, a login twice in one rule, an empty tier, a reservation for a
login outside its rule's order, a rule after an `otherwise` one (never in
force), a rule with no tier at all, a ceiling outside 1 to 100, a ceiling or a
fallback naming a login not stored, a dwell under a minute or over seven days,
a negative margin; and in `[warm]`, a provider remuda does not have, a login not
stored, or one named twice. Renaming or removing a login keeps `[warm]` in step. The page cannot save a config with any of them; a
file edited by hand is still read, and the page shows them.

## Writes

A write parses the file as it is (`toml_edit`) and changes only the values
that differ from the new config, so comments and layout around the rest stay,
and a write that changes nothing leaves the bytes alone. When one table goes and
one comes in its place (only one, or the one holding the same values), it is
a rename and keeps the old one's comments and place; a rename made in the
same save as adding a table holding the renamed one's old values pairs with
that one instead. Rules are matched by their values, so a rule removed takes
its comments with it, and a rule moved keeps them; an edited rule pairs with a
leftover one in order. A table the file has where the config writes an inline one
(`[auto.claude.ceilings]`) stays a table. A table added goes after
the last of its siblings already in the file, so a new schedule follows the
others and a new rule the provider's last. `save` takes the
version the caller read, the SHA-256 of the file's bytes, and refuses with
`Stale` if the file has changed since. A `config.toml.lock` beside the file
serialises writers.

`remuda rename` and `remuda rm` (and the page's) keep the names in step: a
renamed login keeps its place in every tier, reservation, ceiling and the
fallback, and a removed one leaves them, taking a tier it empties with it (a rule left with no tier is then
a problem the page shows). They write nothing when there is
no file, and a config that cannot be updated leaves the store change standing,
with a warning.

## Sources

- [src/config.rs](../../src/config.rs)
- [src/paths.rs](../../src/paths.rs) `config_file`
- [src/commands.rs](../../src/commands.rs) `rename`, `remove`
- [src/serve.rs](../../src/serve.rs) `config`, `/api/config`
