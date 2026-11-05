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
//! A login's week is its first weekly window of the three fixed ones at the
//! time: an extra or later weekly window counts one model's use, and a week's
//! spend is every model's. A login whose weekly window moved to another slot
//! keeps the weeks that ended before the move; a week the move cut through
//! is not counted, since its percent covers the whole of it.

use std::collections::{HashMap, HashSet};
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
    /// The account digest the week's spend was matched by.
    pub account: Option<String>,
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
    /// Some of the login's usage in the week had no price, so `usd` is short.
    pub unpriced: bool,
}

/// One weekly window instance as the history file has it.
struct Span {
    provider: String,
    login: Option<String>,
    digest: Option<String>,
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
            digest: text(c, "accountDigest"),
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
            digest: text(t, "accountDigest"),
            window,
            opened: resets - WEEK_MINUTES as i64 * 60_000,
            resets,
            used: used.unwrap_or(0),
            open: true,
        });
    }
    out
}

/// The weeks of every login, newest first. A week's login is the account
/// digest TokenGauge recorded with it when `known` holds it, else the one
/// `digests` gives its provider and name: a name the store reused for another
/// account names the stored one.
pub fn weeks(
    history: &Value,
    export: &UsageExport,
    digests: &HashMap<(String, String), String>,
    known: &HashSet<String>,
    tz: &TimeZone,
    now: i64,
) -> Vec<Week> {
    let mut spans = spans(history, now);
    for s in &mut spans {
        let named = s
            .login
            .as_ref()
            .and_then(|name| digests.get(&(s.provider.clone(), name.clone())))
            .cloned();
        s.digest = s.digest.take().filter(|d| known.contains(d)).or(named);
    }
    let slot = |w: &str| FIXED.iter().position(|f| *f == w).unwrap_or(FIXED.len());
    let shadowed: Vec<bool> = spans
        .iter()
        .map(|s| {
            spans.iter().any(|t| {
                (&t.provider, &t.login, &t.digest) == (&s.provider, &s.login, &s.digest)
                    && slot(&t.window) < slot(&s.window)
                    && t.opened < s.resets
                    && s.opened < t.resets
            })
        })
        .collect();
    let mut shadowed = shadowed.into_iter();
    spans.retain(|_| !shadowed.next().unwrap_or(false));
    spans.sort_by(|a, b| {
        (&a.provider, &a.login, &a.digest, &a.window, a.opened).cmp(&(
            &b.provider,
            &b.login,
            &b.digest,
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
            .filter(|n| {
                (&n.provider, &n.login, &n.digest, &n.window)
                    == (&s.provider, &s.login, &s.digest, &s.window)
            })
            .map(|n| n.opened);
        let ends = if s.open {
            now
        } else {
            next.map_or(s.resets, |n| n.min(s.resets))
        };
        let digest = s.digest.as_ref();
        let (mut usd, mut unattributed, mut unpriced) = (0.0, 0.0, false);
        for (row, day) in export.rows.iter().zip(&days) {
            let Some((start, end)) = *day else {
                continue;
            };
            if row.provider != s.provider {
                continue;
            }
            let overlap = (end.min(ends) - start.max(s.opened)).max(0);
            if overlap == 0 {
                continue;
            }
            let Some(cost) = row.usd else {
                unpriced |=
                    row.tokens.total() > 0 && row.login.as_ref() == digest && digest.is_some();
                continue;
            };
            // Today's row holds what was spent so far, not a whole day.
            let length = end.min(now) - start;
            let share = cost.total() * overlap as f64 / length.max(overlap) as f64;
            match (&row.login, digest) {
                (None, _) => unattributed += share,
                (Some(l), Some(d)) if l == d => usd += share,
                _ => {}
            }
        }
        out.push(Week {
            provider: s.provider.clone(),
            login: s.login.clone(),
            account: s.digest.clone(),
            opened_at: s.opened,
            ends_at: ends,
            open: s.open,
            used: s.used,
            usd,
            usd_per_percent: (s.used > 0 && digest.is_some()).then(|| usd / s.used as f64),
            unattributed_usd: unattributed,
            partial: covers_from.is_none_or(|from| from > s.opened),
            unpriced,
        });
    }
    out.sort_by_key(|w| std::cmp::Reverse(w.opened_at));
    out
}

/// TokenGauge's window histories beside its snapshot, in the order to read
/// them: the fleet's view while fleet sync keeps one, then this machine's own.
pub fn history_paths(snapshot: &Path) -> [std::path::PathBuf; 2] {
    [
        snapshot.with_file_name("tokengauge-window-fleet.json"),
        snapshot.with_file_name("tokengauge-window-history.json"),
    ]
}

/// TokenGauge's window history, beside its snapshot: `Null` when there is
/// none yet, an error when it is there and cannot be read.
pub fn history(snapshot: &Path) -> anyhow::Result<Value> {
    for path in history_paths(snapshot) {
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                return serde_json::from_str(&text)
                    .map_err(|e| anyhow::anyhow!("{} does not parse: {e}", path.display()));
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(anyhow::anyhow!("{} could not be read: {e}", path.display())),
        }
    }
    Ok(Value::Null)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const DAY: i64 = 86_400_000;
    // 2026-10-05T00:00:00Z, a Monday.
    const MON: i64 = 1_791_158_400_000;

    fn iso(ms: i64) -> String {
        Timestamp::from_millisecond(ms).unwrap().to_string()
    }

    fn closed(login: &str, window: &str, minutes: u64, opened: i64, used: u64) -> Value {
        json!({"provider": "codex", "credential": login, "window": window, "windowMinutes": minutes,
               "openedAt": iso(opened), "resetsAt": iso(opened + minutes as i64 * 60_000),
               "used": used, "seenAt": iso(opened + DAY)})
    }

    fn export(rows: &[(&str, Option<&str>, f64)], covers_from: Option<&str>) -> UsageExport {
        serde_json::from_value(json!({
            "schemaVersion": 1, "generatedAt": 0, "currency": "USD", "coversFrom": covers_from,
            "rows": rows.iter().map(|(date, login, usd)| json!({
                "date": date, "provider": "codex", "model": "gpt-6", "login": login, "source": "cli",
                "tokens": {"input": 0, "output": 0, "cacheWrite5m": 0, "cacheWrite1h": 0, "cacheRead": 0},
                "usd": {"input": usd, "output": 0.0, "cacheWrite": 0.0, "cacheRead": 0.0}
            })).collect::<Vec<_>>()
        }))
        .unwrap()
    }

    fn digests() -> HashMap<(String, String), String> {
        HashMap::from([
            (("codex".into(), "work".into()), "d-work".into()),
            (("codex".into(), "perso".into()), "d-perso".into()),
        ])
    }

    fn known() -> HashSet<String> {
        digests().into_values().collect()
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn a_week_counts_its_logins_days_and_a_day_it_covers_in_part_in_proportion() {
        let opened = MON + DAY / 2;
        let history = json!({"closed": [closed("work", "primary", WEEK_MINUTES, opened, 40)]});
        let rows = export(
            &[
                ("2026-10-04", Some("d-work"), 99.0),
                ("2026-10-05", Some("d-work"), 10.0),
                ("2026-10-06", Some("d-work"), 30.0),
                ("2026-10-06", Some("d-perso"), 500.0),
                ("2026-10-07", None, 8.0),
                ("2026-10-12", Some("d-work"), 20.0),
                ("2026-10-13", Some("d-work"), 99.0),
            ],
            Some("2026-09-01"),
        );
        let weeks = weeks(
            &history,
            &rows,
            &digests(),
            &known(),
            &TimeZone::UTC,
            MON + 30 * DAY,
        );
        assert_eq!(weeks.len(), 1);
        let w = &weeks[0];
        assert!(close(w.usd, 5.0 + 30.0 + 10.0), "{}", w.usd);
        assert!(close(w.unattributed_usd, 8.0));
        assert!(close(w.usd_per_percent.unwrap(), 45.0 / 40.0));
        assert_eq!(
            (w.opened_at, w.ends_at, w.used),
            (opened, opened + 7 * DAY, 40)
        );
        assert!(!w.open && !w.partial);
    }

    #[test]
    fn a_week_reset_early_ends_where_the_next_opened_and_the_open_week_runs_to_now() {
        let next = MON + 3 * DAY;
        let now = next + DAY;
        let history = json!({
            "closed": [closed("work", "primary", WEEK_MINUTES, MON, 70)],
            "trails": [{"provider": "codex", "credential": "work", "window": "primary",
                        "windowMinutes": WEEK_MINUTES, "resetsAt": iso(next + 7 * DAY),
                        "samples": [[next, 0], [next + DAY / 2, 12]]}]
        });
        let rows = export(
            &[
                ("2026-10-05", Some("d-work"), 7.0),
                ("2026-10-08", Some("d-work"), 6.0),
                ("2026-10-09", Some("d-work"), 100.0),
            ],
            Some("2026-09-01"),
        );
        let weeks = weeks(&history, &rows, &digests(), &known(), &TimeZone::UTC, now);
        assert_eq!(weeks.len(), 2);
        let (open, shut) = (&weeks[0], &weeks[1]);
        assert!(open.open && open.used == 12 && open.ends_at == now);
        assert!(close(open.usd, 6.0));
        assert!(!shut.open && shut.ends_at == next);
        assert!(close(shut.usd, 7.0));
    }

    #[test]
    fn a_login_s_week_is_its_first_weekly_fixed_window() {
        let history = json!({"closed": [
            closed("work", "primary", 300, MON, 90),
            closed("work", "secondary", WEEK_MINUTES, MON, 50),
            closed("work", "tertiary", WEEK_MINUTES, MON, 80),
            {"provider": "codex", "credential": "work", "window": "fable", "windowMinutes": WEEK_MINUTES,
             "openedAt": iso(MON), "resetsAt": iso(MON + 7 * DAY), "used": 5, "seenAt": iso(MON)}
        ]});
        let weeks = weeks(
            &history,
            &export(&[], None),
            &digests(),
            &known(),
            &TimeZone::UTC,
            MON + 30 * DAY,
        );
        assert_eq!(weeks.iter().map(|w| w.used).collect::<Vec<_>>(), [50]);
    }

    #[test]
    fn a_login_whose_weekly_window_moved_keeps_its_earlier_weeks() {
        let later = MON + 7 * DAY;
        let history = json!({"closed": [
            closed("work", "secondary", WEEK_MINUTES, MON, 50),
            closed("work", "primary", 300, MON, 90),
            closed("work", "primary", WEEK_MINUTES, later, 30),
            closed("work", "secondary", WEEK_MINUTES, later, 70)
        ]});
        let weeks = weeks(
            &history,
            &export(&[], None),
            &digests(),
            &known(),
            &TimeZone::UTC,
            MON + 30 * DAY,
        );
        assert_eq!(weeks.iter().map(|w| w.used).collect::<Vec<_>>(), [30, 50]);
    }

    #[test]
    fn a_week_counts_the_account_it_was_recorded_for_when_the_name_moved_on() {
        let mut old = closed("work", "primary", WEEK_MINUTES, MON, 40);
        old["accountDigest"] = json!("d-gone");
        let mut stranger = closed("perso", "primary", WEEK_MINUTES, MON, 40);
        stranger["accountDigest"] = json!("d-unknown");
        let history = json!({"closed": [old, stranger]});
        let rows = export(
            &[
                ("2026-10-06", Some("d-gone"), 8.0),
                ("2026-10-06", Some("d-work"), 100.0),
                ("2026-10-06", Some("d-perso"), 4.0),
            ],
            None,
        );
        let mut known = known();
        known.insert("d-gone".into());
        let weeks = weeks(
            &history,
            &rows,
            &digests(),
            &known,
            &TimeZone::UTC,
            MON + 30 * DAY,
        );
        let of = |login: &str| {
            weeks
                .iter()
                .find(|w| w.login.as_deref() == Some(login))
                .unwrap()
                .usd
        };
        assert!(close(of("work"), 8.0));
        assert!(close(of("perso"), 4.0));
    }

    #[test]
    fn today_counts_as_what_was_spent_so_far() {
        let now = MON + 3 * DAY + DAY / 4;
        let history = json!({"trails": [{"provider": "codex", "credential": "work", "window": "primary",
            "windowMinutes": WEEK_MINUTES, "resetsAt": iso(MON + 7 * DAY), "samples": [[MON, 0], [now, 10]]}]});
        let rows = export(&[("2026-10-08", Some("d-work"), 12.0)], Some("2026-09-01"));
        let weeks = weeks(&history, &rows, &digests(), &known(), &TimeZone::UTC, now);
        assert!(close(weeks[0].usd, 12.0), "{}", weeks[0].usd);
    }

    #[test]
    fn a_week_with_unpriced_use_of_its_login_says_so() {
        let history = json!({"closed": [closed("work", "primary", WEEK_MINUTES, MON, 40)]});
        let mut rows = export(
            &[
                ("2026-10-06", Some("d-work"), 1.0),
                ("2026-10-07", Some("d-perso"), 1.0),
            ],
            None,
        );
        rows.rows[1].usd = None;
        rows.rows[1].tokens.output = 5;
        let priced = weeks(
            &history,
            &rows,
            &digests(),
            &known(),
            &TimeZone::UTC,
            MON + 30 * DAY,
        );
        assert!(!priced[0].unpriced);
        rows.rows[0].usd = None;
        rows.rows[0].tokens.output = 5;
        let unpriced = weeks(
            &history,
            &rows,
            &digests(),
            &known(),
            &TimeZone::UTC,
            MON + 30 * DAY,
        );
        assert!(unpriced[0].unpriced);
    }

    #[test]
    fn a_history_that_cannot_be_read_is_an_error_and_a_missing_one_is_none() {
        let dir = tempfile::tempdir().unwrap();
        let snapshot = dir.path().join("tokengauge-usage.json");
        assert_eq!(history(&snapshot).unwrap(), Value::Null);
        std::fs::write(dir.path().join("tokengauge-window-history.json"), "{nope").unwrap();
        assert!(
            history(&snapshot)
                .unwrap_err()
                .to_string()
                .contains("does not parse")
        );
    }

    #[test]
    fn a_week_before_the_record_starts_or_of_an_unknown_login_says_so() {
        let history = json!({"closed": [
            closed("work", "primary", WEEK_MINUTES, MON, 40),
            closed("gone", "primary", WEEK_MINUTES, MON, 40)
        ]});
        let weeks = weeks(
            &history,
            &export(&[], Some("2026-10-07")),
            &digests(),
            &known(),
            &TimeZone::UTC,
            MON + 30 * DAY,
        );
        assert!(weeks.iter().all(|w| w.partial));
        let gone = weeks
            .iter()
            .find(|w| w.login.as_deref() == Some("gone"))
            .unwrap();
        assert_eq!(gone.usd_per_percent, None);
    }
}
