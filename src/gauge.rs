use std::path::Path;

use serde_json::{Map, Value, json};

use crate::fsx::read_json;

/// TokenGauge's usage snapshot, reduced to what the page draws: per provider,
/// the rate windows of the login TokenGauge last saw. remuda only reads it.
pub fn read(snapshot: &Path) -> Option<Value> {
    let data = read_json(snapshot).ok().flatten()?;
    let payloads = data
        .get("payloads")
        .or(Some(&data))
        .and_then(Value::as_array)?;
    let mut providers = Map::new();
    for p in payloads {
        let Some(id) = p.get("provider").and_then(Value::as_str) else {
            continue;
        };
        providers.insert(id.to_owned(), provider(p));
    }
    Some(json!({
        "updatedAt": data.pointer("/meta/updatedAtMs").and_then(Value::as_i64),
        "providers": providers,
    }))
}

fn provider(p: &Value) -> Value {
    let usage = p.get("usage").unwrap_or(&Value::Null);
    let fixed = ["primary", "secondary", "tertiary"]
        .into_iter()
        .filter_map(|k| window(None, usage.get(k)?));
    let extra = usage
        .get("extraRateWindows")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|w| {
            !w.get("placeholder")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        })
        .filter_map(|w| window(w.get("title").and_then(Value::as_str), w.get("window")?));
    json!({
        "stale": p.get("stale").and_then(Value::as_bool).unwrap_or(false),
        "error": p.pointer("/error/message").and_then(Value::as_str),
        "windows": fixed.chain(extra).collect::<Vec<_>>(),
    })
}

fn window(title: Option<&str>, w: &Value) -> Option<Value> {
    let used = w.get("usedPercent")?.as_u64()?;
    let minutes = w.get("windowMinutes").and_then(Value::as_u64);
    let title = match (title, minutes) {
        (Some(t), _) => t.to_owned(),
        (None, Some(m)) => span(m),
        (None, None) => "Usage".to_owned(),
    };
    Some(json!({
        "title": title,
        "usedPercent": used.min(100),
        "resetsAt": w.get("resetsAt").and_then(Value::as_str),
        "windowMinutes": minutes,
    }))
}

fn span(minutes: u64) -> String {
    match minutes {
        1440 => "Daily".into(),
        10080 => "Weekly".into(),
        43200 => "Monthly".into(),
        m if m % 1440 == 0 => format!("{} days", m / 1440),
        m if m % 60 == 0 => format!("{} hours", m / 60),
        m => format!("{m} minutes"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_snapshot_becomes_titled_windows_per_provider() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("tokengauge-usage.json");
        assert!(read(&file).is_none());
        std::fs::write(
            &file,
            json!({
                "meta": {"updatedAtMs": 1_790_638_810_718_i64},
                "payloads": [
                    {"provider": "claude", "stale": false, "error": null, "usage": {
                        "primary": {"usedPercent": 16, "resetsAt": "2026-09-29T03:30:00Z", "windowMinutes": 300},
                        "secondary": {"usedPercent": 140, "resetsAt": null, "windowMinutes": 10080},
                        "tertiary": null,
                        "extraRateWindows": [
                            {"title": "Daily Routines", "placeholder": true,
                             "window": {"usedPercent": 0, "windowMinutes": 10080}},
                            {"title": "Fable only", "placeholder": false,
                             "window": {"usedPercent": 3, "windowMinutes": 10080}}
                        ]
                    }},
                    {"provider": "codex", "stale": true, "error": {"message": "401"}, "usage": null},
                    {"usage": {}}
                ]
            })
            .to_string(),
        )
        .unwrap();
        let usage = read(&file).unwrap();
        assert_eq!(usage["updatedAt"], 1_790_638_810_718_i64);
        let titles: Vec<_> = usage["providers"]["claude"]["windows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|w| {
                (
                    w["title"].as_str().unwrap(),
                    w["usedPercent"].as_u64().unwrap(),
                )
            })
            .collect();
        assert_eq!(
            titles,
            [("5 hours", 16), ("Weekly", 100), ("Fable only", 3)]
        );
        let codex = &usage["providers"]["codex"];
        assert_eq!(codex["stale"], true);
        assert_eq!(codex["error"], "401");
        assert_eq!(codex["windows"], json!([]));

        std::fs::write(&file, "{ torn").unwrap();
        assert!(read(&file).is_none());
    }
}
