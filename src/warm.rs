use std::collections::BTreeMap;

use anyhow::{Context, Result, bail};
use jiff::Timestamp;
use serde_json::Value;

use crate::auto::{self, Figures};
use crate::fsx::now_ms;
use crate::ops;
use crate::provider::Provider;
use crate::store::Store;

/// The window a ping opens, as TokenGauge reports its length.
const WINDOW_MINUTES: u64 = 300;
/// An inactive login's token is renewed first when it has less than this left.
const FRESH_FOR_MS: i64 = 5 * 60_000;
/// The least time between two pings of one login, whatever TokenGauge says.
const MIN_GAP_MS: i64 = 5 * 60_000;
/// How long a failed ping waits before it is tried again.
const RETRY_MS: i64 = 10 * 60_000;

/// When a login was last pinged, and whether that ping went through.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Last {
    pub at: i64,
    pub ok: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Due {
    Now,
    Open { resets_at: Timestamp },
    Not(String),
}

/// Whether `name`'s 5-hour window is closed, from TokenGauge's figures. A
/// login pinged before waits until TokenGauge has asked about it since
/// (`asked`), so it is never pinged twice on the same figures; a failed ping
/// is tried again after a while.
pub fn due(
    figures: &Result<BTreeMap<String, Figures>, String>,
    name: &str,
    now: Timestamp,
    last: Option<Last>,
    asked: Option<i64>,
) -> Due {
    let figures = match figures {
        Ok(f) => f,
        Err(why) => return Due::Not(why.clone()),
    };
    let windows = match figures.get(name).map(|f| auto::current(f, now)) {
        None => return Due::Not("TokenGauge has no figures for it".into()),
        Some(Figures::Missing(why)) => return Due::Not(why),
        Some(Figures::Known(w)) => w,
    };
    let Some(window) = windows
        .iter()
        .find(|w| !w.extra && w.minutes == Some(WINDOW_MINUTES))
    else {
        return Due::Not("it has no 5-hour window".into());
    };
    if let Some(resets_at) = window.resets_at.filter(|at| *at > now) {
        return Due::Open { resets_at };
    }
    if let Some(last) = last {
        let since = now.as_millisecond() - last.at;
        if !last.ok && since < RETRY_MS {
            let minutes = (RETRY_MS - since + 59_999) / 60_000;
            return Due::Not(format!(
                "the last ping failed; trying again in {minutes} min"
            ));
        }
        if last.ok && (since < MIN_GAP_MS || asked.is_none_or(|a| a <= last.at)) {
            return Due::Not("waiting for TokenGauge to see the last ping".into());
        }
    }
    Due::Now
}

/// One login named in `[warm]`, and whether it is due.
#[derive(Debug, Clone, PartialEq)]
pub struct Planned {
    pub provider: String,
    pub name: String,
    pub due: Due,
}

/// Every login named in `[warm]`. `active(provider)` is the login the CLI is
/// on, or why nothing of that provider may be pinged (a live login remuda
/// cannot tie to a stored one could be any of them). `last(provider, name)`
/// is when that login was last pinged. Each login is judged by when
/// TokenGauge last asked about it, not by the snapshot's own time: it carries
/// a login the CLI is not on between asks.
pub fn plan(
    warm: &BTreeMap<String, Vec<String>>,
    usage: Option<&Value>,
    switches: &BTreeMap<String, i64>,
    active: &dyn Fn(&str) -> Result<Option<String>, String>,
    last: &dyn Fn(&str, &str) -> Option<Last>,
    now: Timestamp,
) -> Vec<Planned> {
    let updated = usage
        .and_then(|u| u.get("updatedAt"))
        .and_then(Value::as_i64);
    let mut out = Vec::new();
    for (provider, names) in warm {
        let on = active(provider);
        let asked = |name: &str| {
            let p = usage?.pointer(&format!("/providers/{provider}"))?;
            let own = p
                .get("accounts")
                .and_then(|a| a.get(name))
                .filter(|a| !a.is_null());
            let current = on.as_ref().ok().and_then(Option::as_deref) == Some(name);
            let at = own
                .or_else(|| current.then_some(p))?
                .get("askedAt")?
                .as_str()?;
            at.parse::<Timestamp>().ok().map(|t| t.as_millisecond())
        };
        let figures = on.clone().and_then(|active| {
            let switched = switches
                .get(provider)
                .and_then(|ms| Timestamp::from_millisecond(*ms).ok());
            auto::figures(usage, provider, active.as_deref(), switched, now)
        });
        for name in names {
            out.push(Planned {
                provider: provider.clone(),
                name: name.clone(),
                due: due(
                    &figures,
                    name,
                    now,
                    last(provider, name),
                    asked(name).or(updated),
                ),
            });
        }
    }
    out
}

