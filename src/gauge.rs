use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use jiff::Timestamp;
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

/// Each window's trail in TokenGauge's history beside its snapshot, by
/// provider, credential and window: the instance's reset, and `[ms, percent]`
/// samples, oldest first.
type Trails = HashMap<(String, Option<String>, String), Trail>;
type Trail = (Timestamp, Samples);
type Samples = Vec<(i64, u64)>;

fn trails(snapshot: &Path, now: Timestamp) -> Trails {
    let Some(history) = crate::value::history_paths(snapshot)
        .iter()
        .find_map(|path| load(path))
    else {
        return Trails::new();
    };
    let text = |t: &Value, key: &str| t.get(key).and_then(Value::as_str).map(str::to_owned);
    let trails = history.get("trails").and_then(Value::as_array);
    trails
        .into_iter()
        .flatten()
        .filter_map(|t| {
            let resets = t
                .get("resetsAt")
                .and_then(Value::as_str)
                .and_then(|at| at.parse::<Timestamp>().ok())
                .filter(|at| *at > now)?;
            let samples = t.get("samples")?.as_array()?.iter().filter_map(|s| {
                let [at, used] = s.as_array()?.as_slice() else {
                    return None;
                };
                Some((at.as_i64()?, used.as_u64()?.min(100)))
            });
            let key = (
                text(t, "provider")?,
                text(t, "credential"),
                text(t, "window")?,
            );
            Some((key, (resets, samples.collect())))
        })
        .collect()
}

fn modified_ms(path: &Path) -> Option<i64> {
    let at = fs::metadata(path).ok()?.modified().ok()?;
    i64::try_from(at.duration_since(std::time::UNIX_EPOCH).ok()?.as_millis()).ok()
}

/// Asks TokenGauge's daemon, on the socket beside its snapshot, to fetch now
/// rather than at its next poll, waiting a second at most. No daemon, no fetch.
pub fn nudge(snapshot: &Path) {
    let Some(dir) = snapshot.parent() else {
        return;
    };
    let socket = dir.join("tokengauge.sock");
    let wait = Duration::from_secs(1);
    let (done, finished) = mpsc::channel();
    // `connect` takes no timeout, so the exchange runs where it cannot hold the caller.
    thread::spawn(move || {
        if let Ok(mut daemon) = UnixStream::connect(socket) {
            let _ = daemon.set_read_timeout(Some(wait));
            let _ = daemon.set_write_timeout(Some(wait));
            if daemon.write_all(b"{\"cmd\":\"refresh\"}\n").is_ok() {
                let _ = BufReader::new((&daemon).take(64)).read_line(&mut String::new());
            }
        }
        let _ = done.send(());
    });
    let _ = finished.recv_timeout(wait);
}

pub fn read(snapshot: &Path) -> Option<Value> {
    read_at(snapshot, Timestamp::now())
}

/// TokenGauge's figures as of `now`: a window whose reset has passed since
/// TokenGauge read it is empty again.
fn read_at(snapshot: &Path, now: Timestamp) -> Option<Value> {
    let data = load(snapshot)?;
    let payloads = data.get("payloads").unwrap_or(&data).as_array()?;
    let trails = trails(snapshot, now);
    let mut providers = Map::new();
    for p in payloads {
        let Some(id) = p.get("provider").and_then(Value::as_str) else {
            continue;
        };
        let credential = p.get("credential").and_then(Value::as_str);
        let trail = |window: &str| {
            trails.get(&(
                id.to_owned(),
                credential.map(str::to_owned),
                window.to_owned(),
            ))
        };
        let fields = provider(p, now, &trail);
        let slot = slot(&mut providers, id, p.get("credential"), now);
        if let (Some(slot), Value::Object(fields)) = (slot.as_object_mut(), fields) {
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
        slot(&mut providers, id, e.get("credential"), now)["error"] = message.into();
    }
    Some(json!({
        // A snapshot from before TokenGauge stamped it is as old as the file.
        "updatedAt": data
            .pointer("/meta/updatedAtMs")
            .and_then(Value::as_i64)
            .or_else(|| modified_ms(snapshot)),
        "providers": providers,
    }))
}

fn slot<'a>(
    providers: &'a mut Map<String, Value>,
    id: &str,
    credential: Option<&Value>,
    now: Timestamp,
) -> &'a mut Value {
    let entry = providers.entry(id.to_owned()).or_insert_with(|| {
        let mut entry = provider(&Value::Null, now, &|_| None);
        entry["accounts"] = json!({});
        entry
    });
    match credential.and_then(Value::as_str) {
        Some(name) => {
            let account = &mut entry["accounts"][name];
            if account.is_null() {
                *account = provider(&Value::Null, now, &|_| None);
            }
            account
        }
        None => entry,
    }
}

