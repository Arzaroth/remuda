//! Auto mode's choice: which login a CLI should be on now, and whether to
//! move it there. Pure: the clock, the figures and the active login are given.

use std::collections::BTreeMap;

use jiff::civil::{Date, Weekday};
use jiff::{Timestamp, Zoned};
use serde::Serialize;
use serde_json::Value;

use crate::config::{Auto, Day, OTHERWISE, Rule, Schedule};
use crate::provider::Provider;

const HOUR_MS: f64 = 3_600_000.0;
/// Older figures are not acted on: TokenGauge fetches every few minutes.
const STALE_MS: i64 = 30 * 60_000;

#[derive(Debug, Clone, PartialEq)]
pub struct Window {
    pub title: String,
    pub minutes: Option<u64>,
    pub used: u64,
    pub resets_at: Option<Timestamp>,
    /// One of TokenGauge's extra windows: a limit on one model or feature,
    /// not on the account.
    pub extra: bool,
}

/// What TokenGauge says of a login: its windows, or why there are none.
#[derive(Debug, Clone, PartialEq)]
pub enum Figures {
    Known(Vec<Window>),
    Missing(String),
}

fn day(w: Weekday) -> Day {
    match w {
        Weekday::Monday => Day::Mon,
        Weekday::Tuesday => Day::Tue,
        Weekday::Wednesday => Day::Wed,
        Weekday::Thursday => Day::Thu,
        Weekday::Friday => Day::Fri,
        Weekday::Saturday => Day::Sat,
        Weekday::Sunday => Day::Sun,
    }
}

fn on(s: &Schedule, d: Date) -> bool {
    s.days.is_empty() || s.days.contains(&day(d.weekday()))
}

/// A range that runs past midnight belongs to the day it started on.
pub fn contains(s: &Schedule, now: &Zoned) -> bool {
    let dt = now.datetime();
    let m = dt.hour() as u16 * 60 + dt.minute() as u16;
    match s.hours {
        None => on(s, dt.date()),
        Some(h) if h.start < h.end => on(s, dt.date()) && h.start <= m && m < h.end,
        Some(h) => {
            (m >= h.start && on(s, dt.date()))
                || (m < h.end && dt.date().yesterday().is_ok_and(|y| on(s, y)))
        }
    }
}

/// The first moment after `now` the schedule starts.
pub fn next_start(s: &Schedule, now: &Zoned) -> Option<Zoned> {
    let start = s.hours.map_or(0, |h| h.start);
    let today = now.date();
    (0..=7).find_map(|d| {
        let date = today.checked_add(jiff::Span::new().days(d)).ok()?;
        if !on(s, date) {
            return None;
        }
        let at = date
            .at((start / 60) as i8, (start % 60) as i8, 0, 0)
            .to_zoned(now.time_zone().clone())
            .ok()?;
        (at > *now).then_some(at)
    })
}

/// The account's own windows; a model's or a feature's limit only when the
/// account has no other, as Codex's monthly one.
fn counted(windows: &[Window]) -> impl Iterator<Item = &Window> {
    let account = windows.iter().any(|w| !w.extra);
    windows.iter().filter(move |w| !account || !w.extra)
}

fn longest(windows: &[Window]) -> Option<&Window> {
    counted(windows).max_by_key(|w| (w.minutes.unwrap_or(0), w.resets_at))
}

/// The percentage `w`, one of `name`'s windows, counts as full at: the
/// login's own ceiling for the shortest of several windows (Claude's 5-hour
/// one), the CLI's for the rest and for a login with one window.
fn cap(auto: &Auto, name: &str, windows: &[Window], w: &Window) -> u64 {
    let shortest = (counted(windows).count() > 1)
        .then(|| counted(windows).min_by_key(|x| x.minutes.unwrap_or(u64::MAX)))
        .flatten();
    match auto.ceilings.get(name) {
        Some(c) if shortest.is_some_and(|s| std::ptr::eq(s, w)) => *c,
        _ => auto.ceiling,
    }
}

/// The first of the login's windows at or over its ceiling, with that ceiling.
fn full<'a>(auto: &Auto, name: &str, windows: &'a [Window]) -> Option<(&'a Window, u64)> {
    counted(windows)
        .map(|w| (w, cap(auto, name, windows, w)))
        .find(|(w, c)| w.used >= *c)
}

/// A window whose reset has passed since TokenGauge read it is empty again.
fn current(f: &Figures, now: Timestamp) -> Figures {
    match f {
        Figures::Known(ws) => Figures::Known(
            ws.iter()
                .map(|w| match w.resets_at {
                    Some(at) if at <= now => Window {
                        used: 0,
                        resets_at: None,
                        ..w.clone()
                    },
                    _ => w.clone(),
                })
                .collect(),
        ),
        missing => missing.clone(),
    }
}