/// The login a CLI is on, for [`plan`].
pub fn active(live: &ops::LiveState) -> Result<Option<String>, String> {
    use ops::LiveState;
    match live {
        LiveState::Stored { name, synced: true } => Ok(Some(name.clone())),
        LiveState::Stored { name, .. } => Err(format!(
            "the live login could not be confirmed as {name}, so any stored one could be it"
        )),
        LiveState::Unreadable { error } => Err(format!("the CLI's files cannot be read: {error}")),
        _ => Ok(None),
    }
}

/// What `remuda auto` says of a planned login.
pub fn describe(planned: &Planned) -> String {
    let what = match &planned.due {
        Due::Now => "its 5-hour window is closed; remuda serve opens it".to_owned(),
        Due::Open { resets_at } => format!("its 5-hour window is open until {resets_at}"),
        Due::Not(why) => format!("not pinged: {why}"),
    };
    format!("keep warm {}/{}: {what}", planned.provider, planned.name)
}

/// The credentials a ping for `name` spends: the CLI's own when it is the
/// active login, which is never refreshed, else the stored copy, renewed
/// first when its token is about to expire.
pub fn creds(store: &Store, p: &dyn Provider, name: &str, active: Option<&str>) -> Result<Value> {
    if active == Some(name) {
        let live = p.live()?.context("the CLI is signed out")?;
        if p.expires_at(&live).is_some_and(|at| at <= now_ms()) {
            bail!("its access token has expired, and the CLI renews it, not remuda");
        }
        return Ok(live);
    }
    let mut entry = store
        .get(p.id(), name)?
        .with_context(|| format!("no stored login {}/{name}", p.id()))?;
    if !entry.verified {
        bail!("it does not match its sidecar, so remuda will not spend or refresh it");
    }
    if p.renews() && ops::expires_in_ms(p, &entry).is_some_and(|left| left < FRESH_FOR_MS) {
        ops::refresh_entry(store, p, &mut entry)?;
        store.save(&entry)?;
    }
    Ok(entry.creds)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auto::Window;

    fn t(s: &str) -> Timestamp {
        s.parse().unwrap()
    }

    fn five(resets_at: Option<&str>) -> Window {
        Window {
            title: "5h".into(),
            minutes: Some(300),
            used: 3,
            resets_at: resets_at.map(t),
            extra: false,
        }
    }

    fn known(name: &str, w: Vec<Window>) -> Result<BTreeMap<String, Figures>, String> {
        Ok(BTreeMap::from([(name.to_owned(), Figures::Known(w))]))
    }

    const NOW: &str = "2026-10-21T10:00:00Z";

    #[test]
    fn a_closed_window_is_due_and_an_open_one_waits_for_its_reset() {
        let now = t(NOW);
        assert_eq!(
            due(&known("work", vec![five(None)]), "work", now, None, None),
            Due::Now
        );
        assert_eq!(
            due(
                &known("work", vec![five(Some("2026-10-21T09:00:00Z"))]),
                "work",
                now,
                None,
                None
            ),
            Due::Now
        );
        assert_eq!(
            due(
                &known("work", vec![five(Some("2026-10-21T12:00:00Z"))]),
                "work",
                now,
                None,
                None
            ),
            Due::Open {
                resets_at: t("2026-10-21T12:00:00Z")
            }
        );
    }

    #[test]
    fn a_login_is_not_due_without_figures_a_window_or_a_newer_snapshot() {
        let now = t(NOW);
        let week = Window {
            minutes: Some(10080),
            ..five(None)
        };
        let not = |d: Due| match d {
            Due::Not(why) => why,
            other => panic!("{other:?}"),
        };
        assert!(not(due(&Err("stale".into()), "work", now, None, None)).contains("stale"));
        assert!(
            not(due(
                &known("perso", vec![five(None)]),
                "work",
                now,
                None,
                None
            ))
            .contains("no figures")
        );
        assert!(
            not(due(&known("work", vec![week]), "work", now, None, None)).contains("no 5-hour")
        );
        let extra = Window {
            extra: true,
            ..five(None)
        };
        assert!(
            not(due(&known("work", vec![extra]), "work", now, None, None)).contains("no 5-hour")
        );
        let missing = Ok(BTreeMap::from([(
            "work".to_owned(),
            Figures::Missing("TokenGauge: boom".into()),
        )]));
        assert!(not(due(&missing, "work", now, None, None)).contains("boom"));

        let ping = now.as_millisecond() - 10 * 60_000;
        let ok = Some(Last { at: ping, ok: true });
        let closed = known("work", vec![five(None)]);
        assert!(not(due(&closed, "work", now, ok, Some(ping - 1))).contains("waiting"));
        assert!(not(due(&closed, "work", now, ok, None)).contains("waiting"));
        assert_eq!(due(&closed, "work", now, ok, Some(ping + 1)), Due::Now);

        let recent = Some(Last {
            at: now.as_millisecond() - 60_000,
            ok: true,
        });
        let asked = Some(now.as_millisecond());
        assert!(not(due(&closed, "work", now, recent, asked)).contains("waiting"));

        let failed = Some(Last {
            at: now.as_millisecond() - 3 * 60_000,
            ok: false,
        });
        assert!(not(due(&closed, "work", now, failed, None)).contains("again in 7 min"));
        let failed = Some(Last {
            at: now.as_millisecond() - 11 * 60_000,
            ok: false,
        });
        assert_eq!(due(&closed, "work", now, failed, None), Due::Now);
    }

    #[test]
    fn a_login_waits_for_its_own_ask_not_the_snapshot() {
        let now = t(NOW);
        let ping = now.as_millisecond() - 10 * 60_000;
        let closed = serde_json::json!({"windows": [
            {"title": "5h", "minutes": 300, "usedPercent": 0, "resetsAt": null, "extra": false}
        ]});
        let mut perso = closed.clone();
        perso["askedAt"] = "2026-10-21T09:00:00Z".into();
        let mut work = closed.clone();
        work["askedAt"] = "2026-10-21T09:59:00Z".into();
        let usage = serde_json::json!({
            "updatedAt": now.as_millisecond(),
            "providers": {"claude": {"accounts": {"perso": perso, "work": work}}},
        });
        let warm = BTreeMap::from([("claude".to_owned(), vec!["perso".into(), "work".into()])]);
        let planned = plan(
            &warm,
            Some(&usage),
            &BTreeMap::new(),
            &|_| Ok(Some("work".into())),
            &|_, _| Some(Last { at: ping, ok: true }),
            now,
        );
        let dues: Vec<_> = planned.iter().map(|p| (p.name.as_str(), &p.due)).collect();
        assert!(matches!(dues[0], ("perso", Due::Not(_))), "{dues:?}");
        assert_eq!(dues[1], ("work", &Due::Now));
        assert_eq!(
            describe(&planned[1]),
            "keep warm claude/work: its 5-hour window is closed; remuda serve opens it"
        );
    }

    #[test]
    fn the_active_login_is_spent_but_never_refreshed() {
        use crate::ops::testing::*;
        let mut server = mockito::Server::new();
        let refresh = server.mock("POST", "/v1/oauth/token").expect(0).create();
        let e = env(&server.url());
        e.sign_in(oauth("a-live", "r-live", -HOUR), account("u-1"));
        let err = creds(&e.store, &e.claude, "work", Some("work"))
            .unwrap_err()
            .to_string();
        assert!(err.contains("expired"), "{err}");
        e.sign_in(oauth("a-live", "r-live", HOUR), account("u-1"));
        let live = creds(&e.store, &e.claude, "work", Some("work")).unwrap();
        assert_eq!(e.claude.access_token(&live), Some("a-live"));
        refresh.assert();
    }

    #[test]
    fn an_inactive_login_about_to_expire_is_renewed_and_saved_first() {
        use crate::ops::testing::*;
        let mut server = mockito::Server::new();
        let refresh = server
            .mock("POST", "/v1/oauth/token")
            .with_body(
                serde_json::json!({
                    "access_token": "a-new", "refresh_token": "r-new", "expires_in": 3600,
                    "account": {"uuid": "u-2", "email_address": "u-2@example.com"},
                })
                .to_string(),
            )
            .create();
        let e = env(&server.url());
        e.stored("perso", "u-2", 60_000);
        let got = creds(&e.store, &e.claude, "perso", Some("work")).unwrap();
        assert_eq!(e.claude.access_token(&got), Some("a-new"));
        assert_eq!(e.stored_refresh_token("perso").as_deref(), Some("r-new"));
        refresh.assert();

        e.stored("job", "u-3", HOUR);
        let got = creds(&e.store, &e.claude, "job", None).unwrap();
        assert_eq!(e.claude.access_token(&got), Some("a-job"));
        assert!(creds(&e.store, &e.claude, "ghost", None).is_err());
    }

    #[test]
    fn an_unverified_login_is_neither_spent_nor_refreshed() {
        use crate::ops::testing::*;
        let mut server = mockito::Server::new();
        let refresh = server.mock("POST", "/v1/oauth/token").expect(0).create();
        let e = env(&server.url());
        e.stored("perso", "u-2", 60_000);
        crate::fsx::write_json(
            &e.store.root().join("claude/perso.json"),
            &oauth("a-torn", "r-torn", 60_000),
        )
        .unwrap();
        let err = creds(&e.store, &e.claude, "perso", None)
            .unwrap_err()
            .to_string();
        assert!(err.contains("does not match its sidecar"), "{err}");
        assert_eq!(e.stored_refresh_token("perso").as_deref(), Some("r-torn"));
        refresh.assert();
    }
}
