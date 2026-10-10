use selvedge::plans::{Share, pool, whole_percent};
use serde_json::{Map, Value, json};

/// What the page draws from TokenGauge's figures, per CLI: the usage each
/// stored login shows, and every window added up across them, weighted and
/// absolute. Worked out here so the page only draws, and so the sum is the
/// one TokenGauge's panels print (`selvedge::plans`).
///
/// `state` is the page's state as `/api/state` builds it: `credentials`,
/// `usage` (from [`crate::gauge::read`]) and `health.switchedAt`.
pub fn shown(state: &Value) -> Value {
    let mut out = Map::new();
    let providers = state.get("providers").and_then(Value::as_array);
    for id in providers
        .into_iter()
        .flatten()
        .filter_map(|p| p.get("id").and_then(Value::as_str))
    {
        let creds: Vec<&Value> = state
            .get("credentials")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|c| c.get("provider").and_then(Value::as_str) == Some(id))
            .collect();
        let usage = provider_usage(state, id);
        let accounts: Map<String, Value> = creds
            .iter()
            .filter_map(|c| {
                let name = c.get("name")?.as_str()?;
                Some((name.to_owned(), usage_of(c, usage.as_ref())?))
            })
            .collect();
        out.insert(
            id.to_owned(),
            json!({
                "usage": usage,
                "accounts": accounts,
                "weighable": weighable(&creds, usage.as_ref()),
                "weighted": combine(&creds, usage.as_ref(), true),
                "absolute": combine(&creds, usage.as_ref(), false),
            }),
        );
    }
    Value::Object(out)
}

fn accounts(usage: &Value) -> Option<&Map<String, Value>> {
    usage.get("accounts").and_then(Value::as_object)
}

fn empty(flag: &str) -> Value {
    json!({ flag: true, "windows": [] })
}

/// An unnamed snapshot older than the last switch describes the login that
/// was switched away from.
fn behind(state: &Value, id: &str) -> bool {
    let Some(updated) = state.pointer("/usage/updatedAt").and_then(Value::as_i64) else {
        return false;
    };
    let named = state
        .pointer(&format!("/usage/providers/{id}"))
        .and_then(accounts)
        .is_some_and(|a| !a.is_empty());
    if named {
        return false;
    }
    let switched = state
        .pointer(&format!("/health/switchedAt/{id}"))
        .and_then(Value::as_i64)
        .unwrap_or(0);
    updated < switched
}

fn provider_usage(state: &Value, id: &str) -> Option<Value> {
    if behind(state, id) {
        return Some(empty("behind"));
    }
    state
        .pointer(&format!("/usage/providers/{id}"))
        .filter(|u| !u.is_null())
        .cloned()
}

/// A login's own figures when TokenGauge names it. An unnamed snapshot is the
/// active login's only, and once TokenGauge names some logins, one it does
/// not name has none.
fn usage_of(c: &Value, usage: Option<&Value>) -> Option<Value> {
    let usage = usage?;
    let name = c.get("name").and_then(Value::as_str).unwrap_or_default();
    let named = accounts(usage);
    if let Some(own) = named.and_then(|a| a.get(name)) {
        return Some(own.clone());
    }
    let named = named.is_some_and(|a| !a.is_empty());
    if c.get("active").and_then(Value::as_bool) != Some(true) {
        return Some(empty(if named { "missing" } else { "passive" }));
    }
    let windows = usage
        .get("windows")
        .and_then(Value::as_array)
        .is_some_and(|w| !w.is_empty());
    let error = usage.get("error").is_some_and(|e| !e.is_null());
    Some(if named && !windows && !error {
        empty("missing")
    } else {
        usage.clone()
    })
}

struct Reporting<'a> {
    c: &'a Value,
    windows: Vec<Value>,
    weight: Option<f64>,
}

