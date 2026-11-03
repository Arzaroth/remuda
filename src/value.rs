//! The Usage tab's Value view: what a week of each plan bought, in API
//! dollars per percent of its weekly window.
//!
//! The weeks are TokenGauge's window history (`closed`, and the trails of the
//! weeks still open), the dollars its usage export, both by login. A week's
//! spend is every row of its login on a day the week covers, a day it covers
//! only in part counted in proportion. Spend no record attributes (before
//! remuda's ledger, from a machine without remuda) is shown beside it rather
//! than guessed at, since it may be this login's.
//!
//! A login's week is its first weekly window of the three fixed ones: an
//! extra or later weekly window counts one model's use, and a week's spend is
//! every model's.

use std::collections::HashMap;
use std::path::Path;

use jiff::Timestamp;
use jiff::civil::Date;
use jiff::tz::TimeZone;
use selvedge::usage::export::UsageExport;
use serde::Serialize;
use serde_json::Value;

const WEEK_MINUTES: u64 = 10_080;
const FIXED: [&str; 3] = ["primary", "secondary", "tertiary"];

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Week {
    pub provider: String,
    pub login: Option<String>,
    pub opened_at: i64,
    /// When it reset, or now for a week still open.
    pub ends_at: i64,
    pub open: bool,
    /// Percent of the week's allowance used, at the last reading.
    pub used: u64,
    /// API dollars this login spent in the week.
    pub usd: f64,
    /// `usd / used`; `None` below 1%.
    pub usd_per_percent: Option<f64>,
    /// The provider's spend in the week that no record attributes to a login.
    pub unattributed_usd: f64,
    /// The usage record starts after the week opened.
    pub partial: bool,
}

/// One weekly window instance as the history file has it.
struct Span {
    provider: String,
    login: Option<String>,
    window: String,
    opened: i64,
    resets: i64,
    used: u64,
    open: bool,
}

fn ms(v: Option<&Value>) -> Option<i64> {
    Some(v?.as_str()?.parse::<Timestamp>().ok()?.as_millisecond())
}

fn spans(history: &Value, now: i64) -> Vec<Span> {
    let text = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).map(str::to_owned);
    let weekly = |v: &Value| {
        v.get("windowMinutes").and_then(Value::as_u64) == Some(WEEK_MINUTES)
            && v.get("window")
                .and_then(Value::as_str)
                .is_some_and(|w| FIXED.contains(&w))
    };
    let list = |k: &str| {
        history
            .get(k)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|v| weekly(v))
    };
    let mut out = Vec::new();
    for c in list("closed") {
        let (Some(provider), Some(window), Some(opened), Some(resets), Some(used)) = (
            text(c, "provider"),
            text(c, "window"),
            ms(c.get("openedAt")),
            ms(c.get("resetsAt")),
            c.get("used").and_then(Value::as_u64),
        ) else {
            continue;
        };
        out.push(Span {
            provider,
            login: text(c, "credential"),
            window,
            opened,
            resets,
            used,
            open: false,
        });
    }
    for t in list("trails") {
        let (Some(provider), Some(window), Some(resets)) = (
            text(t, "provider"),
            text(t, "window"),
            ms(t.get("resetsAt")),
        ) else {
            continue;
        };
        let used = t
            .get("samples")
            .and_then(Value::as_array)
            .and_then(|s| s.last()?.get(1)?.as_u64());
        if resets <= now {
            continue;
        }
        out.push(Span {
            provider,
            login: text(t, "credential"),
            window,
            opened: resets - WEEK_MINUTES as i64 * 60_000,
            resets,
            used: used.unwrap_or(0),
            open: true,
        });
    }
    out
}

/// The weeks of every login, newest first. `digests` names each login's
/// digest by provider and name, as the export's rows carry it.
pub fn weeks(
    history: &Value,
    export: &UsageExport,
    digests: &HashMap<(String, String), String>,
    tz: &TimeZone,
    now: i64,
) -> Vec<Week> {
    let mut spans = spans(history, now);
    let slot = |w: &str| FIXED.iter().position(|f| *f == w).unwrap_or(FIXED.len());
    let mut first: HashMap<(String, Option<String>), usize> = HashMap::new();
    for s in &spans {
        let at = first
            .entry((s.provider.clone(), s.login.clone()))
            .or_insert(FIXED.len());
        *at = (*at).min(slot(&s.window));
    }
    spans.retain(|s| first.get(&(s.provider.clone(), s.login.clone())) == Some(&slot(&s.window)));
    spans.sort_by(|a, b| {
        (&a.provider, &a.login, &a.window, a.opened).cmp(&(
            &b.provider,
            &b.login,
            &b.window,
            b.opened,
        ))
    });
    let day_ms = |date: &str| -> Option<(i64, i64)> {
        let day = date.parse::<Date>().ok()?;
        let start = day.to_zoned(tz.clone()).ok()?;
        let end = day.tomorrow().ok()?.to_zoned(tz.clone()).ok()?;
        Some((
            start.timestamp().as_millisecond(),
            end.timestamp().as_millisecond(),
        ))
    };
    let days: Vec<Option<(i64, i64)>> = export.rows.iter().map(|r| day_ms(&r.date)).collect();
    let covers_from = export
        .covers_from
        .as_deref()
        .and_then(day_ms)
        .map(|(s, _)| s);
    let mut out = Vec::new();
    for (i, s) in spans.iter().enumerate() {
        let next = spans
            .get(i + 1)
            .filter(|n| (&n.provider, &n.login, &n.window) == (&s.provider, &s.login, &s.window))
            .map(|n| n.opened);
        let ends = if s.open {
            now
        } else {
            next.map_or(s.resets, |n| n.min(s.resets))
        };
        let digest = s
            .login
            .as_ref()
            .and_then(|name| digests.get(&(s.provider.clone(), name.clone())));
        let (mut usd, mut unattributed) = (0.0, 0.0);
        for (row, day) in export.rows.iter().zip(&days) {
            let (Some((start, end)), Some(cost)) = (day, row.usd) else {
                continue;
            };
            if row.provider != s.provider {
                continue;
            }
            let overlap = (end.min(&ends) - start.max(&s.opened)).max(0);
            if overlap == 0 {
                continue;
            }
            let share = cost.total() * overlap as f64 / (end - start) as f64;
            match (&row.login, digest) {
                (None, _) => unattributed += share,
                (Some(l), Some(d)) if l == d => usd += share,
                _ => {}
            }
        }
        out.push(Week {
            provider: s.provider.clone(),
            login: s.login.clone(),
            opened_at: s.opened,
            ends_at: ends,
            open: s.open,
            used: s.used,
            usd,
            usd_per_percent: (s.used > 0 && digest.is_some()).then(|| usd / s.used as f64),
            unattributed_usd: unattributed,
            partial: covers_from.is_none_or(|from| from > s.opened),
        });
    }
    out.sort_by_key(|w| std::cmp::Reverse(w.opened_at));
    out
}

/// TokenGauge's window history, beside its snapshot.
pub fn history(snapshot: &Path) -> Value {
    std::fs::read_to_string(snapshot.with_file_name("tokengauge-window-history.json"))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or(Value::Null)
}