/// Headroom left in the longest window per hour until it resets: what would
/// be lost at the reset if the login sat idle.
fn score(auto: &Auto, name: &str, windows: &[Window], now: Timestamp) -> Option<f64> {
    let w = longest(windows)?;
    let room = cap(auto, name, windows, w).saturating_sub(w.used);
    let hours = match w.resets_at {
        Some(at) => (at.as_millisecond() - now.as_millisecond()) as f64 / HOUR_MS,
        None => w.minutes.map_or(168.0, |m| m as f64 / 60.0),
    };
    Some(room as f64 / hours.max(1.0))
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Seen {
    pub name: String,
    /// 0 is the first tier; none when auto mode may not pick it now.
    pub tier: Option<usize>,
    pub score: Option<f64>,
    pub usable: bool,
    pub maxed: bool,
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Action {
    Stay { why: String },
    Switch { to: String, why: String },
    Hold { why: String },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Decision {
    /// The rule in force, as `rule 2 (otherwise)`, or none for the logins
    /// picked when no rule is.
    pub rule: Option<String>,
    pub logins: Vec<Seen>,
    pub action: Action,
}

impl Decision {
    pub fn hold(why: impl Into<String>) -> Self {
        Decision {
            rule: None,
            logins: Vec::new(),
            action: Action::Hold { why: why.into() },
        }
    }
}

pub struct Input<'a> {
    pub auto: &'a Auto,
    pub schedules: &'a BTreeMap<String, Schedule>,
    pub now: &'a Zoned,
    /// The stored login the CLI is on, if any.
    pub active: Option<&'a str>,
    /// When the CLI last switched, by hand or by auto mode.
    pub switched_at: Option<Timestamp>,
    pub figures: &'a BTreeMap<String, Figures>,
}

fn in_force<'a>(input: &Input<'a>) -> Option<(usize, &'a Rule)> {
    input.auto.rules.iter().enumerate().find(|(_, r)| {
        r.when == OTHERWISE
            || input
                .schedules
                .get(&r.when)
                .is_some_and(|s| contains(s, input.now))
    })
}

fn clock(t: &Zoned) -> String {
    t.strftime("%a %H:%M").to_string()
}

pub fn decide(input: &Input) -> Decision {
    let now = input.now.timestamp();
    let figures: BTreeMap<String, Figures> = input
        .figures
        .iter()
        .map(|(name, f)| (name.clone(), current(f, now)))
        .collect();
    let (rule, mut tiers, reserve) = match in_force(input) {
        Some((i, r)) => (
            Some(format!("rule {} ({})", i + 1, r.when)),
            r.order.clone(),
            r.reserve.clone(),
        ),
        None if !input.auto.logins.is_empty() => {
            (None, vec![input.auto.logins.clone()], BTreeMap::new())
        }
        None => return Decision::hold("no rule is in force, and no logins are set for that case"),
    };

    let mut notes: BTreeMap<String, String> = BTreeMap::new();
    let mut kept = Vec::new();
    let mut freed = Vec::new();
    for (login, schedule) in &reserve {
        let Some(s) = input.schedules.get(schedule) else {
            continue;
        };
        if contains(s, input.now) {
            continue;
        }
        let resets = match figures.get(login) {
            Some(Figures::Known(w)) => longest(w).and_then(|w| w.resets_at),
            _ => None,
        };
        let starts = next_start(s, input.now);
        match (resets, &starts) {
            (Some(r), Some(st)) if r <= st.timestamp() => {
                for t in tiers.iter_mut() {
                    t.retain(|n| n != login);
                }
                freed.push(login.clone());
                notes.insert(
                    login.clone(),
                    format!("kept for {schedule}, free now: it resets before {schedule} starts"),
                );
            }
            _ => {
                kept.push(login.clone());
                let when = starts
                    .map(|s| format!(" ({})", clock(&s)))
                    .unwrap_or_default();
                notes.insert(login.clone(), format!("kept for {schedule}{when}"));
            }
        }
    }

    if !freed.is_empty() {
        tiers.retain(|t| !t.is_empty());
        tiers.insert(0, freed);
    }

    let mut logins = Vec::new();
    for (tier, names) in tiers.iter().enumerate() {
        for name in names {
            if kept.contains(name) {
                continue;
            }
            let mut seen = Seen {
                name: name.clone(),
                tier: Some(tier),
                score: None,
                usable: false,
                maxed: false,
                note: notes.remove(name).unwrap_or_default(),
            };
            match figures.get(name) {
                Some(Figures::Known(w)) if !w.is_empty() => {
                    seen.score = score(input.auto, name, w, now);
                    match full(input.auto, name, w) {
                        Some((at, c)) => {
                            seen.maxed = true;
                            seen.note = full_note(at, c);
                        }
                        None => seen.usable = true,
                    }
                }
                Some(Figures::Missing(why)) => seen.note = why.clone(),
                _ => seen.note = "no figures from TokenGauge".into(),
            }
            logins.push(seen);
        }
    }
    for name in kept {
        logins.push(Seen {
            name: name.clone(),
            tier: None,
            score: None,
            usable: false,
            maxed: false,
            note: notes.remove(&name).unwrap_or_default(),
        });
    }

    let active_full = input.active.and_then(|a| match figures.get(a) {
        Some(Figures::Known(w)) => full(input.auto, a, w).map(|(at, c)| full_note(at, c)),
        _ => None,
    });
    let fallback = input
        .auto
        .fallback
        .as_deref()
        .filter(|f| !logins.iter().any(|s| s.name == *f && s.tier.is_none()))
        .filter(|f| match figures.get(*f) {
            Some(Figures::Known(w)) if !w.is_empty() => counted(w).all(|w| w.used < 100),
            _ => false,
        });
    let fallback = fallback.filter(|_| input.active.is_none() || active_full.is_some());
    let action = act(input, &logins, active_full, fallback);
    Decision {
        rule,
        logins,
        action,
    }
}

fn full_note(w: &Window, cap: u64) -> String {
    let title = w.title.to_lowercase();
    if cap >= 100 {
        format!("{title} limit maxed")
    } else {
        format!("{title} at {}%, full at {cap}%", w.used)
    }
}

/// `active_full` says why the active login is full, wherever it sits;
/// `fallback` is the login to go to when every other is too, given only when
/// the active one is full or there is none, and the fallback has room and is
/// not kept for a schedule.
fn act(
    input: &Input,
    logins: &[Seen],
    active_full: Option<String>,
    fallback: Option<&str>,
) -> Action {
    let best = logins.iter().filter(|s| s.usable).min_by(|a, b| {
        a.tier
            .cmp(&b.tier)
            .then(b.score.unwrap_or(0.0).total_cmp(&a.score.unwrap_or(0.0)))
    });
    let Some(best) = best else {
        let why = "every login it may pick is full or has no figures";
        return match fallback {
            Some(f) if input.active != Some(f) => Action::Switch {
                to: f.to_owned(),
                why: format!("{why}; {f} is the fallback"),
            },
            Some(f) => Action::Stay {
                why: format!("{why}; staying on {f}, the fallback"),
            },
            None => Action::Stay { why: why.into() },
        };
    };
    let Some(active) = input.active else {
        return Action::Switch {
            to: best.name.clone(),
            why: "no stored login is in use".into(),
        };
    };
    if active == best.name {
        return Action::Stay {
            why: format!("{active} is the best pick"),
        };
    }
    let current = logins.iter().find(|s| s.name == active);
    if let Some(full) = active_full {
        return Action::Switch {
            to: best.name.clone(),
            why: format!("{active}'s {full}"),
        };
    }
    if let Some(c) = current
        && c.tier.is_some()
        && !c.usable
    {
        return Action::Stay {
            why: format!("{active} has no figures to compare: {}", c.note),
        };
    }
    let dwell_ms = i64::try_from(input.auto.dwell.0.saturating_mul(1000)).unwrap_or(i64::MAX);
    if let Some(at) = input.switched_at {
        let until = at.as_millisecond().saturating_add(dwell_ms);
        if input.now.timestamp().as_millisecond() < until {
            let until = Timestamp::from_millisecond(until)
                .map(|t| clock(&t.to_zoned(input.now.time_zone().clone())))
                .unwrap_or_default();
            return Action::Stay {
                why: format!("stays on {active} until {until}, the least time on a login"),
            };
        }
    }
    let Some(c) = current.filter(|c| c.tier.is_some()) else {
        return Action::Switch {
            to: best.name.clone(),
            why: format!(
                "{active} is not one it may pick now{}",
                current.map(|c| format!(": {}", c.note)).unwrap_or_default()
            ),
        };
    };
    if best.tier < c.tier {
        return Action::Switch {
            to: best.name.clone(),
            why: format!("{} comes before {active}", best.name),
        };
    }
    let (b, a) = (best.score.unwrap_or(0.0), c.score.unwrap_or(0.0));
    if b > a * (1.0 + input.auto.margin) {
        Action::Switch {
            to: best.name.clone(),
            why: format!(
                "{} has more to spend before its reset ({b:.1}%/h against {a:.1}%/h)",
                best.name
            ),
        }
    } else {
        Action::Stay {
            why: format!(
                "{active} is within {:.0}% of {} ({a:.1}%/h against {b:.1}%/h)",
                input.auto.margin * 100.0,
                best.name
            ),
        }
    }
}

fn windows(v: &Value) -> Vec<Window> {
    v.get("windows")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|w| {
            Some(Window {
                title: w
                    .get("title")
                    .and_then(Value::as_str)
                    .unwrap_or("usage")
                    .to_owned(),
                extra: w.get("extra").and_then(Value::as_bool).unwrap_or(false),
                minutes: w.get("minutes").and_then(Value::as_u64),
                used: w.get("usedPercent")?.as_u64()?,
                resets_at: w
                    .get("resetsAt")
                    .and_then(Value::as_str)
                    .and_then(|s| s.parse().ok()),
            })
        })
        .collect()
}

