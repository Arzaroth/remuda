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
}

impl Default for Auto {
    fn default() -> Self {
        Auto {
            enabled: false,
            dwell: default_dwell(),
            margin: default_margin(),
            logins: Vec::new(),
            rules: Vec::new(),
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
    use sha2::{Digest, Sha256};
    Sha256::digest(text.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
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
    merge_table(doc.as_table_mut(), config.document().as_table());
    let body = doc.to_string();
    if body != text {
        fsx::write_private(path, &body)?;
    }
    Ok(version(&body))
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

fn merge_table(old: &mut Table, new: &Table) {
    let gone: Vec<String> = old
        .iter()
        .map(|(k, _)| k.to_owned())
        .filter(|k| !new.contains_key(k))
        .collect();
    for k in gone {
        old.remove(&k);
    }
    for (k, item) in new.iter() {
        match old.get_mut(k) {
            Some(existing) => merge_item(existing, item),
            None => {
                old.insert(k, item.clone());
            }
        }
    }
}

fn merge_item(old: &mut Item, new: &Item) {
    match (&mut *old, new) {
        (Item::Table(o), Item::Table(n)) => merge_table(o, n),
        (Item::ArrayOfTables(o), Item::ArrayOfTables(n)) => {
            while o.len() > n.len() {
                o.remove(o.len() - 1);
            }
            for (i, t) in n.iter().enumerate() {
                match o.get_mut(i) {
                    Some(existing) => merge_table(existing, t),
                    None => o.push(t.clone()),
                }
            }
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
