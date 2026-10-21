//! `config.toml`: auto mode's settings, which the page edits as well as the
//! user's editor. A write changes only what differs from the file, so the
//! comments and layout around everything else stay.

use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use toml_edit::{ArrayOfTables, DocumentMut, InlineTable, Item, Table, Value};

use crate::fsx;

pub const OTHERWISE: &str = "otherwise";
const MIN_DWELL: u64 = 60;
const MAX_DWELL: u64 = 7 * 24 * 3600;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub schedules: BTreeMap<String, Schedule>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub auto: BTreeMap<String, Auto>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Schedule {
    /// Empty means every day.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub days: Vec<Day>,
    /// None means all day.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hours: Option<Hours>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Day {
    Mon,
    Tue,
    Wed,
    Thu,
    Fri,
    Sat,
    Sun,
}

impl Day {
    fn name(self) -> &'static str {
        match self {
            Day::Mon => "mon",
            Day::Tue => "tue",
            Day::Wed => "wed",
            Day::Thu => "thu",
            Day::Fri => "fri",
            Day::Sat => "sat",
            Day::Sun => "sun",
        }
    }
}

/// Local time, in minutes since midnight. `start > end` runs past midnight.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Hours {
    pub start: u16,
    pub end: u16,
}

impl TryFrom<String> for Hours {
    type Error = String;

    fn try_from(text: String) -> Result<Self, String> {
        let clock = |s: &str| -> Option<u16> {
            let (h, m) = s.trim().split_once(':')?;
            if h.is_empty() || h.len() > 2 || m.len() != 2 {
                return None;
            }
            let (h, m): (u16, u16) = (h.parse().ok()?, m.parse().ok()?);
            (m < 60 && (h < 24 || (h == 24 && m == 0))).then_some(h * 60 + m)
        };
        let bad = || format!("{text:?} is not a range of hours like \"09:00-18:00\"");
        let (start, end) = text.split_once('-').ok_or_else(bad)?;
        let (start, end) = (clock(start).ok_or_else(bad)?, clock(end).ok_or_else(bad)?);
        if start == end || start == 24 * 60 {
            return Err(bad());
        }
        Ok(Hours { start, end })
    }
}

impl From<Hours> for String {
    fn from(h: Hours) -> String {
        let clock = |m: u16| format!("{:02}:{:02}", m / 60, m % 60);
        format!("{}-{}", clock(h.start), clock(h.end))
    }
}

/// A span written like `30m`, `1h`, `1h30m` or `90s`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Span(pub u64);

impl TryFrom<String> for Span {
    type Error = String;

    fn try_from(text: String) -> Result<Self, String> {
        let bad = || format!("{text:?} is not a span like \"30m\" or \"1h30m\"");
        let mut total = 0u64;
        let mut digits = String::new();
        for c in text.trim().chars() {
            if c.is_ascii_digit() {
                digits.push(c);
                continue;
            }
            let unit = match c {
                'h' => 3600,
                'm' => 60,
                's' => 1,
                _ => return Err(bad()),
            };
            let n: u64 = digits.parse().map_err(|_| bad())?;
            total = n
                .checked_mul(unit)
                .and_then(|n| total.checked_add(n))
                .ok_or_else(bad)?;
            digits.clear();
        }
        if !digits.is_empty() || text.trim().is_empty() {
            return Err(bad());
        }
        Ok(Span(total))
    }
}

impl From<Span> for String {
    fn from(s: Span) -> String {
        let (h, m, sec) = (s.0 / 3600, s.0 / 60 % 60, s.0 % 60);
        let mut out = String::new();
        for (n, unit) in [(h, 'h'), (m, 'm'), (sec, 's')] {
            if n > 0 {
                out.push_str(&format!("{n}{unit}"));
            }
        }
        if out.is_empty() { "0s".into() } else { out }
    }
}

fn default_dwell() -> Span {
    Span(30 * 60)
}

fn is_default_dwell(s: &Span) -> bool {
    *s == default_dwell()
}

fn default_margin() -> f64 {
    0.25
}

fn is_default_margin(m: &f64) -> bool {
    *m == default_margin()
}

fn default_ceiling() -> u64 {
    100
}

fn is_default_ceiling(c: &u64) -> bool {
    *c == default_ceiling()
}

fn is_false(b: &bool) -> bool {
    !b
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Auto {
    #[serde(default, skip_serializing_if = "is_false")]
    pub enabled: bool,
    #[serde(default = "default_dwell", skip_serializing_if = "is_default_dwell")]
    pub dwell: Span,
    #[serde(default = "default_margin", skip_serializing_if = "is_default_margin")]
    pub margin: f64,
    /// One tier, used when no rule is in force.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub logins: Vec<String>,
    #[serde(default, rename = "rule", skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<Rule>,
    /// The percentage at which a window counts as full.
    #[serde(
        default = "default_ceiling",
        skip_serializing_if = "is_default_ceiling"
    )]
    pub ceiling: u64,
    /// Login name to the percentage its shortest window counts as full at.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub ceilings: BTreeMap<String, u64>,
    /// The login to go to when every login it may pick is full.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback: Option<String>,
}