fn figures_of(v: &Value) -> Figures {
    if let Some(e) = v.get("error").and_then(Value::as_str) {
        return Figures::Missing(format!("TokenGauge: {e}"));
    }
    if v.get("stale").and_then(Value::as_bool).unwrap_or(false) {
        let why = v
            .get("staleReason")
            .and_then(Value::as_str)
            .unwrap_or("stale");
        return Figures::Missing(format!("TokenGauge's figures are stale: {why}"));
    }
    match windows(v) {
        w if w.is_empty() => Figures::Missing("no figures from TokenGauge".into()),
        w => Figures::Known(w),
    }
}

/// Each login's figures from [`crate::gauge::read`]'s output, or why none can
/// be trusted this round. A snapshot that names no credential holds the
/// active login's.
pub fn figures(
    usage: Option<&Value>,
    provider: &str,
    active: Option<&str>,
    switched_at: Option<Timestamp>,
    now: Timestamp,
) -> Result<BTreeMap<String, Figures>, String> {
    let usage = usage.ok_or("TokenGauge has written no figures")?;
    let p = usage
        .pointer(&format!("/providers/{provider}"))
        .filter(|p| !p.is_null())
        .ok_or_else(|| format!("TokenGauge has no figures for {provider}"))?;
    let updated = usage.get("updatedAt").and_then(Value::as_i64);
    if let Some(updated) = updated
        && now.as_millisecond() - updated > STALE_MS
    {
        return Err(format!(
            "TokenGauge's figures are {} minutes old; is its daemon running?",
            (now.as_millisecond() - updated) / 60_000
        ));
    }
    if let (Some(at), Some(updated)) = (switched_at, updated)
        && updated < at.as_millisecond()
    {
        return Err("waiting for TokenGauge to fetch after the last switch".into());
    }
    let accounts = p.get("accounts").and_then(Value::as_object);
    let mut out: BTreeMap<String, Figures> = accounts
        .into_iter()
        .flatten()
        .map(|(name, v)| (name.clone(), figures_of(v)))
        .collect();
    if let Some(active) = active
        && !out.contains_key(active)
    {
        out.insert(active.to_owned(), figures_of(p));
    }
    Ok(out)
}

/// How many processes under `proc` (a `/proc`) run one of `names`: by
/// name, by a release binary named after its target (`codex-x86_64-...`), or
/// as a script under node, bun or deno. `me` is left out.
pub fn running(proc: &std::path::Path, names: &[&str], me: u32) -> std::io::Result<usize> {
    let is = |base: &[u8]| {
        names.iter().any(|n| {
            let n = n.as_bytes();
            base == n
                || base
                    .strip_prefix(n)
                    .is_some_and(|rest| rest.starts_with(b"-") || rest == b".js")
        })
    };
    let base = |arg: &[u8]| {
        arg.rsplit(|b| *b == b'/')
            .next()
            .unwrap_or_default()
            .to_vec()
    };
    let mut count = 0;
    for entry in std::fs::read_dir(proc)? {
        let entry = entry?;
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|n| n.parse::<u32>().ok())
        else {
            continue;
        };
        if pid == me {
            continue;
        }
        let comm = std::fs::read(entry.path().join("comm")).unwrap_or_default();
        let cmdline = std::fs::read(entry.path().join("cmdline")).unwrap_or_default();
        let args: Vec<Vec<u8>> = cmdline.split(|b| *b == 0).take(2).map(base).collect();
        let script = args
            .first()
            .is_some_and(|a| matches!(a.as_slice(), b"node" | b"bun" | b"deno"))
            && args.get(1).is_some_and(|a| is(a));
        if is(comm.trim_ascii_end()) || args.first().is_some_and(|a| is(a)) || script {
            count += 1;
        }
    }
    Ok(count)
}