fn reporting<'a>(creds: &[&'a Value], usage: Option<&Value>) -> Vec<Reporting<'a>> {
    creds
        .iter()
        .map(|c| {
            let u = usage_of(c, usage);
            let failed = u
                .as_ref()
                .and_then(|u| u.get("error"))
                .is_some_and(|e| !e.is_null());
            let windows = match &u {
                Some(u) if !failed => u
                    .get("windows")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default(),
                _ => Vec::new(),
            };
            let weight = u
                .as_ref()
                .and_then(|u| u.get("planWeight"))
                .and_then(Value::as_f64)
                .filter(|w| w.is_finite() && *w > 0.0);
            Reporting { c, windows, weight }
        })
        .collect()
}

fn weighable(creds: &[&Value], usage: Option<&Value>) -> bool {
    reporting(creds, usage)
        .iter()
        .any(|r| r.weight.is_some() && !r.windows.is_empty())
}

fn text<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or_default()
}

/// Every window the logins report, added up over the logins that report it.
/// Weighted, a login with no known plan weight is left out and named; with
/// no weight at all, or absolute, every login counts once.
fn combine(creds: &[&Value], usage: Option<&Value>, weighted: bool) -> Vec<Value> {
    let shown = reporting(creds, usage);
    let mut titles: Vec<&str> = Vec::new();
    for w in shown.iter().flat_map(|s| &s.windows) {
        let title = text(w, "title");
        if !titles.contains(&title) {
            titles.push(title);
        }
    }
    titles
        .into_iter()
        .filter_map(|title| {
            let in_window: Vec<(&Reporting, &Value)> = shown
                .iter()
                .filter_map(|s| Some((s, s.windows.iter().find(|w| text(w, "title") == title)?)))
                .collect();
            let by_weight = weighted && in_window.iter().any(|(s, _)| s.weight.is_some());
            let left_out: Vec<&str> = if by_weight {
                in_window
                    .iter()
                    .filter(|(s, _)| s.weight.is_none())
                    .map(|(s, _)| text(s.c, "name"))
                    .collect()
            } else {
                Vec::new()
            };
            let parts: Vec<(&Reporting, &Value, f64)> = in_window
                .iter()
                .filter(|(s, _)| !by_weight || s.weight.is_some())
                .map(|(s, w)| {
                    (
                        *s,
                        *w,
                        if by_weight {
                            s.weight.unwrap_or(1.0)
                        } else {
                            1.0
                        },
                    )
                })
                .collect();
            let pooled = pool(
                &parts
                    .iter()
                    .map(|(_, w, weight)| Share {
                        used: w.get("usedPercent").and_then(Value::as_f64).unwrap_or(0.0),
                        weight: *weight,
                    })
                    .collect::<Vec<_>>(),
            )?;
            Some(json!({
                "title": title,
                "weighted": by_weight,
                "used": whole_percent(pooled.used),
                "of": whole_percent(pooled.capacity),
                "pooled": pooled.fraction * 100.0,
                "parts": parts
                    .iter()
                    .map(|(s, w, weight)| json!({
                        "name": text(s.c, "name"),
                        "active": s.c.get("active").and_then(Value::as_bool).unwrap_or(false),
                        "window": w,
                        "weight": weight,
                    }))
                    .collect::<Vec<_>>(),
                "widths": pooled.segments.iter().map(|s| s.width).collect::<Vec<_>>(),
                "leftOut": left_out,
            }))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cred(name: &str, active: bool) -> Value {
        json!({"provider": "claude", "name": name, "active": active})
    }

    fn win(used: u64) -> Value {
        json!([{"title": "5 hours", "usedPercent": used, "resetsAt": null}])
    }

    fn of(creds: &[Value], usage: Value, weighted: bool) -> Vec<Value> {
        let refs: Vec<&Value> = creds.iter().collect();
        combine(&refs, Some(&usage), weighted)
    }

    #[test]
    fn a_login_takes_its_own_figures_when_tokengauge_names_it() {
        let usage = json!({"windows": win(7), "accounts": {"work": {"windows": win(42)}}});
        let work = usage_of(&cred("work", false), Some(&usage)).unwrap();
        assert_eq!(work["windows"][0]["usedPercent"], 42);
        assert_eq!(
            usage_of(&cred("perso", false), Some(&usage)),
            Some(json!({"missing": true, "windows": []}))
        );
    }

    #[test]
    fn the_active_login_falls_back_to_the_provider_figures_only_when_there_are_some() {
        let usage = json!({"windows": win(7), "accounts": {"work": {"windows": win(42)}}});
        assert_eq!(usage_of(&cred("perso", true), Some(&usage)), Some(usage));
        let empty = json!({"windows": [], "accounts": {"work": {"windows": win(42)}}});
        assert_eq!(
            usage_of(&cred("perso", true), Some(&empty)),
            Some(json!({"missing": true, "windows": []}))
        );
    }

    #[test]
    fn an_old_snapshot_is_the_active_logins_only() {
        let usage = json!({"windows": win(7), "accounts": {}});
        assert_eq!(
            usage_of(&cred("work", true), Some(&usage)),
            Some(usage.clone())
        );
        assert_eq!(
            usage_of(&cred("perso", false), Some(&usage)),
            Some(json!({"passive": true, "windows": []}))
        );
        assert_eq!(usage_of(&cred("perso", false), None), None);
    }

    #[test]
    fn an_unnamed_snapshot_older_than_the_last_switch_is_behind() {
        let state = |updated: i64, switched: i64, accounts: Value| {
            json!({
                "health": {"switchedAt": {"claude": switched}},
                "usage": {"updatedAt": updated, "providers": {"claude": {"windows": [], "accounts": accounts}}},
            })
        };
        assert!(behind(&state(1, 2, json!({})), "claude"));
        assert!(!behind(&state(3, 2, json!({})), "claude"));
        assert!(!behind(
            &state(1, 2, json!({"work": {"windows": []}})),
            "claude"
        ));
    }

    #[test]
    fn a_window_is_summed_over_the_logins_that_report_it() {
        let usage = json!({"windows": [], "accounts": {
            "work": {"windows": [
                {"title": "Weekly", "usedPercent": 81, "resetsAt": "2026-10-12T00:00:00Z"},
                {"title": "5 hours", "usedPercent": 64, "resetsAt": null},
            ]},
            "perso": {"windows": [{"title": "Weekly", "usedPercent": 46, "resetsAt": "2026-10-10T00:00:00Z"}]},
            "spare": {"windows": [], "credentialState": "expired"},
            "gone": {"windows": win(99), "error": "401"},
        }});
        let creds = [
            cred("work", true),
            cred("perso", false),
            cred("spare", false),
            cred("gone", false),
        ];
        let combined = of(&creds, usage, false);
        let (weekly, hours) = (&combined[0], &combined[1]);
        assert_eq!(
            (&weekly["title"], &weekly["used"], &weekly["of"]),
            (&json!("Weekly"), &json!(127), &json!(200))
        );
        let parts: Vec<(&Value, &Value)> = weekly["parts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| (&p["name"], &p["active"]))
            .collect();
        assert_eq!(
            parts,
            [
                (&json!("work"), &json!(true)),
                (&json!("perso"), &json!(false))
            ]
        );
        assert_eq!((&hours["used"], &hours["of"]), (&json!(64), &json!(100)));
    }

    #[test]
    fn nothing_combines_without_figures() {
        let work = [cred("work", true)];
        let refs: Vec<&Value> = work.iter().collect();
        assert!(combine(&refs, None, true).is_empty());
        assert!(of(&work, json!({"behind": true, "windows": []}), true).is_empty());
    }

    fn weighed() -> Value {
        json!({"windows": [], "accounts": {
            "perso": {"windows": [{"title": "Session", "usedPercent": 31, "resetsAt": null}], "planWeight": 20},
            "work": {"windows": [{"title": "Session", "usedPercent": 100, "resetsAt": null}], "planWeight": 1},
            "odd": {"windows": [{"title": "Session", "usedPercent": 50, "resetsAt": null}]},
        }})
    }

    fn three() -> [Value; 3] {
        [cred("perso", true), cred("work", false), cred("odd", false)]
    }

    #[test]
    fn logins_weigh_by_plan_in_units_of_the_largest() {
        let session = &of(&three(), weighed(), true)[0];
        assert_eq!(
            (&session["used"], &session["of"], &session["leftOut"]),
            (&json!(36), &json!(105), &json!(["odd"]))
        );
        let pooled = session["pooled"].as_f64().unwrap();
        assert!((pooled - (31.0 * 20.0 + 100.0) / 21.0).abs() < 1e-9);
        let widths = session["widths"].as_array().unwrap();
        assert!((widths[0].as_f64().unwrap() - 0.9).abs() < 1e-9);
        let refs: Vec<Value> = three().to_vec();
        let refs: Vec<&Value> = refs.iter().collect();
        assert!(weighable(&refs, Some(&weighed())));
    }

    #[test]
    fn absolute_or_unweighed_logins_count_once() {
        let session = &of(&three(), weighed(), false)[0];
        assert_eq!(
            (&session["used"], &session["of"], &session["leftOut"]),
            (&json!(181), &json!(300), &json!([]))
        );
        let unweighed = json!({"windows": [], "accounts": {"odd": weighed()["accounts"]["odd"]}});
        let odd = [cred("odd", false)];
        let refs: Vec<&Value> = odd.iter().collect();
        assert!(!weighable(&refs, Some(&unweighed)));
        let session = &of(&odd, unweighed, true)[0];
        assert_eq!(
            (&session["used"], &session["of"]),
            (&json!(50), &json!(100))
        );
    }

    #[test]
    fn a_window_only_unweighed_logins_report_is_counted_once() {
        let extra = json!({"windows": [], "accounts": {
            "perso": weighed()["accounts"]["perso"],
            "odd": {"windows": [{"title": "Opus", "usedPercent": 40, "resetsAt": null}]},
        }});
        let combined = of(&[cred("perso", true), cred("odd", false)], extra, true);
        assert_eq!(
            (&combined[0]["weighted"], &combined[0]["used"]),
            (&json!(true), &json!(31))
        );
        assert_eq!(
            (
                &combined[1]["title"],
                &combined[1]["weighted"],
                &combined[1]["of"]
            ),
            (&json!("Opus"), &json!(false), &json!(100))
        );
    }

    #[test]
    fn a_team_seat_counts_as_its_fraction_of_a_pro() {
        let usage = json!({"windows": [], "accounts": {
            "max": {"windows": [{"title": "Session", "usedPercent": 100, "resetsAt": null}], "planWeight": 20},
            "team": {"windows": [{"title": "Session", "usedPercent": 100, "resetsAt": null}], "planWeight": 1.25},
        }});
        let session = &of(&[cred("max", true), cred("team", false)], usage, true)[0];
        assert_eq!(
            (&session["used"], &session["of"]),
            (&json!(106), &json!(106))
        );
    }

    #[test]
    fn every_cli_gets_its_logins_and_both_sums() {
        let state = json!({
            "providers": [{"id": "claude"}, {"id": "grok"}],
            "credentials": [cred("perso", true), cred("work", false)],
            "usage": {"updatedAt": 5, "providers": {"claude": weighed()}},
            "health": {"switchedAt": {}},
        });
        let shown = shown(&state);
        assert_eq!(shown["claude"]["accounts"]["work"]["planWeight"], 1);
        assert_eq!(shown["claude"]["weighable"], true);
        assert_eq!(shown["claude"]["weighted"][0]["of"], 105);
        assert_eq!(shown["claude"]["absolute"][0]["of"], 200);
        assert_eq!(shown["grok"]["usage"], Value::Null);
        assert_eq!(shown["grok"]["weighted"], json!([]));
    }
}