impl Default for Auto {
    fn default() -> Self {
        Auto {
            enabled: false,
            dwell: default_dwell(),
            margin: default_margin(),
            logins: Vec::new(),
            rules: Vec::new(),
            ceiling: default_ceiling(),
            ceilings: BTreeMap::new(),
            fallback: None,
        }
    }
}

impl Auto {
    fn forget(&mut self, name: &str) {
        self.logins.retain(|n| n != name);
        for rule in &mut self.rules {
            for tier in &mut rule.order {
                tier.retain(|n| n != name);
            }
            rule.order.retain(|t| !t.is_empty());
            rule.reserve.remove(name);
        }
        self.ceilings.remove(name);
        if self.fallback.as_deref() == Some(name) {
            self.fallback = None;
        }
    }

    fn rename(&mut self, from: &str, to: &str) {
        let swap = |n: &mut String| {
            if n == from {
                *n = to.to_owned();
            }
        };
        self.logins.iter_mut().for_each(swap);
        for rule in &mut self.rules {
            rule.order.iter_mut().flatten().for_each(swap);
            if let Some(schedule) = rule.reserve.remove(from) {
                rule.reserve.insert(to.to_owned(), schedule);
            }
        }
        if let Some(c) = self.ceilings.remove(from) {
            self.ceilings.insert(to.to_owned(), c);
        }
        if let Some(f) = self.fallback.as_mut() {
            swap(f);
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    /// A schedule's name, or `otherwise`.
    pub when: String,
    /// Tiers, first first.
    #[serde(default)]
    pub order: Vec<Vec<String>>,
    /// Login name to the schedule it is kept for.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub reserve: BTreeMap<String, String>,
}

/// Something in the config auto mode cannot act on, and where it is.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Problem {
    pub at: String,
    pub message: String,
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.at, self.message)
    }
}

impl Config {
    /// `logins(provider)` names the provider's stored logins, or is `None`
    /// for a provider remuda does not know.
    pub fn check(&self, logins: &dyn Fn(&str) -> Option<Vec<String>>) -> Vec<Problem> {
        let mut problems = Vec::new();
        let mut flag = |at: &str, message: String| {
            problems.push(Problem {
                at: at.to_owned(),
                message,
            })
        };

        for (name, schedule) in &self.schedules {
            let at = format!("schedules.{name}");
            if name == OTHERWISE {
                flag(&at, format!("{OTHERWISE:?} is kept for the catch-all rule"));
            }
            let mut days = schedule.days.clone();
            days.sort();
            days.dedup();
            if days.len() != schedule.days.len() {
                flag(&at, "a day is named twice".into());
            }
        }
        let is_schedule = |s: &str| s != OTHERWISE && self.schedules.contains_key(s);

        for (provider, auto) in &self.auto {
            let at = format!("auto.{provider}");
            let Some(stored) = logins(provider) else {
                flag(&at, format!("remuda has no provider {provider:?}"));
                continue;
            };
            let unknown = |name: &str| {
                (!stored.iter().any(|s| s == name))
                    .then(|| format!("no stored login {provider}/{name}"))
            };
            for name in &auto.logins {
                if let Some(m) = unknown(name) {
                    flag(&format!("{at}.logins"), m);
                }
            }
            let percent = |c: u64| (1..=100).contains(&c);
            if !percent(auto.ceiling) {
                flag(
                    &format!("{at}.ceiling"),
                    "a ceiling is a percentage, 1 to 100".into(),
                );
            }
            for (name, c) in &auto.ceilings {
                let cat = format!("{at}.ceilings.{name}");
                if let Some(m) = unknown(name) {
                    flag(&cat, m);
                }
                if !percent(*c) {
                    flag(&cat, "a ceiling is a percentage, 1 to 100".into());
                }
            }
            if let Some(m) = auto.fallback.as_deref().and_then(unknown) {
                flag(&format!("{at}.fallback"), m);
            }
            let mut otherwise_at = None;
            for (i, rule) in auto.rules.iter().enumerate() {
                let rat = format!("{at}.rule[{}]", i + 1);
                let mut seen: Vec<&str> = Vec::new();
                for name in rule.order.iter().flatten() {
                    if let Some(m) = unknown(name) {
                        flag(&format!("{rat}.order"), m);
                    }
                    if seen.contains(&name.as_str()) {
                        flag(&format!("{rat}.order"), format!("{name} is named twice"));
                    }
                    seen.push(name);
                }
                if rule.order.is_empty() {
                    flag(&format!("{rat}.order"), "the rule names no login".into());
                }
                if rule.order.iter().any(Vec::is_empty) {
                    flag(&format!("{rat}.order"), "a tier is empty".into());
                }
                if rule.when != OTHERWISE && !is_schedule(&rule.when) {
                    flag(
                        &format!("{rat}.when"),
                        format!("no schedule named {:?}", rule.when),
                    );
                }
                if let Some(first) = otherwise_at {
                    flag(
                        &rat,
                        format!("never in force: rule {first} matches any time"),
                    );
                } else if rule.when == OTHERWISE {
                    otherwise_at = Some(i + 1);
                }
                for (name, schedule) in &rule.reserve {
                    let at = format!("{rat}.reserve.{name}");
                    if !seen.contains(&name.as_str()) {
                        flag(&at, format!("{name} is not in this rule's order"));
                    }
                    if !is_schedule(schedule) {
                        flag(&at, format!("no schedule named {schedule:?}"));
                    }
                }
            }
            if !(MIN_DWELL..=MAX_DWELL).contains(&auto.dwell.0) {
                flag(
                    &format!("{at}.dwell"),
                    "the least time on a login is between 1 minute and 7 days".into(),
                );
            }
            if !auto.margin.is_finite() || auto.margin < 0.0 {
                flag(
                    &format!("{at}.margin"),
                    "the margin is a fraction, 0 or more".into(),
                );
            }
        }
        problems
    }