/// Why a switch has to wait for the CLI's sessions to end, if it does
/// ([`Provider::sessions`]). Not seeing the processes counts as seeing one.
pub fn sessions_open(p: &dyn Provider) -> Option<String> {
    let names = p.sessions();
    if names.is_empty() {
        return None;
    }
    match running(std::path::Path::new("/proc"), names, std::process::id()) {
        Ok(0) => None,
        Ok(_) => Some(format!("waiting for every {} session to end", p.name())),
        Err(e) => Some(format!("cannot see whether {} runs: {e}", p.name())),
    }
}

pub struct Context<'a> {
    pub schedules: &'a BTreeMap<String, Schedule>,
    pub usage: Option<&'a Value>,
    pub switched_at: Option<i64>,
    pub now: &'a Zoned,
}

/// The decision for one CLI from its live login and TokenGauge's snapshot. A
/// switch is held while the CLI has sessions it must wait out.
pub fn assess(
    auto: &Auto,
    p: &dyn Provider,
    live: &crate::ops::LiveState,
    cx: &Context,
) -> Decision {
    let provider = p.id();
    use crate::ops::LiveState;
    let active = match live {
        LiveState::SignedOut => None,
        LiveState::Stored { name, synced: true } => Some(name.as_str()),
        LiveState::Stored { name, .. } => {
            return Decision::hold(format!(
                "the live login could not be confirmed as {name}; a switch would lose its tokens"
            ));
        }
        LiveState::Unstored { email } => {
            return Decision::hold(format!(
                "the CLI is on a login remuda has not stored{}: import it first",
                email
                    .as_deref()
                    .map(|e| format!(" ({e})"))
                    .unwrap_or_default()
            ));
        }
        LiveState::Foreign { what } => {
            return Decision::hold(format!("the CLI is on {what}, which remuda cannot store"));
        }
        LiveState::Unreadable { error } => {
            return Decision::hold(format!("the CLI's files cannot be read: {error}"));
        }
    };
    let switched_at = cx
        .switched_at
        .and_then(|ms| Timestamp::from_millisecond(ms).ok());
    let figures = match figures(cx.usage, provider, active, switched_at, cx.now.timestamp()) {
        Ok(f) => f,
        Err(why) => return Decision::hold(why),
    };
    let mut decision = decide(&Input {
        auto,
        schedules: cx.schedules,
        now: cx.now,
        active,
        switched_at,
        figures: &figures,
    });
    if let Action::Switch { to, why } = &decision.action
        && let Some(busy) = sessions_open(p)
    {
        decision.action = Action::Hold {
            why: format!("would switch to {to} ({why}), {busy}"),
        };
    }
    decision
}