fn provider<'t>(p: &Value, now: Timestamp, trail: &dyn Fn(&str) -> Option<&'t Trail>) -> Value {
    let usage = p.get("usage").unwrap_or(&Value::Null);
    let fixed = ["primary", "secondary", "tertiary"]
        .into_iter()
        .filter_map(|k| window(None, usage.get(k)?, false, now, trail(k)));
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
        .filter_map(|w| {
            let title = w.get("title").and_then(Value::as_str);
            let name = w.get("id").and_then(Value::as_str).or(title)?;
            window(title, w.get("window")?, true, now, trail(name))
        });
    json!({
        "stale": p.get("stale").and_then(Value::as_bool).unwrap_or(false),
        // TokenGauge carries a login it is not signed into between asks, so
        // these figures can be older than the snapshot they came in.
        "askedAt": usage.get("updatedAt").and_then(Value::as_str),
        "staleReason": p.get("staleReason").and_then(Value::as_str),
        "error": p.pointer("/error/message").and_then(Value::as_str),
        "credentialState": p.get("credentialState").and_then(Value::as_str),
        "plan": usage
            .get("loginMethod")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|m| !m.is_empty()),
        // Claude's Team seats are sold as 1.25x and 6.25x a Pro.
        "planWeight": p
            .get("planWeight")
            .filter(|w| w.as_f64().is_some_and(|w| w.is_finite() && w > 0.0)),
        "windows": fixed.chain(extra).collect::<Vec<_>>(),
        "bankedResets": banked(usage.get("bankedResets")),
    })
}

fn banked(resets: Option<&Value>) -> Value {
    match resets.and_then(|r| r.get("available")?.as_u64()) {
        Some(available) if available > 0 => {
            let mut out = json!({
                "available": available,
                "expiresAt": resets.and_then(|r| r.get("expiresAt")?.as_str()),
            });
            let kinds: Vec<Value> = resets
                .and_then(|r| r.get("kinds")?.as_array())
                .into_iter()
                .flatten()
                .filter_map(|k| {
                    let kind = k
                        .get("kind")?
                        .as_str()
                        .filter(|k| crate::claude::RESET_KINDS.contains(k))?;
                    let count = k.get("count")?.as_u64().filter(|&n| n > 0)?;
                    let clears: Vec<&str> = k
                        .get("clears")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                        .collect();
                    let mut out = json!({
                        "kind": kind,
                        "count": count,
                        "expiresAt": k.get("expiresAt").and_then(Value::as_str),
                    });
                    if !clears.is_empty() {
                        out["clears"] = json!(clears);
                    }
                    if k.get("idle").and_then(Value::as_bool) == Some(true) {
                        out["idle"] = json!(true);
                    }
                    Some(out)
                })
                .collect();
            if !kinds.is_empty() {
                out["kinds"] = Value::Array(kinds);
            }
            out
        }
        _ => Value::Null,
    }
}