    fn document(&self) -> DocumentMut {
        let mut doc = DocumentMut::new();
        if !self.schedules.is_empty() {
            let mut all = Table::new();
            all.set_implicit(true);
            for (name, s) in &self.schedules {
                let mut t = Table::new();
                if !s.days.is_empty() {
                    t["days"] =
                        toml_edit::value(Value::Array(s.days.iter().map(|d| d.name()).collect()));
                }
                if let Some(h) = s.hours {
                    t["hours"] = toml_edit::value(String::from(h));
                }
                all[name.as_str()] = Item::Table(t);
            }
            doc["schedules"] = Item::Table(all);
        }
        if !self.auto.is_empty() {
            let mut all = Table::new();
            all.set_implicit(true);
            for (provider, a) in &self.auto {
                let mut t = Table::new();
                if a.enabled {
                    t["enabled"] = toml_edit::value(true);
                }
                if !is_default_dwell(&a.dwell) {
                    t["dwell"] = toml_edit::value(String::from(a.dwell));
                }
                if !is_default_margin(&a.margin) {
                    t["margin"] = toml_edit::value(a.margin);
                }
                if !a.logins.is_empty() {
                    t["logins"] = toml_edit::value(Value::Array(
                        a.logins.iter().map(String::as_str).collect(),
                    ));
                }
                if !is_default_ceiling(&a.ceiling) {
                    t["ceiling"] = toml_edit::value(a.ceiling as i64);
                }
                if !a.ceilings.is_empty() {
                    let mut ceilings = InlineTable::new();
                    for (login, c) in &a.ceilings {
                        ceilings.insert(login, (*c as i64).into());
                    }
                    t["ceilings"] = toml_edit::value(ceilings);
                }
                if let Some(f) = &a.fallback {
                    t["fallback"] = toml_edit::value(f.as_str());
                }
                if !a.rules.is_empty() {
                    let mut rules = ArrayOfTables::new();
                    for r in &a.rules {
                        let mut rt = Table::new();
                        rt["when"] = toml_edit::value(r.when.as_str());
                        rt["order"] = toml_edit::value(Value::Array(
                            r.order
                                .iter()
                                .map(|tier| Value::Array(tier.iter().map(String::as_str).collect()))
                                .collect(),
                        ));
                        if !r.reserve.is_empty() {
                            let mut reserve = InlineTable::new();
                            for (login, schedule) in &r.reserve {
                                reserve.insert(login, schedule.as_str().into());
                            }
                            rt["reserve"] = toml_edit::value(reserve);
                        }
                        rules.push(rt);
                    }
                    t["rule"] = Item::ArrayOfTables(rules);
                }
                all[provider.as_str()] = Item::Table(t);
            }
            doc["auto"] = Item::Table(all);
        }
        doc
    }
}

/// The config as read, and the digest of the bytes it was read from, which a
/// write must name to replace them.
#[derive(Debug, Clone, PartialEq)]
pub struct Loaded {
    pub config: Config,
    pub version: String,
}

fn version(text: &str) -> String {
    crate::store::digest(text)
}

fn read(path: &Path) -> Result<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("failed to read {}", path.display())),
    }
}

fn parse(path: &Path, text: &str) -> Result<Config> {
    toml_edit::de::from_str(text).with_context(|| format!("{} is not valid", path.display()))
}

/// A missing file is an empty config.
pub fn load(path: &Path) -> Result<Loaded> {
    let text = read(path)?.unwrap_or_default();
    Ok(Loaded {
        config: parse(path, &text)?,
        version: version(&text),
    })
}

/// The file changed after the version a write was asked against.
#[derive(Debug)]
pub struct Stale;

impl fmt::Display for Stale {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("the config changed since it was read; load it again")
    }
}