pub fn describe(
    name: &str,
    auto: &Auto,
    active: Option<&str>,
    d: &Decision,
    out: &mut dyn std::io::Write,
) -> std::io::Result<()> {
    let state = if auto.enabled { "on" } else { "off" };
    let rule = d.rule.as_deref().unwrap_or("no rule in force");
    writeln!(out, "{name}: auto mode {state}, {rule}")?;
    let width = d.logins.iter().map(|s| s.name.len()).max().unwrap_or(0);
    for s in &d.logins {
        let tier = s.tier.map_or("-".to_owned(), |t| (t + 1).to_string());
        let score = s.score.map_or(String::new(), |v| format!("{v:.1}%/h"));
        let mut note = s.note.clone();
        if Some(s.name.as_str()) == active {
            note = if note.is_empty() {
                "in use".into()
            } else {
                format!("in use, {note}")
            };
        }
        writeln!(out, "  {tier}  {:width$}  {score:>8}  {note}", s.name)?;
    }
    let would = if auto.enabled { "" } else { "would " };
    let (verb, why) = match &d.action {
        Action::Stay { why } => (format!("{would}stay"), why),
        Action::Switch { to, why } => (format!("{would}switch to {to}"), why),
        Action::Hold { why } => (format!("{would}hold"), why),
    };
    writeln!(out, "  {verb}: {why}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, Hours, Span};
    use jiff::tz::TimeZone;

    const H: i64 = 3_600_000;

    /// Wednesday 2026-10-21, at `hhmm` UTC.
    fn at(hhmm: &str) -> Zoned {
        format!("2026-10-21T{hhmm}:00Z")
            .parse::<Timestamp>()
            .unwrap()
            .to_zoned(TimeZone::UTC)
    }

    fn config() -> Config {
        toml_edit::de::from_str(
            r#"
[schedules.workday]
days  = ["mon", "tue", "wed", "thu", "fri"]
hours = "09:00-18:00"

[auto.claude]
enabled = true

[[auto.claude.rule]]
when  = "workday"
order = [["work"], ["perso"]]

[[auto.claude.rule]]
when    = "otherwise"
order   = [["perso"], ["work"]]
reserve = { work = "workday" }
"#,
        )
        .unwrap()
    }

    /// A login with a 5-hour and a weekly window, `used` percent each,
    /// resetting in `five` and `week` hours from `now`.
    fn login(now: &Zoned, five: (u64, i64), week: (u64, i64)) -> Figures {
        let t = |h: i64| {
            Some(Timestamp::from_millisecond(now.timestamp().as_millisecond() + h * H).unwrap())
        };
        Figures::Known(vec![
            Window {
                title: "5 hours".into(),
                minutes: Some(300),
                used: five.0,
                resets_at: t(five.1),
                extra: false,
            },
            Window {
                title: "Weekly".into(),
                minutes: Some(10080),
                used: week.0,
                resets_at: t(week.1),
                extra: false,
            },
        ])
    }

    fn run(
        c: &Config,
        now: &Zoned,
        active: Option<&str>,
        switched: Option<i64>,
        f: &[(&str, Figures)],
    ) -> Decision {
        decide(&Input {
            auto: &c.auto["claude"],
            schedules: &c.schedules,
            now,
            active,
            switched_at: switched.map(|h| {
                Timestamp::from_millisecond(now.timestamp().as_millisecond() - h * H).unwrap()
            }),
            figures: &f.iter().map(|(n, x)| (n.to_string(), x.clone())).collect(),
        })
    }

    fn switch_to(d: &Decision) -> Option<&str> {
        match &d.action {
            Action::Switch { to, .. } => Some(to),
            _ => None,
        }
    }

    #[test]
    fn a_schedule_holds_its_days_and_hours_and_runs_past_midnight() {
        let workday = &config().schedules["workday"];
        assert!(contains(workday, &at("09:00")));
        assert!(!contains(workday, &at("18:00")));
        assert!(!contains(workday, &at("08:59")));
        let nights = Schedule {
            days: vec![Day::Tue],
            hours: Some(Hours {
                start: 22 * 60,
                end: 6 * 60,
            }),
        };
        assert!(
            contains(&nights, &at("05:59")),
            "Tuesday's night, on Wednesday"
        );
        assert!(!contains(&nights, &at("22:00")), "Wednesday has no night");
        let weekend = Schedule {
            days: vec![Day::Sat, Day::Sun],
            hours: None,
        };
        assert!(!contains(&weekend, &at("12:00")));
        assert_eq!(
            next_start(&weekend, &at("12:00")).unwrap().to_string(),
            "2026-10-24T00:00:00+00:00[UTC]"
        );
        assert_eq!(
            next_start(workday, &at("12:00")).unwrap().to_string(),
            "2026-10-22T09:00:00+00:00[UTC]"
        );
        assert_eq!(
            next_start(workday, &at("08:00")).unwrap().to_string(),
            "2026-10-21T09:00:00+00:00[UTC]"
        );
    }

    #[test]
    fn during_work_hours_the_first_tier_wins_over_a_better_score() {
        let c = config();
        let now = at("10:00");
        let d = run(
            &c,
            &now,
            Some("perso"),
            None,
            &[
                ("work", login(&now, (10, 2), (10, 150))),
                ("perso", login(&now, (10, 2), (10, 10))),
            ],
        );
        assert_eq!(d.rule.as_deref(), Some("rule 1 (workday)"));
        assert_eq!(switch_to(&d), Some("work"));
    }

    #[test]
    fn a_maxed_login_is_left_at_once_for_the_next_tier() {
        let c = config();
        let now = at("10:00");
        let d = run(
            &c,
            &now,
            Some("work"),
            Some(0),
            &[
                ("work", login(&now, (100, 2), (40, 50))),
                ("perso", login(&now, (10, 2), (10, 100))),
            ],
        );
        assert_eq!(switch_to(&d), Some("perso"));
        assert!(
            matches!(&d.action, Action::Switch { why, .. } if why == "work's 5 hours limit maxed")
        );
    }

    #[test]
    fn at_night_a_reserved_login_waits_for_its_schedule() {
        let c = config();
        let now = at("20:00");
        let d = run(
            &c,
            &now,
            Some("perso"),
            None,
            &[
                ("work", login(&now, (0, 2), (50, 30))),
                ("perso", login(&now, (100, 2), (10, 100))),
            ],
        );
        let work = d.logins.iter().find(|s| s.name == "work").unwrap();
        assert_eq!(
            (work.tier, work.note.as_str()),
            (None, "kept for workday (Thu 09:00)")
        );
        assert_eq!(
            d.action,
            Action::Stay {
                why: "every login it may pick is full or has no figures".into()
            }
        );
    }

    #[test]
    fn at_night_a_reserved_login_whose_week_ends_first_is_spent_first() {
        let c = config();
        let now = at("20:00");
        let d = run(
            &c,
            &now,
            Some("perso"),
            None,
            &[
                ("work", login(&now, (0, 2), (50, 6))),
                ("perso", login(&now, (10, 2), (10, 100))),
            ],
        );
        let work = d.logins.iter().find(|s| s.name == "work").unwrap();
        assert_eq!(work.tier, Some(0));
        assert!(work.note.starts_with("kept for workday, free now"));
        assert_eq!(switch_to(&d), Some("work"));
    }

    #[test]
    fn within_a_tier_the_headroom_a_reset_would_wipe_goes_first() {
        let mut c = config();
        let a = c.auto.get_mut("claude").unwrap();
        a.rules.clear();
        a.logins = vec!["a".into(), "b".into()];
        let now = at("10:00");
        let figures = [
            ("a", login(&now, (0, 2), (60, 24))),
            ("b", login(&now, (0, 2), (10, 144))),
        ];
        let d = run(&c, &now, Some("b"), None, &figures);
        assert_eq!(d.rule, None);
        assert_eq!(switch_to(&d), Some("a"), "40%/24h beats 90%/144h");
        let d = run(&c, &now, Some("a"), None, &figures);
        assert_eq!(
            d.action,
            Action::Stay {
                why: "a is the best pick".into()
            }
        );
    }

    #[test]
    fn a_small_gain_or_a_recent_switch_keeps_the_login() {
        let mut c = config();
        let a = c.auto.get_mut("claude").unwrap();
        a.rules.clear();
        a.logins = vec!["a".into(), "b".into()];
        let now = at("10:00");
        let close = [
            ("a", login(&now, (0, 2), (45, 50))),
            ("b", login(&now, (0, 2), (50, 50))),
        ];
        let d = run(&c, &now, Some("b"), None, &close);
        assert!(
            matches!(&d.action, Action::Stay { why } if why.starts_with("b is within 25% of a"))
        );

        let far = [
            ("a", login(&now, (0, 2), (50, 10))),
            ("b", login(&now, (0, 2), (10, 150))),
        ];
        c.auto.get_mut("claude").unwrap().dwell = Span(3600);
        let d = run(&c, &now, Some("b"), Some(0), &far);
        assert_eq!(
            d.action,
            Action::Stay {
                why: "stays on b until Wed 11:00, the least time on a login".into()
            }
        );
        let d = run(&c, &now, Some("b"), Some(2), &far);
        assert_eq!(switch_to(&d), Some("a"));
    }

    #[test]
    fn a_login_without_figures_is_not_picked_and_the_active_one_is_not_left_blind() {
        let c = config();
        let now = at("10:00");
        let d = run(
            &c,
            &now,
            Some("perso"),
            None,
            &[
                (
                    "work",
                    Figures::Missing("TokenGauge: 401 Unauthorized".into()),
                ),
                ("perso", login(&now, (10, 2), (10, 100))),
            ],
        );
        assert_eq!(
            d.action,
            Action::Stay {
                why: "perso is the best pick".into()
            }
        );
        let d = run(
            &c,
            &now,
            Some("work"),
            None,
            &[
                (
                    "work",
                    Figures::Missing("TokenGauge: 401 Unauthorized".into()),
                ),
                ("perso", login(&now, (10, 2), (10, 100))),
            ],
        );
        assert!(
            matches!(&d.action, Action::Stay { why } if why.starts_with("work has no figures"))
        );
    }

    #[test]
    fn a_login_outside_the_rule_is_left_once_its_time_is_up() {
        let c = config();
        let now = at("10:00");
        let f = [
            ("work", login(&now, (10, 2), (10, 100))),
            ("perso", login(&now, (10, 2), (10, 100))),
        ];
        let d = run(&c, &now, Some("other"), Some(0), &f);
        assert!(matches!(d.action, Action::Stay { .. }));
        let d = run(&c, &now, Some("other"), Some(1), &f);
        assert_eq!(switch_to(&d), Some("work"));
        let d = run(&c, &now, None, Some(0), &f);
        assert_eq!(switch_to(&d), Some("work"), "nothing signed in");
    }

    #[test]
    fn figures_are_held_back_until_tokengauge_has_caught_up() {
        let usage = serde_json::json!({
            "updatedAt": 1000,
            "providers": {"claude": {"windows": [{"minutes": 300, "usedPercent": 5, "resetsAt": null}],
                "accounts": {"work": {"error": "401 Unauthorized", "windows": []},
                             "perso": {"stale": true, "staleReason": "timed out", "windows": []}}}}
        });
        let ms = |v| Timestamp::from_millisecond(v).ok();
        let now = ms(60_000).unwrap();
        assert_eq!(
            figures(Some(&usage), "claude", None, ms(2000), now),
            Err("waiting for TokenGauge to fetch after the last switch".into())
        );
        assert_eq!(
            figures(None, "claude", None, None, now),
            Err("TokenGauge has written no figures".into())
        );
        assert_eq!(
            figures(Some(&usage), "codex", None, None, now),
            Err("TokenGauge has no figures for codex".into())
        );
        assert_eq!(
            figures(
                Some(&usage),
                "claude",
                None,
                None,
                ms(1000 + 31 * 60_000).unwrap()
            ),
            Err("TokenGauge's figures are 31 minutes old; is its daemon running?".into())
        );
        let f = figures(Some(&usage), "claude", Some("old"), ms(500), now).unwrap();
        assert_eq!(
            f["work"],
            Figures::Missing("TokenGauge: 401 Unauthorized".into())
        );
        assert_eq!(
            f["perso"],
            Figures::Missing("TokenGauge's figures are stale: timed out".into())
        );
        assert_eq!(
            f["old"],
            Figures::Known(vec![Window {
                title: "usage".into(),
                minutes: Some(300),
                used: 5,
                resets_at: None,
                extra: false,
            }]),
            "a snapshot naming no credential holds the active login's"
        );
    }

    #[test]
    fn a_live_login_auto_mode_cannot_keep_holds_it() {
        use crate::ops::LiveState;
        let c = config();
        let now = at("10:00");
        let cx = Context {
            schedules: &c.schedules,
            usage: None,
            switched_at: None,
            now: &now,
        };
        let tmp = tempfile::tempdir().unwrap();
        let claude =
            crate::claude::Claude::at(tmp.path(), crate::claude::Api::local("http://127.0.0.1:9"));
        let why = |live: LiveState| match assess(&c.auto["claude"], &claude, &live, &cx).action {
            Action::Hold { why } => why,
            other => panic!("{other:?}"),
        };
        assert_eq!(
            why(LiveState::Unstored {
                email: Some("x@example.com".into())
            }),
            "the CLI is on a login remuda has not stored (x@example.com): import it first"
        );
        assert!(
            why(LiveState::Stored {
                name: "work".into(),
                synced: false
            })
            .contains("could not be confirmed")
        );
        assert_eq!(
            why(LiveState::SignedOut),
            "TokenGauge has written no figures"
        );
    }

    fn process(proc: &std::path::Path, pid: u32, comm: &str, args: &[&str]) {
        let dir = proc.join(pid.to_string());
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("comm"), format!("{comm}\n")).unwrap();
        std::fs::write(dir.join("cmdline"), args.join("\0")).unwrap();
    }

    #[test]
    fn codex_sessions_are_found_by_name_release_binary_and_script() {
        let tmp = tempfile::tempdir().unwrap();
        let proc = tmp.path();
        std::fs::create_dir(proc.join("self")).unwrap();
        process(proc, 10, "zsh", &["-zsh"]);
        process(proc, 11, "vim", &["vim", "codex"]);
        process(proc, 12, "codexx", &["/usr/bin/codexx"]);
        assert_eq!(running(proc, &["codex"], 1).unwrap(), 0);

        process(proc, 20, "codex", &["codex", "exec"]);
        process(
            proc,
            21,
            "codex-x86_64-un",
            &["/opt/codex-x86_64-unknown-linux-musl"],
        );
        process(
            proc,
            22,
            "node",
            &["/usr/bin/node", "/usr/lib/node_modules/codex.js"],
        );
        assert_eq!(running(proc, &["codex"], 1).unwrap(), 3);
        assert_eq!(
            running(proc, &["codex"], 20).unwrap(),
            2,
            "itself is left out"
        );
        assert!(running(&proc.join("gone"), &["codex"], 1).is_err());
    }

    #[test]
    fn only_a_cli_that_names_its_sessions_waits_for_them() {
        let tmp = tempfile::tempdir().unwrap();
        let claude =
            crate::claude::Claude::at(tmp.path(), crate::claude::Api::local("http://127.0.0.1:9"));
        assert_eq!(sessions_open(&claude), None);
    }

    #[test]
    fn a_model_limit_does_not_max_the_account_but_counts_when_it_is_all_there_is() {
        let c = config();
        let now = at("10:00");
        let scoped = |used| Window {
            title: "Fable only".into(),
            minutes: Some(10080),
            used,
            resets_at: None,
            extra: true,
        };
        let Figures::Known(mut perso) = login(&now, (10, 2), (10, 100)) else {
            unreachable!()
        };
        perso.push(scoped(100));
        let d = run(
            &c,
            &now,
            Some("work"),
            None,
            &[
                ("work", login(&now, (100, 2), (10, 100))),
                ("perso", Figures::Known(perso)),
            ],
        );
        assert_eq!(switch_to(&d), Some("perso"));
        assert!(
            matches!(&d.action, Action::Switch { why, .. } if why == "work's 5 hours limit maxed")
        );

        let d = run(
            &c,
            &now,
            Some("work"),
            None,
            &[
                ("work", Figures::Known(vec![scoped(100)])),
                ("perso", login(&now, (10, 2), (10, 100))),
            ],
        );
        assert_eq!(switch_to(&d), Some("perso"), "the only window counts");
    }

    #[test]
    fn a_window_whose_reset_has_passed_is_empty_again() {
        let c = config();
        let now = at("10:00");
        let d = run(
            &c,
            &now,
            Some("work"),
            Some(0),
            &[
                ("work", login(&now, (100, -1), (10, 100))),
                ("perso", login(&now, (10, 2), (10, 100))),
            ],
        );
        assert_eq!(
            d.action,
            Action::Stay {
                why: "work is the best pick".into()
            }
        );
    }

    #[test]
    fn a_maxed_login_outside_the_rule_is_left_at_once() {
        let c = config();
        let now = at("10:00");
        let d = run(
            &c,
            &now,
            Some("other"),
            Some(0),
            &[
                ("other", login(&now, (100, 2), (10, 100))),
                ("work", login(&now, (10, 2), (10, 100))),
            ],
        );
        assert!(
            matches!(&d.action, Action::Switch { to, why } if to == "work" && why == "other's 5 hours limit maxed")
        );
    }

    #[test]
    fn a_freed_reservation_is_spent_before_a_better_score() {
        let c = config();
        let now = at("20:00");
        let d = run(
            &c,
            &now,
            Some("perso"),
            None,
            &[
                ("work", login(&now, (0, 2), (95, 6))),
                ("perso", login(&now, (0, 2), (0, 100))),
            ],
        );
        assert_eq!(d.logins[0].name, "work");
        assert_eq!(switch_to(&d), Some("work"));
    }

    #[test]
    fn no_rule_and_no_logins_holds_and_a_huge_dwell_does_not_overflow() {
        let mut c = config();
        let a = c.auto.get_mut("claude").unwrap();
        a.rules.retain(|r| r.when == "workday");
        let now = at("20:00");
        assert_eq!(
            run(&c, &now, Some("work"), None, &[]).action,
            Action::Hold {
                why: "no rule is in force, and no logins are set for that case".into()
            }
        );
        let a = c.auto.get_mut("claude").unwrap();
        a.dwell = Span(u64::MAX);
        let now = at("10:00");
        let d = run(
            &c,
            &now,
            Some("perso"),
            Some(1),
            &[
                ("work", login(&now, (10, 2), (10, 100))),
                ("perso", login(&now, (10, 2), (10, 100))),
            ],
        );
        assert!(matches!(d.action, Action::Stay { .. }));
    }

    #[test]
    fn describe_reads_like_the_decision() {
        let c = config();
        let now = at("20:00");
        let d = run(
            &c,
            &now,
            Some("perso"),
            None,
            &[
                ("work", login(&now, (0, 2), (50, 30))),
                ("perso", login(&now, (10, 2), (10, 100))),
            ],
        );
        let mut out = Vec::new();
        describe(
            "Claude Code",
            &c.auto["claude"],
            Some("perso"),
            &d,
            &mut out,
        )
        .unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(
            text.starts_with("Claude Code: auto mode on, rule 2 (otherwise)\n"),
            "{text}"
        );
        assert!(text.contains("  1  perso"), "{text}");
        assert!(text.contains("in use\n"), "{text}");
        assert!(text.contains("  -  work"), "{text}");
        assert!(text.ends_with("  stay: perso is the best pick\n"), "{text}");
    }

    /// The quota balancer this replaces: IDC, then Axeo, then perso; the
    /// 5-hour window full at 93% for IDC and 88% for Axeo, every other at 97%.
    fn balancer() -> Config {
        toml_edit::de::from_str(
            r#"
[auto.claude]
enabled  = true
dwell    = "1m"
ceiling  = 97
ceilings = { IDC = 93, Axeo = 88 }
fallback = "perso"

[[auto.claude.rule]]
when  = "otherwise"
order = [["IDC"], ["Axeo"], ["perso"]]
"#,
        )
        .unwrap()
    }

    #[test]
    fn a_login_over_its_ceiling_is_left_at_once_for_the_next_tier() {
        let c = balancer();
        let now = at("10:00");
        let d = run(
            &c,
            &now,
            Some("IDC"),
            Some(0),
            &[
                ("IDC", login(&now, (93, 2), (40, 100))),
                ("Axeo", login(&now, (87, 2), (40, 100))),
                ("perso", login(&now, (0, 2), (0, 100))),
            ],
        );
        assert!(
            matches!(&d.action, Action::Switch { to, why } if to == "Axeo" && why == "IDC's 5 hours at 93%, full at 93%"),
            "{:?}",
            d.action
        );
        let axeo = d.logins.iter().find(|s| s.name == "Axeo").unwrap();
        assert!(axeo.usable, "87% is under Axeo's 88%");
    }

    #[test]
    fn the_cli_ceiling_holds_every_other_window() {
        let c = balancer();
        let now = at("10:00");
        let d = run(
            &c,
            &now,
            Some("IDC"),
            Some(0),
            &[
                ("IDC", login(&now, (10, 2), (97, 100))),
                ("Axeo", login(&now, (10, 2), (40, 100))),
            ],
        );
        assert!(
            matches!(&d.action, Action::Switch { to, why } if to == "Axeo" && why == "IDC's weekly at 97%, full at 97%")
        );
    }

    #[test]
    fn the_first_tier_comes_back_once_under_its_ceiling() {
        let c = balancer();
        let now = at("10:00");
        let f = [
            ("IDC", login(&now, (92, 2), (40, 100))),
            ("Axeo", login(&now, (10, 2), (40, 100))),
        ];
        assert!(
            matches!(
                run(&c, &now, Some("Axeo"), Some(0), &f).action,
                Action::Stay { .. }
            ),
            "the dwell first"
        );
        assert_eq!(
            switch_to(&run(&c, &now, Some("Axeo"), Some(1), &f)),
            Some("IDC")
        );
    }

    #[test]
    fn with_every_login_full_the_fallback_takes_the_cli_unless_it_is_maxed() {
        let c = balancer();
        let now = at("10:00");
        let full = |perso: u64| {
            run(
                &c,
                &now,
                Some("IDC"),
                Some(0),
                &[
                    ("IDC", login(&now, (95, 2), (40, 100))),
                    ("Axeo", login(&now, (90, 2), (40, 100))),
                    ("perso", login(&now, (perso, 2), (40, 100))),
                ],
            )
        };
        assert_eq!(
            full(98).action,
            Action::Switch {
                to: "perso".into(),
                why: "every login it may pick is full or has no figures; perso is the fallback"
                    .into()
            }
        );
        assert_eq!(
            full(100).action,
            Action::Stay {
                why: "every login it may pick is full or has no figures".into()
            }
        );
        let d = run(
            &c,
            &now,
            Some("perso"),
            Some(0),
            &[
                ("IDC", login(&now, (95, 2), (40, 100))),
                ("Axeo", login(&now, (90, 2), (40, 100))),
                ("perso", login(&now, (98, 2), (40, 100))),
            ],
        );
        assert_eq!(d.action, Action::Stay {
            why: "every login it may pick is full or has no figures; staying on perso, the fallback".into()
        });
    }

    #[test]
    fn headroom_is_counted_up_to_the_ceiling() {
        let mut c = balancer();
        let a = c.auto.get_mut("claude").unwrap();
        a.rules.clear();
        a.logins = vec!["IDC".into(), "Axeo".into()];
        a.ceilings.clear();
        a.ceiling = 50;
        let now = at("10:00");
        let d = run(
            &c,
            &now,
            None,
            None,
            &[
                ("IDC", login(&now, (0, 2), (40, 10))),
                ("Axeo", login(&now, (0, 2), (0, 100))),
            ],
        );
        let idc = d.logins.iter().find(|s| s.name == "IDC").unwrap();
        assert_eq!(
            idc.score,
            Some(1.0),
            "10 points to its 50% ceiling over 10 hours"
        );
    }

    #[test]
    fn the_fallback_waits_for_the_active_login_to_be_full() {
        let c = balancer();
        let now = at("10:00");
        let full_others = |active: (&'static str, Figures)| {
            run(
                &c,
                &now,
                Some(active.0),
                Some(0),
                &[
                    active,
                    ("Axeo", login(&now, (90, 2), (40, 100))),
                    ("perso", login(&now, (98, 2), (40, 100))),
                ],
            )
        };
        let d = full_others(("IDC", Figures::Missing("TokenGauge: 500".into())));
        assert!(
            matches!(&d.action, Action::Stay { why } if why.starts_with("every login it may pick is full")),
            "{:?}",
            d.action
        );
        let d = full_others(("spare", login(&now, (5, 2), (5, 100))));
        assert!(
            matches!(d.action, Action::Stay { .. }),
            "a login outside the rule with room is kept"
        );
        let d = run(
            &c,
            &now,
            None,
            None,
            &[
                ("IDC", login(&now, (95, 2), (40, 100))),
                ("perso", login(&now, (98, 2), (40, 100))),
            ],
        );
        assert_eq!(switch_to(&d), Some("perso"), "nothing signed in");
    }

    #[test]
    fn a_fallback_kept_for_its_schedule_is_not_used() {
        let mut c = config();
        c.auto.get_mut("claude").unwrap().fallback = Some("work".into());
        let now = at("20:00");
        let d = run(
            &c,
            &now,
            Some("perso"),
            None,
            &[
                ("work", login(&now, (0, 2), (50, 30))),
                ("perso", login(&now, (100, 2), (10, 100))),
            ],
        );
        assert_eq!(
            d.action,
            Action::Stay {
                why: "every login it may pick is full or has no figures".into()
            }
        );
    }

    #[test]
    fn a_login_with_one_window_keeps_the_cli_ceiling() {
        let c = balancer();
        let now = at("10:00");
        let weekly = |used| {
            Figures::Known(vec![Window {
                title: "Weekly".into(),
                minutes: Some(10080),
                used,
                resets_at: None,
                extra: false,
            }])
        };
        let d = run(
            &c,
            &now,
            Some("IDC"),
            Some(0),
            &[
                ("IDC", weekly(94)),
                ("Axeo", login(&now, (10, 2), (40, 100))),
            ],
        );
        assert!(
            matches!(d.action, Action::Stay { .. }),
            "94% is under the CLI's 97%, not IDC's 93%: {:?}",
            d.action
        );
    }
}
