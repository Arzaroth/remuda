//! Auto mode's choice: which login a CLI should be on now, and whether to
//! move it there. Pure: the clock, the figures and the active login are given.

use std::collections::BTreeMap;

use jiff::civil::{Date, Weekday};
use jiff::{Timestamp, Zoned};
use serde::Serialize;
use serde_json::Value;

use crate::config::{Auto, Day, OTHERWISE, Rule, Schedule};

const HOUR_MS: f64 = 3_600_000.0;

#[derive(Debug, Clone, PartialEq)]
pub struct Window {
    pub minutes: Option<u64>,
    pub used: u64,
    pub resets_at: Option<Timestamp>,
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

fn longest(windows: &[Window]) -> Option<&Window> {
    windows
        .iter()
        .max_by_key(|w| (w.minutes.unwrap_or(0), w.resets_at))
}

fn window_name(w: &Window) -> String {
    match w.minutes {
        Some(300) => "5-hour window".into(),
        Some(1440) => "daily window".into(),
        Some(10080) => "weekly window".into(),
        Some(43200) => "monthly window".into(),
        Some(m) if m % 1440 == 0 => format!("{}-day window", m / 1440),
        Some(m) if m % 60 == 0 => format!("{}-hour window", m / 60),
        Some(m) => format!("{m}-minute window"),
        None => "usage window".into(),
    }
}

/// Headroom left in the longest window per hour until it resets: what would
/// be lost at the reset if the login sat idle.
fn score(windows: &[Window], now: Timestamp) -> Option<f64> {
    let w = longest(windows)?;
    let hours = match w.resets_at {
        Some(at) => (at.as_millisecond() - now.as_millisecond()) as f64 / HOUR_MS,
        None => w.minutes.map_or(168.0, |m| m as f64 / 60.0),
    };
    Some((100 - w.used.min(100)) as f64 / hours.max(1.0))
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
    for (login, schedule) in &reserve {
        let Some(s) = input.schedules.get(schedule) else {
            continue;
        };
        if contains(s, input.now) {
            continue;
        }
        let resets = match input.figures.get(login) {
            Some(Figures::Known(w)) => longest(w).and_then(|w| w.resets_at),
            _ => None,
        };
        let starts = next_start(s, input.now);
        match (resets, &starts) {
            (Some(r), Some(st)) if r <= st.timestamp() => {
                for t in tiers.iter_mut() {
                    t.retain(|n| n != login);
                }
                if tiers.is_empty() {
                    tiers.push(Vec::new());
                }
                tiers[0].insert(0, login.clone());
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
            match input.figures.get(name) {
                Some(Figures::Known(w)) if !w.is_empty() => {
                    seen.score = score(w, now);
                    match w.iter().find(|w| w.used >= 100) {
                        Some(full) => {
                            seen.maxed = true;
                            seen.note = format!("{} maxed", window_name(full));
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

    let action = act(input, &logins);
    Decision {
        rule,
        logins,
        action,
    }
}

fn act(input: &Input, logins: &[Seen]) -> Action {
    let best = logins.iter().filter(|s| s.usable).min_by(|a, b| {
        a.tier
            .cmp(&b.tier)
            .then(b.score.unwrap_or(0.0).total_cmp(&a.score.unwrap_or(0.0)))
    });
    let Some(best) = best else {
        return Action::Stay {
            why: "every login it may pick is maxed or has no figures".into(),
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
    if let Some(c) = current
        && c.maxed
    {
        return Action::Switch {
            to: best.name.clone(),
            why: format!("{active}'s {}", c.note),
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
    let dwell_ms = input.auto.dwell.0 as i64 * 1000;
    if let Some(at) = input.switched_at {
        let until = at.as_millisecond() + dwell_ms;
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
) -> Result<BTreeMap<String, Figures>, String> {
    let usage = usage.ok_or("TokenGauge has written no figures")?;
    let p = usage
        .pointer(&format!("/providers/{provider}"))
        .filter(|p| !p.is_null())
        .ok_or_else(|| format!("TokenGauge has no figures for {provider}"))?;
    let updated = usage.get("updatedAt").and_then(Value::as_i64);
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

/// Why a switch has to wait for the CLI's sessions to end, if it does: Codex
/// never takes up a new login while it runs, and a refresh it has in flight
/// writes the old account's tokens over the new one.
pub fn sessions_open(provider: &str) -> Option<String> {
    if provider != "codex" {
        return None;
    }
    let me = std::process::id().to_string();
    let open = std::fs::read_dir("/proc")
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_name().to_str() != Some(me.as_str()))
        .filter(|e| {
            let dir = e.path();
            let comm = std::fs::read_to_string(dir.join("comm")).unwrap_or_default();
            let cmdline = std::fs::read(dir.join("cmdline")).unwrap_or_default();
            comm.trim() == "codex"
                || cmdline.split(|b| *b == 0).take(2).any(|arg| {
                    let name = arg.rsplit(|b| *b == b'/').next().unwrap_or_default();
                    name == b"codex" || name == b"codex.js"
                })
        })
        .count();
    (open > 0).then(|| "waiting for every Codex session to end".to_owned())
}

pub struct Context<'a> {
    pub schedules: &'a BTreeMap<String, Schedule>,
    pub usage: Option<&'a Value>,
    pub switched_at: Option<i64>,
    pub now: &'a Zoned,
}

/// The decision for one CLI from its live login and TokenGauge's snapshot.
/// `busy` holds a switch back, saying why.
pub fn assess(
    auto: &Auto,
    provider: &str,
    live: &crate::ops::LiveState,
    cx: &Context,
    busy: Option<String>,
) -> Decision {
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
    let figures = match figures(cx.usage, provider, active, switched_at) {
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
    if let (Action::Switch { to, why }, Some(busy)) = (&decision.action, busy) {
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
                minutes: Some(300),
                used: five.0,
                resets_at: t(five.1),
            },
            Window {
                minutes: Some(10080),
                used: week.0,
                resets_at: t(week.1),
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
            matches!(&d.action, Action::Switch { why, .. } if why == "work's 5-hour window maxed")
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
                why: "every login it may pick is maxed or has no figures".into()
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
        assert_eq!(
            figures(Some(&usage), "claude", None, ms(2000)),
            Err("waiting for TokenGauge to fetch after the last switch".into())
        );
        assert_eq!(
            figures(None, "claude", None, None),
            Err("TokenGauge has written no figures".into())
        );
        assert_eq!(
            figures(Some(&usage), "codex", None, None),
            Err("TokenGauge has no figures for codex".into())
        );
        let f = figures(Some(&usage), "claude", Some("old"), ms(500)).unwrap();
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
                minutes: Some(300),
                used: 5,
                resets_at: None
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
        let why =
            |live: LiveState| match assess(&c.auto["claude"], "claude", &live, &cx, None).action {
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

    #[test]
    fn a_busy_cli_turns_a_switch_into_a_hold() {
        let c = config();
        let now = at("10:00");
        let usage = serde_json::json!({"updatedAt": now.timestamp().as_millisecond(), "providers": {"claude": {
            "windows": [], "accounts": {
                "work": {"windows": [{"minutes": 300, "usedPercent": 1, "resetsAt": null}]},
                "perso": {"windows": [{"minutes": 300, "usedPercent": 1, "resetsAt": null}]}}}}});
        let cx = Context {
            schedules: &c.schedules,
            usage: Some(&usage),
            switched_at: None,
            now: &now,
        };
        let live = crate::ops::LiveState::Stored {
            name: "perso".into(),
            synced: true,
        };
        let d = assess(
            &c.auto["claude"],
            "claude",
            &live,
            &cx,
            Some("waiting for every Codex session to end".into()),
        );
        assert_eq!(d.action, Action::Hold {
            why: "would switch to work (work comes before perso), waiting for every Codex session to end".into()
        });
    }
}