impl std::error::Error for Stale {}

fn lock(path: &Path) -> Result<std::fs::File> {
    let mut name = path
        .file_name()
        .context("path has no file name")?
        .to_owned();
    name.push(".lock");
    fsx::lock_file(&path.with_file_name(name))
}

/// Writes `config` over the file read as `version`, keeping the comments and
/// layout of whatever did not change. Returns the new version.
pub fn save(path: &Path, config: &Config, expected: &str) -> Result<String> {
    let _lock = lock(path)?;
    let text = read(path)?.unwrap_or_default();
    if version(&text) != expected {
        return Err(Stale.into());
    }
    write(path, &text, config)
}

fn write(path: &Path, text: &str, config: &Config) -> Result<String> {
    let mut doc: DocumentMut = text
        .parse()
        .with_context(|| format!("{} is not valid", path.display()))?;
    let mut new = config.document();
    keep_defaults(&doc, &mut new);
    merge_table(doc.as_table_mut(), new.as_table());
    place_added(&mut doc);
    let body = doc.to_string();
    if body != text {
        fsx::write_private(path, &body)?;
    }
    Ok(version(&body))
}

/// Applies `edit` to the config as it is now; writes nothing when there is
/// no file or `edit` changes nothing.
fn update(path: &Path, edit: impl FnOnce(&mut Config)) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let _lock = lock(path)?;
    let Some(text) = read(path)? else {
        return Ok(());
    };
    let mut config = parse(path, &text)?;
    let before = config.clone();
    edit(&mut config);
    if config != before {
        write(path, &text, &config)?;
    }
    Ok(())
}

pub fn forget_login(path: &Path, provider: &str, name: &str) -> Result<()> {
    update(path, |c| {
        if let Some(auto) = c.auto.get_mut(provider) {
            auto.forget(name);
        }
    })
}

pub fn rename_login(path: &Path, provider: &str, from: &str, to: &str) -> Result<()> {
    update(path, |c| {
        if let Some(auto) = c.auto.get_mut(provider) {
            auto.rename(from, to);
        }
    })
}

fn bare(v: &Value) -> String {
    let mut v = v.clone();
    match &mut v {
        Value::Array(a) => a.fmt(),
        Value::InlineTable(t) => t.fmt(),
        _ => {}
    }
    v.decor_mut().clear();
    v.to_string()
}

/// One table gone and one come in its place is a rename: it keeps the
/// comments and the place in the file the old one had.
fn merge_table(old: &mut Table, new: &Table) {
    let gone: Vec<String> = old
        .iter()
        .map(|(k, _)| k.to_owned())
        .filter(|k| !new.contains_key(k))
        .collect();
    let come: Vec<&str> = new
        .iter()
        .map(|(k, _)| k)
        .filter(|k| !old.contains_key(k))
        .collect();
    let mut renamed = None;
    if let [from] = gone.as_slice()
        && old[from.as_str()].is_table()
    {
        let tables: Vec<&str> = come
            .iter()
            .copied()
            .filter(|k| new[*k].is_table())
            .collect();
        let was = plain(&old[from.as_str()]);
        let same: Vec<&str> = tables
            .iter()
            .copied()
            .filter(|k| plain(&new[*k]) == was)
            .collect();
        renamed = match (same.as_slice(), tables.as_slice()) {
            ([to], _) | ([], [to]) => Some((*to, old.remove(from))),
            _ => None,
        };
    }
    for k in &gone {
        old.remove(k);
    }
    for (k, item) in new.iter() {
        match old.get_mut(k) {
            Some(existing) => merge_item(existing, item),
            None => {
                let item = match &renamed {
                    Some((to, Some(moved))) if *to == k => {
                        let mut moved = moved.clone();
                        merge_item(&mut moved, item);
                        moved
                    }
                    _ => item.clone(),
                };
                old.insert(k, item);
            }
        }
    }
}

/// A table the merge added has no place in the file yet: it goes after the
/// last of its siblings already there (the last schedule, the last rule),
/// with a blank line.
fn place_added(doc: &mut DocumentMut) {
    let mut keys: Vec<(f64, bool)> = Vec::new();
    sort_keys(doc.as_table(), -1.0, &mut keys);
    let mut order: Vec<usize> = (0..keys.len()).collect();
    order.sort_by(|a, b| keys[*a].0.total_cmp(&keys[*b].0));
    let mut rank = vec![0; keys.len()];
    for (r, i) in order.into_iter().enumerate() {
        rank[i] = r as isize;
    }
    let mut i = 0;
    each_table(doc.as_table_mut(), &mut |t| {
        if t.is_implicit() {
            return;
        }
        if keys[i].1 && rank[i] > 0 && t.decor().prefix().is_none() {
            t.decor_mut().set_prefix("\n");
        }
        t.set_position(Some(rank[i]));
        i += 1;
    });
}