fn window(
    title: Option<&str>,
    w: &Value,
    extra: bool,
    now: Timestamp,
    trail: Option<&Trail>,
) -> Option<Value> {
    let used = w.get("usedPercent")?.as_u64()?;
    let title = match (title, w.get("windowMinutes").and_then(Value::as_u64)) {
        (Some(t), _) => t.to_owned(),
        (None, Some(m)) => span(m),
        (None, None) => "Usage".to_owned(),
    };
    let minutes = w.get("windowMinutes").and_then(Value::as_u64);
    let resets = w.get("resetsAt").and_then(Value::as_str);
    let reset = resets.and_then(|at| at.parse::<Timestamp>().ok());
    let elapsed = reset.is_some_and(|at| at <= now);
    let opened = reset.zip(minutes).map(|(at, m)| {
        let span = i64::try_from(m).unwrap_or(i64::MAX).saturating_mul(60_000);
        at.as_millisecond().saturating_sub(span)
    });
    // The fleet's view can hold another machine's later instance under this
    // login's name; TokenGauge counts resets this close as one instance.
    let same = |at: &Timestamp| {
        let tolerance = minutes.map_or(0, |m| m.saturating_mul(3_000).min(3_600_000));
        reset.is_some_and(|r| r.as_millisecond().abs_diff(at.as_millisecond()) <= tolerance)
    };
    let trail = trail
        .filter(|(at, _)| !elapsed && same(at))
        .map(|(_, samples)| {
            samples
                .iter()
                .filter(|(at, _)| opened.is_none_or(|o| *at >= o))
                .map(|(at, used)| json!([at, used]))
                .collect::<Vec<_>>()
        });
    Some(json!({
        "title": title,
        "minutes": minutes,
        "extra": extra,
        "usedPercent": if elapsed { 0 } else { used.min(100) },
        "resetsAt": resets.filter(|_| !elapsed),
        "trail": trail,
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
    use std::os::unix::net::UnixListener;

    #[test]
    fn a_nudge_sends_the_daemon_a_refresh_and_waits_for_its_ack() {
        let tmp = tempfile::tempdir().unwrap();
        let daemon = UnixListener::bind(tmp.path().join("tokengauge.sock")).unwrap();
        let heard = std::thread::spawn(move || {
            let (s, _) = daemon.accept().unwrap();
            let mut line = String::new();
            BufReader::new(&s).read_line(&mut line).unwrap();
            (&s).write_all(b"{\"type\":\"ack\"}\n").unwrap();
            line
        });
        nudge(&tmp.path().join("tokengauge-usage.json"));
        assert_eq!(heard.join().unwrap(), "{\"cmd\":\"refresh\"}\n");
    }

    #[test]
    fn a_daemon_that_never_ends_its_ack_holds_a_nudge_a_second_at_most() {
        let tmp = tempfile::tempdir().unwrap();
        let daemon = UnixListener::bind(tmp.path().join("tokengauge.sock")).unwrap();
        std::thread::spawn(move || {
            let (mut s, _) = daemon.accept().unwrap();
            while s.write_all(b"ack").is_ok() {
                std::thread::sleep(Duration::from_millis(100));
            }
        });
        let start = std::time::Instant::now();
        nudge(&tmp.path().join("tokengauge-usage.json"));
        assert!(start.elapsed() < Duration::from_secs(3));
    }

    #[test]
    fn a_nudge_without_a_daemon_does_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        nudge(&tmp.path().join("tokengauge-usage.json"));
        nudge(Path::new(""));
    }

    #[test]
    fn a_daemon_that_never_acks_holds_a_nudge_a_second_at_most() {
        let tmp = tempfile::tempdir().unwrap();
        let _daemon = UnixListener::bind(tmp.path().join("tokengauge.sock")).unwrap();
        let start = std::time::Instant::now();
        nudge(&tmp.path().join("tokengauge-usage.json"));
        assert!(start.elapsed() < Duration::from_secs(3));
    }

    #[test]
    fn banked_resets_ride_along_only_while_some_are_left() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("tokengauge-usage.json");
        let payload = |name: &str, resets: Value| json!({"provider": "claude", "credential": name, "usage": {"bankedResets": resets}});
        std::fs::write(
            &file,
            json!({"payloads": [
                payload("work", json!({"available": 1, "expiresAt": "2026-10-22T16:00:00+00:00"})),
                payload("spent", json!({"available": 0, "expiresAt": "2026-10-22T16:00:00+00:00"})),
                payload("odd", json!({"available": "1"})),
                payload("none", Value::Null),
            ]})
            .to_string(),
        )
        .unwrap();
        let usage = read(&file).unwrap();
        let accounts = &usage["providers"]["claude"]["accounts"];
        assert_eq!(
            accounts["work"]["bankedResets"],
            json!({"available": 1, "expiresAt": "2026-10-22T16:00:00+00:00"})
        );
        for name in ["spent", "odd", "none"] {
            assert!(accounts[name]["bankedResets"].is_null(), "{name}");
        }
    }

    #[test]
    fn banked_resets_say_their_kind_when_tokengauge_does() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("tokengauge-usage.json");
        let at = "2027-03-09T07:59:00+00:00";
        std::fs::write(
            &file,
            json!({"payloads": [{"provider": "claude", "credential": "work", "usage": {"bankedResets": {
                "available": 1, "expiresAt": at,
                "kinds": [
                    {"kind": "fable", "count": 1, "expiresAt": at, "idle": true,
                     "clears": ["five_hour", 7, "seven_day_overage_included"]},
                    {"kind": "lunar", "count": 1},
                    {"kind": "weekly", "count": 0},
                ],
            }}}]})
            .to_string(),
        )
        .unwrap();
        let usage = read(&file).unwrap();
        assert_eq!(
            usage["providers"]["claude"]["accounts"]["work"]["bankedResets"],
            json!({"available": 1, "expiresAt": at,
                   "kinds": [{"kind": "fable", "count": 1, "expiresAt": at, "idle": true,
                              "clears": ["five_hour", "seven_day_overage_included"]}]})
        );
    }

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
                        "updatedAt": "2026-09-28T23:10:00Z",
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
        let usage = read_at(&file, "2026-09-28T00:00:00Z".parse().unwrap()).unwrap();
        assert_eq!(usage["updatedAt"], 1_790_638_810_718_i64);
        let titles: Vec<_> = usage["providers"]["claude"]["windows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|w| {
                (
                    w["title"].as_str().unwrap(),
                    w["minutes"].as_u64().unwrap(),
                    w["usedPercent"].as_u64().unwrap(),
                )
            })
            .collect();
        assert_eq!(
            titles,
            [
                ("5 hours", 300, 16),
                ("Weekly", 10080, 100),
                ("Fable only", 10080, 3)
            ]
        );
        assert_eq!(
            usage["providers"]["claude"]["askedAt"],
            "2026-09-28T23:10:00Z"
        );
        let codex = &usage["providers"]["codex"];
        assert_eq!(codex["askedAt"], Value::Null);
        assert_eq!(codex["stale"], true);
        assert_eq!(codex["staleReason"], "timed out");
        assert_eq!(codex["windows"], json!([]));
        let grok = &usage["providers"]["grok"];
        assert_eq!(grok["error"], "401 Unauthorized");
        assert_eq!(grok["windows"], json!([]));

        let legacy = json!([{"provider": "claude", "usage": {"primary": {"usedPercent": 5}}}]);
        std::fs::write(&file, legacy.to_string()).unwrap();
        let usage = read(&file).unwrap();
        assert_eq!(
            usage["updatedAt"],
            json!(modified_ms(&file)),
            "as old as the file"
        );
        let window = &usage["providers"]["claude"]["windows"][0];
        assert_eq!(
            (&window["title"], &window["minutes"]),
            (&json!("Usage"), &Value::Null)
        );

        let named = json!({
            "meta": {"schemaVersion": 2, "updatedAtMs": 1},
            "payloads": [
                {"provider": "claude", "credential": "perso", "active": true, "planWeight": 20,
                 "usage": {"primary": {"usedPercent": 42, "windowMinutes": 300}, "loginMethod": " Max "}},
                {"provider": "claude", "usage": {"primary": {"usedPercent": 7, "windowMinutes": 300}}},
                {"provider": "claude", "credential": "work", "active": false,
                 "usage": {"primary": {"usedPercent": 0, "windowMinutes": 300}, "loginMethod": ""}},
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
        assert_eq!(accounts["perso"]["plan"], "Max");
        assert_eq!(accounts["work"]["plan"], Value::Null);
        assert_eq!(accounts["work"]["windows"][0]["usedPercent"], 0);
        assert_eq!(accounts["old"]["credentialState"], "expired");
        assert_eq!(accounts["old"]["windows"], json!([]));
        assert_eq!(accounts["gone"]["error"], "401 Unauthorized");
        assert_eq!(accounts["perso"]["error"], Value::Null);
        assert_eq!(usage["providers"]["codex"]["error"], "timed out");
        assert_eq!(usage["providers"]["codex"]["accounts"], json!({}));

        let odd: Vec<Value> = [json!(0), json!(-1), json!("20")]
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
        for i in 0..3 {
            assert_eq!(
                weights["providers"]["claude"]["accounts"][format!("odd{i}")]["planWeight"],
                Value::Null
            );
        }

        let seats = json!({"meta": {"schemaVersion": 2, "updatedAtMs": 1}, "payloads": [
            {"provider": "claude", "credential": "standard", "planWeight": 1.25,
             "usage": {"primary": {"usedPercent": 1}}},
            {"provider": "claude", "credential": "premium", "planWeight": 6.25,
             "usage": {"primary": {"usedPercent": 1}}},
        ]});
        std::fs::write(&file, seats.to_string()).unwrap();
        let seats = read(&file).unwrap();
        let accounts = &seats["providers"]["claude"]["accounts"];
        assert_eq!(accounts["standard"]["planWeight"], 1.25);
        assert_eq!(accounts["premium"]["planWeight"], 6.25);

        assert!(read(tmp.path()).is_none());

        std::fs::write(&file, "{ torn").unwrap();
        assert!(read(&file).is_none());
    }

    #[test]
    fn each_window_carries_its_trail_since_it_opened() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("tokengauge-usage.json");
        let five =
            json!({"usedPercent": 40, "resetsAt": "2026-11-03T15:00:00Z", "windowMinutes": 300});
        let fable =
            json!({"usedPercent": 3, "resetsAt": "2026-11-08T00:00:00Z", "windowMinutes": 10080});
        std::fs::write(
            &file,
            json!({"payloads": [{"provider": "claude", "credential": "work", "usage": {
                "primary": five,
                "extraRateWindows": [{"title": "Fable only", "window": fable}]
            }}]})
            .to_string(),
        )
        .unwrap();
        let ms = |at: &str| at.parse::<Timestamp>().unwrap().as_millisecond();
        let trail = |window: &str, resets: &str, samples: Value| {
            json!({"provider": "claude", "credential": "work", "window": window,
                   "windowMinutes": 300, "resetsAt": resets, "samples": samples})
        };
        std::fs::write(
            tmp.path().join("tokengauge-window-history.json"),
            json!({"trails": [
                trail("primary", "2026-11-03T15:00:00Z", json!([
                    [ms("2026-11-03T09:30:00Z"), 90],
                    [ms("2026-11-03T10:30:00Z"), 5],
                    [ms("2026-11-03T12:00:00Z"), 40],
                    ["odd", 3]
                ])),
                trail("Fable only", "2026-11-08T00:00:00Z", json!([[ms("2026-11-03T08:00:00Z"), 3]])),
                trail("secondary", "2026-11-03T01:00:00Z", json!([[1, 1]])),
            ]})
            .to_string(),
        )
        .unwrap();

        let usage = read_at(&file, "2026-11-03T12:30:00Z".parse().unwrap()).unwrap();
        let windows = &usage["providers"]["claude"]["accounts"]["work"]["windows"];
        assert_eq!(
            windows[0]["trail"],
            json!([
                [ms("2026-11-03T10:30:00Z"), 5],
                [ms("2026-11-03T12:00:00Z"), 40]
            ]),
            "the sample from before the window opened is the last instance's"
        );
        assert_eq!(
            windows[1]["trail"],
            json!([[ms("2026-11-03T08:00:00Z"), 3]])
        );

        let later = read_at(&file, "2026-11-03T16:00:00Z".parse().unwrap()).unwrap();
        let reset = &later["providers"]["claude"]["accounts"]["work"]["windows"][0];
        assert_eq!(reset["trail"], Value::Null);

        std::fs::write(
            tmp.path().join("tokengauge-window-fleet.json"),
            json!({"trails": [trail("primary", "2026-11-03T15:00:00Z", json!([
                [ms("2026-11-03T11:00:00Z"), 20],
                [ms("2026-11-03T12:00:00Z"), 40]
            ]))]})
            .to_string(),
        )
        .unwrap();
        let fleet = read_at(&file, "2026-11-03T12:30:00Z".parse().unwrap()).unwrap();
        assert_eq!(
            fleet["providers"]["claude"]["accounts"]["work"]["windows"][0]["trail"],
            json!([
                [ms("2026-11-03T11:00:00Z"), 20],
                [ms("2026-11-03T12:00:00Z"), 40]
            ]),
            "the fleet's view wins while fleet sync keeps one"
        );

        std::fs::write(
            tmp.path().join("tokengauge-window-fleet.json"),
            json!({"trails": [trail("primary", "2026-11-03T20:00:00Z", json!([
                [ms("2026-11-03T12:00:00Z"), 4]
            ]))]})
            .to_string(),
        )
        .unwrap();
        let later = read_at(&file, "2026-11-03T12:30:00Z".parse().unwrap()).unwrap();
        assert_eq!(
            later["providers"]["claude"]["accounts"]["work"]["windows"][0]["trail"],
            Value::Null,
            "another machine's later instance is not this window's"
        );

        std::fs::remove_file(tmp.path().join("tokengauge-window-fleet.json")).unwrap();
        std::fs::remove_file(tmp.path().join("tokengauge-window-history.json")).unwrap();
        let bare = read_at(&file, "2026-11-03T12:30:00Z".parse().unwrap()).unwrap();
        let windows = &bare["providers"]["claude"]["accounts"]["work"]["windows"];
        assert_eq!(windows[0]["trail"], Value::Null);
    }

    #[test]
    fn a_window_of_absurd_length_keeps_its_whole_trail() {
        let now: Timestamp = "2026-11-03T12:00:00Z".parse().unwrap();
        let samples = vec![(now.as_millisecond() - 60_000, 30)];
        let w = json!({"usedPercent": 30, "resetsAt": "2026-11-03T13:00:00Z", "windowMinutes": u64::MAX / 2});
        let reset = "2026-11-03T13:00:00Z".parse().unwrap();
        let shown = window(None, &w, false, now, Some(&(reset, samples))).unwrap();
        assert_eq!(shown["trail"], json!([[now.as_millisecond() - 60_000, 30]]));
    }

    #[test]
    fn a_window_whose_reset_has_passed_is_back_at_zero() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("tokengauge-usage.json");
        std::fs::write(
            &file,
            json!({"payloads": [{"provider": "claude", "credential": "work", "stale": true, "usage": {
                "primary": {"usedPercent": 10, "resetsAt": "2026-10-19T17:59:59.535049+00:00", "windowMinutes": 300},
                "secondary": {"usedPercent": 9, "resetsAt": "2026-10-24T00:59:59.535070+00:00", "windowMinutes": 10080},
                "tertiary": {"usedPercent": 40, "resetsAt": "soon", "windowMinutes": 1440},
                "extraRateWindows": [{"title": "Fable only", "placeholder": false,
                    "window": {"usedPercent": 7, "resetsAt": "2026-10-20T08:00:00+02:00", "windowMinutes": 10080}}]
            }}]})
            .to_string(),
        )
        .unwrap();
        let shown = |now: &str| {
            let usage = read_at(&file, now.parse().unwrap()).unwrap();
            usage["providers"]["claude"]["accounts"]["work"]["windows"]
                .as_array()
                .unwrap()
                .iter()
                .map(|w| {
                    (
                        w["usedPercent"].as_u64().unwrap(),
                        w["resetsAt"].is_string(),
                    )
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(
            shown("2026-10-19T17:00:00Z"),
            [(10, true), (9, true), (40, true), (7, true)]
        );
        assert_eq!(
            shown("2026-10-20T07:00:00Z"),
            [(0, false), (9, true), (40, true), (0, false)]
        );
    }
}
