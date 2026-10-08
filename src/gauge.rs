use std::fs::{self, File};
use std::io::Read;
use std::path::Path;

use serde_json::{Map, Value, json};

const LIMIT: u64 = 4 << 20;

fn load(snapshot: &Path) -> Option<Value> {
    if !fs::metadata(snapshot).ok()?.is_file() {
        return None;
    }
    let mut text = String::new();
    File::open(snapshot)
        .ok()?
        .take(LIMIT)
        .read_to_string(&mut text)
        .ok()?;
    serde_json::from_str(&text).ok()
}

pub fn read(snapshot: &Path) -> Option<Value> {
    let data = load(snapshot)?;
    let payloads = data.get("payloads").unwrap_or(&data).as_array()?;
    let mut providers = Map::new();
    for p in payloads {
        let Some(id) = p.get("provider").and_then(Value::as_str) else {
            continue;
        };
        let slot = slot(&mut providers, id, p.get("credential"));
        if let (Some(slot), Value::Object(fields)) = (slot.as_object_mut(), provider(p)) {
            slot.extend(fields);
        }
    }
    let errors = data.get("errors").and_then(Value::as_array);
    for e in errors.into_iter().flatten() {
        let (Some(id), Some(message)) = (
            e.get("provider").and_then(Value::as_str),
            e.get("message").and_then(Value::as_str),
        ) else {
            continue;
        };
        slot(&mut providers, id, e.get("credential"))["error"] = message.into();
    }
    Some(json!({
        "updatedAt": data.pointer("/meta/updatedAtMs").and_then(Value::as_i64),
        "providers": providers,
    }))
}

fn slot<'a>(
    providers: &'a mut Map<String, Value>,
    id: &str,
    credential: Option<&Value>,
) -> &'a mut Value {
    let entry = providers.entry(id.to_owned()).or_insert_with(|| {
        let mut entry = provider(&Value::Null);
        entry["accounts"] = json!({});
        entry
    });
    match credential.and_then(Value::as_str) {
        Some(name) => {
            let account = &mut entry["accounts"][name];
            if account.is_null() {
                *account = provider(&Value::Null);
            }
            account
        }
        None => entry,
    }
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
        "staleReason": p.get("staleReason").and_then(Value::as_str),
        "error": p.pointer("/error/message").and_then(Value::as_str),
        "credentialState": p.get("credentialState").and_then(Value::as_str),
        "planWeight": p.get("planWeight").and_then(Value::as_u64).filter(|w| *w > 0),
        "windows": fixed.chain(extra).collect::<Vec<_>>(),
    })
}

fn window(title: Option<&str>, w: &Value) -> Option<Value> {
    let used = w.get("usedPercent")?.as_u64()?;
    let title = match (title, w.get("windowMinutes").and_then(Value::as_u64)) {
        (Some(t), _) => t.to_owned(),
        (None, Some(m)) => span(m),
        (None, None) => "Usage".to_owned(),
    };
    Some(json!({
        "title": title,
        "usedPercent": used.min(100),
        "resetsAt": w.get("resetsAt").and_then(Value::as_str),
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
                        "tertiary": {"usedPercent": null, "windowMinutes": 60},
                        "extraRateWindows": [
                            {"title": "Daily Routines", "placeholder": true,
                             "window": {"usedPercent": 0, "windowMinutes": 10080}},
                            {"title": "Fable only", "placeholder": false,
                             "window": {"usedPercent": 3, "windowMinutes": 10080}}
                        ]
                    }},
                    {"provider": "codex", "stale": true, "staleReason": "timed out", "usage": null},
                    {"usage": {}}
                ],
                "errors": [{"provider": "grok", "message": "401 Unauthorized", "raw": "..."}]
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
        assert_eq!(codex["staleReason"], "timed out");
        assert_eq!(codex["windows"], json!([]));
        let grok = &usage["providers"]["grok"];
        assert_eq!(grok["error"], "401 Unauthorized");
        assert_eq!(grok["windows"], json!([]));

        let legacy = json!([{"provider": "claude", "usage": {"primary": {"usedPercent": 5}}}]);
        std::fs::write(&file, legacy.to_string()).unwrap();
        assert_eq!(
            read(&file).unwrap()["providers"]["claude"]["windows"][0]["title"],
            "Usage"
        );

        let named = json!({
            "meta": {"schemaVersion": 2, "updatedAtMs": 1},
            "payloads": [
                {"provider": "claude", "credential": "perso", "active": true, "planWeight": 20,
                 "usage": {"primary": {"usedPercent": 42, "windowMinutes": 300}}},
                {"provider": "claude", "usage": {"primary": {"usedPercent": 7, "windowMinutes": 300}}},
                {"provider": "claude", "credential": "work", "active": false,
                 "usage": {"primary": {"usedPercent": 0, "windowMinutes": 300}}},
                {"provider": "claude", "credential": "old", "active": false,
                 "credentialState": "expired", "usage": null},
            ],
            "errors": [
                {"provider": "claude", "credential": "gone", "message": "401 Unauthorized"},
                {"provider": "codex", "active": true, "message": "timed out"},
            ],
        });
        std::fs::write(&file, named.to_string()).unwrap();
        let usage = read(&file).unwrap();
        let claude = &usage["providers"]["claude"];
        assert_eq!(claude["windows"][0]["usedPercent"], 7);
        assert_eq!(claude["error"], Value::Null);
        let accounts = &claude["accounts"];
        assert_eq!(accounts["perso"]["windows"][0]["usedPercent"], 42);
        assert_eq!(accounts["perso"]["planWeight"], 20);
        assert_eq!(accounts["work"]["planWeight"], Value::Null);
        assert_eq!(accounts["work"]["windows"][0]["usedPercent"], 0);
        assert_eq!(accounts["old"]["credentialState"], "expired");
        assert_eq!(accounts["old"]["windows"], json!([]));
        assert_eq!(accounts["gone"]["error"], "401 Unauthorized");
        assert_eq!(accounts["perso"]["error"], Value::Null);
        assert_eq!(usage["providers"]["codex"]["error"], "timed out");
        assert_eq!(usage["providers"]["codex"]["accounts"], json!({}));

        let odd: Vec<Value> = [json!(0), json!(-1), json!(20.5), json!("20")]
            .into_iter()
            .enumerate()
            .map(|(i, weight)| {
                json!({"provider": "claude", "credential": format!("odd{i}"), "planWeight": weight,
                       "usage": {"primary": {"usedPercent": 1}}})
            })
            .collect();
        let odd = json!({"meta": {"schemaVersion": 2, "updatedAtMs": 1}, "payloads": odd});
        std::fs::write(&file, odd.to_string()).unwrap();
        let weights = read(&file).unwrap();
        for i in 0..4 {
            assert_eq!(
                weights["providers"]["claude"]["accounts"][format!("odd{i}")]["planWeight"],
                Value::Null
            );
        }

        assert!(read(tmp.path()).is_none());

        std::fs::write(&file, "{ torn").unwrap();
        assert!(read(&file).is_none());
    }
}