fn children(table: &Table) -> Vec<&Table> {
    table
        .iter()
        .flat_map(|(_, item)| match item {
            Item::Table(t) => vec![t],
            Item::ArrayOfTables(a) => a.iter().collect(),
            _ => vec![],
        })
        .collect()
}

fn last_placed(table: &Table) -> Option<isize> {
    let below = children(table).into_iter().filter_map(last_placed).max();
    table.position().max(below)
}

/// Where each explicit table sorts, in [`each_table`]'s order, and whether
/// it is new; returns the last key it gave.
fn sort_keys(table: &Table, mut cur: f64, keys: &mut Vec<(f64, bool)>) -> f64 {
    let kids = children(table);
    let siblings = kids.iter().filter_map(|t| last_placed(t)).max();
    for t in kids {
        if !t.is_implicit() {
            let key = match t.position() {
                Some(p) => p as f64,
                None => cur.max(siblings.map_or(cur, |p| p as f64)) + 1e-6,
            };
            keys.push((key, t.position().is_none()));
            cur = key;
        }
        cur = sort_keys(t, cur, keys);
    }
    cur
}

/// Tables in the order the config nests them.
fn each_table(table: &mut Table, f: &mut impl FnMut(&mut Table)) {
    for (_, item) in table.iter_mut() {
        match item {
            Item::Table(t) => {
                f(t);
                each_table(t, f);
            }
            Item::ArrayOfTables(a) => {
                for t in a.iter_mut() {
                    f(t);
                    each_table(t, f);
                }
            }
            _ => {}
        }
    }
}

fn plain(item: &Item) -> Option<serde_json::Value> {
    let mut doc = DocumentMut::new();
    doc["x"] = item.clone();
    toml_edit::de::from_document(doc).ok()
}

/// A key the file sets to its default stays, rather than going because the
/// config leaves defaults out.
fn keep_defaults(old: &DocumentMut, new: &mut DocumentMut) {
    let Some(auto) = new.get_mut("auto").and_then(Item::as_table_mut) else {
        return;
    };
    for (id, section) in auto.iter_mut() {
        let Some(section) = section.as_table_mut() else {
            continue;
        };
        let had = |k: &str| {
            old.get("auto")
                .and_then(|a| a.get(id.get()))
                .and_then(|p| p.get(k))
                .is_some()
        };
        let defaults = [
            ("enabled", toml_edit::value(false)),
            ("dwell", toml_edit::value(String::from(default_dwell()))),
            ("margin", toml_edit::value(default_margin())),
            ("ceiling", toml_edit::value(default_ceiling() as i64)),
        ];
        for (k, v) in defaults {
            if !section.contains_key(k) && had(k) {
                section.insert(k, v);
            }
        }
    }
}

/// Each new table takes the old one holding the same values, so a rule
/// removed or moved keeps its comments; the rest pair up in order.
fn merge_rules(old: &mut ArrayOfTables, new: &ArrayOfTables) {
    let olds: Vec<Table> = old.iter().cloned().collect();
    let plain_of = |t: &Table| plain(&Item::Table(t.clone()));
    let mut used = vec![false; olds.len()];
    let mut pick: Vec<Option<usize>> = new
        .iter()
        .map(|t| {
            let want = plain_of(t);
            let j = (0..olds.len()).find(|j| !used[*j] && plain_of(&olds[*j]) == want);
            if let Some(j) = j {
                used[j] = true;
            }
            j
        })
        .collect();
    let mut rest = (0..olds.len())
        .filter(|j| !used[*j])
        .collect::<Vec<_>>()
        .into_iter();
    for p in pick.iter_mut().filter(|p| p.is_none()) {
        *p = rest.next();
    }
    let mut places: Vec<isize> = pick
        .iter()
        .flatten()
        .filter_map(|j| olds[*j].position())
        .collect();
    places.sort();
    let mut places = places.into_iter();
    let mut out = ArrayOfTables::new();
    for (t, j) in new.iter().zip(pick) {
        out.push(match j {
            Some(j) => {
                let mut kept = olds[j].clone();
                merge_table(&mut kept, t);
                kept.set_position(places.next());
                kept
            }
            None => t.clone(),
        });
    }
    *old = out;
}

fn merge_item(old: &mut Item, new: &Item) {
    match (&mut *old, new) {
        (Item::Table(o), Item::Table(n)) => merge_table(o, n),
        (Item::ArrayOfTables(o), Item::ArrayOfTables(n)) => merge_rules(o, n),
        (Item::Table(o), Item::Value(Value::InlineTable(n))) => {
            merge_table(o, &n.clone().into_table());
        }
        (Item::Value(o), Item::Value(n)) => {
            if bare(o) != bare(n) {
                let decor = o.decor().clone();
                *o = n.clone();
                *o.decor_mut() = decor;
            }
        }
        (o, n) => *o = n.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"# auto mode
[schedules.workday]
days  = ["mon", "tue", "wed", "thu", "fri"]
hours = "09:00-18:00"   # local time

[auto.claude]
enabled = true

[[auto.claude.rule]]
when  = "workday"
order = [["work"], ["perso"]]

[[auto.claude.rule]]
when    = "otherwise"
order   = [["perso"], ["work"]]
reserve = { work = "workday" }
"#;

    fn stored(provider: &str) -> Option<Vec<String>> {
        (provider == "claude").then(|| vec!["work".into(), "perso".into()])
    }

    fn sample() -> Config {
        toml_edit::de::from_str(SAMPLE).unwrap()
    }

    fn written(text: &str, config: &Config) -> String {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.toml");
        std::fs::write(&path, text).unwrap();
        save(&path, config, &version(text)).unwrap();
        std::fs::read_to_string(&path).unwrap()
    }

    #[test]
    fn the_roadmap_example_reads_and_checks_clean() {
        let c = sample();
        let day = &c.schedules["workday"];
        assert_eq!(day.days.len(), 5);
        assert_eq!(
            day.hours,
            Some(Hours {
                start: 540,
                end: 1080
            })
        );
        let auto = &c.auto["claude"];
        assert!(auto.enabled);
        assert_eq!(auto.dwell, Span(1800));
        assert_eq!(auto.margin, 0.25);
        assert_eq!(auto.rules[1].reserve["work"], "workday");
        assert_eq!(c.check(&stored), []);
    }

    #[test]
    fn hours_and_spans_read_the_way_they_are_written() {
        let hours = |s: &str| Hours::try_from(s.to_owned());
        assert_eq!(
            hours("22:00-06:30"),
            Ok(Hours {
                start: 1320,
                end: 390
            })
        );
        assert_eq!(
            hours("9:00-24:00"),
            Ok(Hours {
                start: 540,
                end: 1440
            })
        );
        for bad in [
            "09:00",
            "09:00-09:00",
            "24:00-01:00",
            "25:00-01:00",
            "9-17",
            "09:60-10:00",
        ] {
            assert!(hours(bad).is_err(), "{bad}");
        }
        assert_eq!(
            String::from(Hours {
                start: 1320,
                end: 390
            }),
            "22:00-06:30"
        );

        let span = |s: &str| Span::try_from(s.to_owned());
        assert_eq!(span("1h30m"), Ok(Span(5400)));
        assert_eq!(span("90s"), Ok(Span(90)));
        for bad in ["", "30", "m", "1d", "1h 30m"] {
            assert!(span(bad).is_err(), "{bad}");
        }
        assert_eq!(String::from(Span(5400)), "1h30m");
        assert_eq!(String::from(Span(0)), "0s");
    }

    #[test]
    fn unknown_keys_are_refused() {
        assert!(toml_edit::de::from_str::<Config>("[auto.claude]\nenable = true").is_err());
    }

    #[test]
    fn check_names_what_auto_mode_cannot_act_on() {
        let c: Config = toml_edit::de::from_str(
            r#"
[schedules.otherwise]
[schedules.week]
days = ["mon", "mon"]

[auto.nope]

[auto.claude]
margin = -1.0
logins = ["ghost"]

[[auto.claude.rule]]
when  = "nights"
order = [["work", "work"], []]
reserve = { perso = "week", work = "weekend" }

[[auto.claude.rule]]
when  = "otherwise"
order = [["perso"]]

[[auto.claude.rule]]
when  = "week"
order = [["perso"]]
"#,
        )
        .unwrap();
        let problems: Vec<String> = c.check(&stored).iter().map(ToString::to_string).collect();
        assert_eq!(
            problems,
            [
                "schedules.otherwise: \"otherwise\" is kept for the catch-all rule",
                "schedules.week: a day is named twice",
                "auto.claude.logins: no stored login claude/ghost",
                "auto.claude.rule[1].order: work is named twice",
                "auto.claude.rule[1].order: a tier is empty",
                "auto.claude.rule[1].when: no schedule named \"nights\"",
                "auto.claude.rule[1].reserve.perso: perso is not in this rule's order",
                "auto.claude.rule[1].reserve.work: no schedule named \"weekend\"",
                "auto.claude.rule[3]: never in force: rule 2 matches any time",
                "auto.claude.margin: the margin is a fraction, 0 or more",
                "auto.nope: remuda has no provider \"nope\"",
            ]
        );
    }

    #[test]
    fn a_write_keeps_the_comments_around_what_it_did_not_change() {
        let mut c = sample();
        c.auto.get_mut("claude").unwrap().enabled = false;
        c.schedules.get_mut("workday").unwrap().hours = Some(Hours {
            start: 480,
            end: 1020,
        });

        let text = written(SAMPLE, &c);

        assert!(text.starts_with("# auto mode\n[schedules.workday]\n"));
        assert!(text.contains("hours = \"08:00-17:00\"   # local time\n"));
        assert!(
            text.contains("enabled = false\n"),
            "a key set to its default stays"
        );
        assert!(text.contains("reserve = { work = \"workday\" }"));
        assert_eq!(toml_edit::de::from_str::<Config>(&text).unwrap(), c);
    }

    #[test]
    fn a_write_that_changes_nothing_leaves_the_bytes_alone() {
        assert_eq!(written(SAMPLE, &sample()), SAMPLE);
    }

    #[test]
    fn rules_are_added_and_dropped_in_place() {
        let mut c = sample();
        let rules = &mut c.auto.get_mut("claude").unwrap().rules;
        rules.remove(0);
        rules.push(Rule {
            when: "workday".into(),
            order: vec![vec!["work".into()]],
            reserve: BTreeMap::new(),
        });

        let text = written(SAMPLE, &c);

        assert_eq!(toml_edit::de::from_str::<Config>(&text).unwrap(), c);
        assert_eq!(text.matches("[[auto.claude.rule]]").count(), 2);
    }

    #[test]
    fn a_renamed_schedule_keeps_its_comments_and_place() {
        let mut c = sample();
        let day = c.schedules.remove("workday").unwrap();
        c.schedules.insert("weekdays".into(), day.clone());
        c.schedules.insert("nights".into(), Schedule::default());
        let rules = &mut c.auto.get_mut("claude").unwrap().rules;
        rules[0].when = "weekdays".into();
        rules[1].reserve.insert("work".into(), "weekdays".into());

        let text = written(SAMPLE, &c);

        assert!(text.starts_with(
            "# auto mode\n[schedules.weekdays]\ndays  = [\"mon\", \"tue\", \"wed\", \"thu\", \"fri\"]\nhours = \"09:00-18:00\"   # local time\n\n[schedules.nights]\n\n[auto.claude]\n"
        ));
        assert_eq!(toml_edit::de::from_str::<Config>(&text).unwrap(), c);
    }

    #[test]
    fn an_added_rule_and_provider_land_after_their_siblings() {
        let mut c = sample();
        c.auto.get_mut("claude").unwrap().rules.push(Rule {
            when: "workday".into(),
            order: vec![vec!["perso".into()]],
            reserve: BTreeMap::new(),
        });
        c.auto.insert(
            "codex".into(),
            Auto {
                logins: vec!["team".into()],
                ..Auto::default()
            },
        );

        let text = written(&format!("{SAMPLE}# the end\n"), &c);

        assert!(text.starts_with(SAMPLE));
        assert!(text.ends_with(
            "[[auto.claude.rule]]\nwhen = \"workday\"\norder = [[\"perso\"]]\n\n[auto.codex]\nlogins = [\"team\"]\n# the end\n"
        ));
    }

    #[test]
    fn a_removed_rule_takes_its_comments_and_the_others_keep_theirs() {
        let text = SAMPLE
            .replace(
                "[[auto.claude.rule]]\nwhen  = \"workday\"",
                "# first\n[[auto.claude.rule]]\nwhen  = \"workday\"",
            )
            .replace(
                "[[auto.claude.rule]]\nwhen    = \"otherwise\"",
                "# second\n[[auto.claude.rule]]\nwhen    = \"otherwise\"",
            );
        let mut c = sample();
        c.auto.get_mut("claude").unwrap().rules.remove(0);

        let out = written(&text, &c);

        assert!(!out.contains("# first"), "{out}");
        assert!(
            out.contains("# second\n[[auto.claude.rule]]\nwhen    = \"otherwise\""),
            "{out}"
        );
        assert_eq!(toml_edit::de::from_str::<Config>(&out).unwrap(), c);
    }

    #[test]
    fn moved_rules_keep_their_comments_and_order() {
        let text = SAMPLE.replace(
            "[[auto.claude.rule]]\nwhen  = \"workday\"",
            "# first\n[[auto.claude.rule]]\nwhen  = \"workday\"",
        );
        let mut c = sample();
        c.auto.get_mut("claude").unwrap().rules.swap(0, 1);

        let out = written(&text, &c);

        assert_eq!(toml_edit::de::from_str::<Config>(&out).unwrap(), c);
        let first = out.find("# first").unwrap();
        assert!(
            out[first..].starts_with("# first\n[[auto.claude.rule]]\nwhen  = \"workday\""),
            "{out}"
        );
        assert!(
            out.find("when    = \"otherwise\"").unwrap() < first,
            "{out}"
        );
    }

    #[test]
    fn keys_written_at_their_default_stay_with_their_comments() {
        let text = "[auto.claude]\nenabled = false\ndwell = \"30m\" # default\nmargin = 0.25 # default\nlogins = [\"work\"]\n";
        let mut c: Config = toml_edit::de::from_str(text).unwrap();
        assert_eq!(written(text, &c), text);
        c.auto
            .get_mut("claude")
            .unwrap()
            .logins
            .push("perso".into());
        assert_eq!(
            written(text, &c),
            text.replace("[\"work\"]", "[\"work\", \"perso\"]")
        );
    }

    #[test]
    fn a_rule_with_no_tiers_and_a_dwell_out_of_range_are_problems() {
        let c: Config = toml_edit::de::from_str(
            "[auto.claude]\ndwell = \"30s\"\n[[auto.claude.rule]]\nwhen = \"otherwise\"\norder = []\n",
        )
        .unwrap();
        let problems: Vec<String> = c.check(&stored).iter().map(ToString::to_string).collect();
        assert_eq!(
            problems,
            [
                "auto.claude.rule[1].order: the rule names no login",
                "auto.claude.dwell: the least time on a login is between 1 minute and 7 days",
            ]
        );
        let long: Config = toml_edit::de::from_str("[auto.claude]\ndwell = \"169h\"\n").unwrap();
        assert_eq!(long.check(&stored).len(), 1);
    }

    #[test]
    fn ceilings_and_a_fallback_are_read_written_checked_and_kept_in_step() {
        let text = "[auto.claude]\nceiling = 97\nceilings = { work = 93 }\nfallback = \"perso\"\n";
        let c: Config = toml_edit::de::from_str(text).unwrap();
        let a = &c.auto["claude"];
        assert_eq!(
            (a.ceiling, a.ceilings["work"], a.fallback.as_deref()),
            (97, 93, Some("perso"))
        );
        assert_eq!(c.check(&stored), []);
        assert_eq!(written(text, &c), text);

        let bad: Config = toml_edit::de::from_str(
            "[auto.claude]\nceiling = 0\nceilings = { ghost = 101 }\nfallback = \"gone\"\n",
        )
        .unwrap();
        let problems: Vec<String> = bad.check(&stored).iter().map(ToString::to_string).collect();
        assert_eq!(
            problems,
            [
                "auto.claude.ceiling: a ceiling is a percentage, 1 to 100",
                "auto.claude.ceilings.ghost: no stored login claude/ghost",
                "auto.claude.ceilings.ghost: a ceiling is a percentage, 1 to 100",
                "auto.claude.fallback: no stored login claude/gone",
            ]
        );

        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.toml");
        std::fs::write(&path, text).unwrap();
        rename_login(&path, "claude", "work", "job").unwrap();
        rename_login(&path, "claude", "perso", "home").unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "[auto.claude]\nceiling = 97\nceilings = { job = 93 }\nfallback = \"home\"\n"
        );
        forget_login(&path, "claude", "job").unwrap();
        forget_login(&path, "claude", "home").unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "[auto.claude]\nceiling = 97\n"
        );
    }

    #[test]
    fn a_ceilings_table_written_by_hand_stays_a_table() {
        let text =
            "[auto.claude]\nenabled = true\n\n[auto.claude.ceilings]\nIDC = 93 # five hours\n";
        let mut c: Config = toml_edit::de::from_str(text).unwrap();
        c.auto
            .get_mut("claude")
            .unwrap()
            .ceilings
            .insert("Axeo".into(), 88);
        assert_eq!(
            written(text, &c),
            "[auto.claude]\nenabled = true\n\n[auto.claude.ceilings]\nIDC = 93 # five hours\nAxeo = 88\n"
        );
    }

    #[test]
    fn a_new_file_is_written_from_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("remuda/config.toml");
        let loaded = load(&path).unwrap();
        assert_eq!(loaded.config, Config::default());

        let v = save(&path, &sample(), &loaded.version).unwrap();

        let back = load(&path).unwrap();
        assert_eq!(back.config, sample());
        assert_eq!(back.version, v);
        assert!(!std::fs::read_to_string(&path).unwrap().contains("[auto]\n"));
    }

    #[test]
    fn a_write_against_an_old_version_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.toml");
        let old = load(&path).unwrap().version;
        std::fs::write(&path, SAMPLE).unwrap();

        let err = save(&path, &Config::default(), &old).unwrap_err();

        assert!(err.is::<Stale>());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), SAMPLE);
    }

    #[test]
    fn a_removed_login_leaves_its_tiers_and_reservations() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.toml");
        std::fs::write(&path, SAMPLE).unwrap();

        forget_login(&path, "claude", "work").unwrap();

        let rules = &load(&path).unwrap().config.auto["claude"].rules;
        assert_eq!(rules[0].order, [["perso"]]);
        assert_eq!(rules[1].order, [["perso"]]);
        assert!(rules[1].reserve.is_empty());
    }

    #[test]
    fn a_renamed_login_keeps_its_place() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.toml");
        std::fs::write(&path, SAMPLE).unwrap();

        rename_login(&path, "claude", "work", "job").unwrap();
        rename_login(&path, "codex", "perso", "x").unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("order = [[\"job\"], [\"perso\"]]"));
        assert!(text.contains("reserve = { job = \"workday\" }"));
        assert!(text.contains("hours = \"09:00-18:00\"   # local time"));
    }

    #[test]
    fn upkeep_writes_nothing_without_a_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.toml");
        forget_login(&path, "claude", "work").unwrap();
        assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), 0);
    }
}
