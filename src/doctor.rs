use std::collections::HashMap;
use std::ffi::OsStr;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Result, bail};
use selvedge::update::version_gt;
use serde::Serialize;
use serde_json::Value;

use crate::commands::Live;
use crate::ops::LiveState;
use crate::provider::Provider;
use crate::store::Store;
use crate::{child, gauge, pins, runs, units};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Ok,
    Info,
    Warn,
    Bad,
}

impl Level {
    fn as_str(self) -> &'static str {
        match self {
            Level::Ok => "ok",
            Level::Info => "info",
            Level::Warn => "warn",
            Level::Bad => "bad",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Vital {
    pub id: String,
    pub level: Level,
    pub title: String,
    /// `{since}` in it stands for how long ago `since` was, so the page can
    /// keep it current.
    pub detail: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub since: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fix: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub items: Vec<String>,
}

impl Vital {
    fn new(id: &str, level: Level, title: &str, detail: impl Into<String>) -> Vital {
        Vital {
            id: id.to_owned(),
            level,
            title: title.to_owned(),
            detail: detail.into(),
            since: None,
            fix: None,
            items: Vec::new(),
        }
    }

    fn since(mut self, at: i64) -> Vital {
        self.since = Some(at);
        self
    }

    fn fix(mut self, fix: impl Into<String>) -> Vital {
        self.fix = Some(fix.into());
        self
    }

    fn items(mut self, items: Vec<String>) -> Vital {
        self.items = items;
        self
    }

    pub fn detail_at(&self, now: i64) -> String {
        match self.since {
            Some(at) => self.detail.replace("{since}", &ago(now - at)),
            None => self.detail.clone(),
        }
    }
}

const MIN: i64 = 60_000;
const STALE_SNAPSHOT: i64 = 30 * MIN;
const LATE_REFRESH: i64 = 75 * MIN;

fn ago(ms: i64) -> String {
    let plural = |n: i64, unit: &str| format!("{n} {unit}{} ago", if n == 1 { "" } else { "s" });
    match ms.max(0) / MIN {
        0 => "under a minute ago".to_owned(),
        m if m < 60 => plural(m, "minute"),
        m if m < 24 * 60 => plural(m / 60, "hour"),
        m => plural(m / (24 * 60), "day"),
    }
}

pub struct Checkup<'a> {
    pub store: &'a Store,
    pub lives: &'a [Live<'a>],
    pub units: &'a [PathBuf],
    pub snapshot: &'a Path,
    pub config: &'a Path,
    /// The version of the `remuda serve` that runs, if one does, and
    /// whether a remuda unit runs it.
    pub serving: Option<(String, bool)>,
    /// Where to look for the CLIs, as `$PATH`.
    pub search: Option<&'a OsStr>,
    /// The directory whose `.remuda.toml` is looked at, if any.
    pub here: Option<&'a Path>,
    pub now: i64,
}

pub fn vitals(c: &Checkup) -> Vec<Vital> {
    let mut out = Vec::new();
    let config = match crate::config::load(c.config) {
        Ok(loaded) => loaded.config,
        Err(e) => {
            out.push(
                Vital::new("config", Level::Bad, "Settings", format!("{e:#}"))
                    .fix(format!("fix or remove {}", c.config.display())),
            );
            crate::config::Config::default()
        }
    };
    let automated: Vec<&str> = c
        .lives
        .iter()
        .map(|l| l.provider.id())
        .filter(|id| config.auto.get(*id).is_some_and(|a| a.enabled))
        .collect();
    let proxy = crate::proxy::settings::File::at(c.store.root().join(".proxy.json"))
        .read()
        .is_ok_and(|s| s.enabled);
    let mut needs = Vec::new();
    if !automated.is_empty() {
        needs.push("auto mode");
    }
    if !config.warm.is_empty() {
        needs.push("keep-warm");
    }
    if proxy {
        needs.push("the API proxy");
    }
    out.push(serve(c, &needs));
    let timer = units::state(c.units, units::TIMER, "timers.target.wants");
    out.push(timer_vital(timer));
    out.push(refresh(c, timer));
    out.push(tokengauge(c, needs.iter().any(|n| *n != "the API proxy")));
    out.push(logins(c));
    out.push(live(c, &automated));
    out.extend(versions(c));
    out.extend(pin(c));
    out
}

fn and_list(items: &[&str]) -> String {
    match items {
        [] => String::new(),
        [one] => (*one).to_owned(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

fn serve(c: &Checkup, needs: &[&str]) -> Vital {
    let unit = units::state(c.units, units::SERVE, "default.target.wants");
    let start = match unit {
        "masked" => format!("`systemctl --user unmask {}`", units::SERVE),
        "absent" => "`remuda serve`, or install remuda-serve.service with install.sh".to_owned(),
        _ => format!("`systemctl --user enable --now {}`", units::SERVE),
    };
    let title = "remuda serve";
    let Some((version, service)) = &c.serving else {
        return if needs.is_empty() {
            Vital::new(
                "serve",
                Level::Info,
                title,
                "not running; nothing set up needs it",
            )
        } else {
            Vital::new(
                "serve",
                Level::Bad,
                title,
                format!(
                    "not running, so {} {} nothing",
                    and_list(needs),
                    if needs.len() == 1 { "does" } else { "do" }
                ),
            )
            .fix(start)
        };
    };
    let ours = env!("CARGO_PKG_VERSION");
    if version != ours {
        let fix = if version_gt(ours, version) {
            format!("`systemctl --user restart {}`", units::SERVE)
        } else {
            "`remuda --update`, or look at which remuda comes first on $PATH".to_owned()
        };
        return Vital::new(
            "serve",
            Level::Warn,
            title,
            format!("runs remuda {version}, and this is {ours}"),
        )
        .fix(fix);
    }
    if *service {
        return Vital::new(
            "serve",
            Level::Ok,
            title,
            format!("running as {}", units::SERVE),
        );
    }
    let level = if needs.is_empty() {
        Level::Info
    } else {
        Level::Warn
    };
    Vital::new(
        "serve",
        level,
        title,
        format!(
            "running, but not as {}: it stops with its terminal or at logout",
            units::SERVE
        ),
    )
    .fix(start)
}

fn timer_vital(state: &str) -> Vital {
    let title = "Refresh timer";
    match state {
        "enabled" => Vital::new("timer", Level::Ok, title, "on, every 30 minutes"),
        "masked" => Vital::new("timer", Level::Warn, title, "masked")
            .fix(format!("`systemctl --user unmask {}`", units::TIMER)),
        "disabled" => Vital::new("timer", Level::Warn, title, "installed but off")
            .fix(format!("`systemctl --user enable --now {}`", units::TIMER)),
        _ => Vital::new(
            "timer",
            Level::Warn,
            title,
            "not installed: the logins not in use stay fresh only when refreshed by hand",
        )
        .fix("install it with install.sh, or `remuda refresh` now and then"),
    }
}

fn refresh(c: &Checkup, timer: &str) -> Vital {
    let title = "Last refresh";
    let Some(run) = runs::last(c.store) else {
        return Vital::new("refresh", Level::Info, title, "the timer has not run yet");
    };
    let refreshed = match run.refreshed.len() {
        1 => "1 login".to_owned(),
        n => format!("{n} logins"),
    };
    if !run.problems.is_empty() {
        let problems = match run.problems.len() {
            1 => "1 problem".to_owned(),
            n => format!("{n} problems"),
        };
        return Vital::new(
            "refresh",
            Level::Bad,
            title,
            format!("{{since}}, refreshed {refreshed}, {problems}"),
        )
        .since(run.at)
        .items(run.problems);
    }
    if timer == "enabled" && c.now - run.at > LATE_REFRESH {
        return Vital::new(
            "refresh",
            Level::Warn,
            title,
            "{since}, though the timer runs every 30 minutes",
        )
        .since(run.at)
        .fix(format!("`systemctl --user status {}`", units::TIMER));
    }
    Vital::new(
        "refresh",
        Level::Ok,
        title,
        format!("{{since}}, refreshed {refreshed}"),
    )
    .since(run.at)
}

fn tokengauge(c: &Checkup, needed: bool) -> Vital {
    let title = "TokenGauge";
    if !c.snapshot.exists() {
        let (level, detail) = if needed {
            (
                Level::Warn,
                "not found: auto mode and keep-warm hold without its figures",
            )
        } else {
            (Level::Info, "not found: usage figures come from it")
        };
        return Vital::new("tokengauge", level, title, detail)
            .fix("install it from https://github.com/Arzaroth/TokenGauge");
    }
    let Some(usage) = gauge::read(c.snapshot) else {
        return Vital::new(
            "tokengauge",
            Level::Warn,
            title,
            format!("{} cannot be read", c.snapshot.display()),
        );
    };
    let errors = gauge_errors(&usage);
    let Some(at) = usage.get("updatedAt").and_then(Value::as_i64) else {
        return Vital::new(
            "tokengauge",
            Level::Warn,
            title,
            "its snapshot carries no time",
        )
        .items(errors);
    };
    if c.now - at > STALE_SNAPSHOT {
        let holds = if needed {
            ": auto mode and keep-warm hold until it fetches again"
        } else {
            ""
        };
        return Vital::new(
            "tokengauge",
            Level::Warn,
            title,
            format!("figures from {{since}}, so its daemon has likely stopped{holds}"),
        )
        .since(at)
        .items(errors);
    }
    if !errors.is_empty() {
        return Vital::new(
            "tokengauge",
            Level::Warn,
            title,
            "figures from {since}, with errors",
        )
        .since(at)
        .items(errors);
    }
    Vital::new("tokengauge", Level::Ok, title, "figures from {since}").since(at)
}

fn gauge_errors(usage: &Value) -> Vec<String> {
    let mut errors = Vec::new();
    let providers = usage.get("providers").and_then(Value::as_object);
    for (id, p) in providers.into_iter().flatten() {
        if let Some(e) = p.get("error").and_then(Value::as_str) {
            errors.push(format!("{id}: {e}"));
        }
        let accounts = p.get("accounts").and_then(Value::as_object);
        for (name, a) in accounts.into_iter().flatten() {
            if let Some(e) = a.get("error").and_then(Value::as_str) {
                errors.push(format!("{id}/{name}: {e}"));
            }
        }
    }
    errors
}

fn sign_in_again(p: &dyn Provider, name: &str) -> String {
    if p.id() == crate::commands::DEFAULT_PROVIDER {
        format!("`remuda login {name} --force`")
    } else {
        format!("`remuda login -p {} {name} --force`", p.id())
    }
}

fn logins(c: &Checkup) -> Vital {
    let title = "Stored logins";
    let mut count = 0;
    let mut level = Level::Ok;
    let mut items = Vec::new();
    for live in c.lives {
        let p = live.provider;
        let entries = match c.store.list(p.id()) {
            Ok(entries) => entries,
            Err(e) => {
                level = Level::Bad;
                items.push(format!("{}: {e:#}", p.id()));
                continue;
            }
        };
        for e in entries {
            count += 1;
            let spec = format!("{}/{}", p.id(), e.name);
            if p.refresh_expires_at(&e.creds).is_some_and(|t| t <= c.now) {
                level = Level::Bad;
                items.push(format!(
                    "{spec}: its refresh token has expired; sign it in again: {}",
                    sign_in_again(p, &e.name)
                ));
            } else if !e.verified {
                level = level.max(Level::Warn);
                items.push(format!(
                    "{spec}: unverified, so it is not refreshed; sign it in again: {}",
                    sign_in_again(p, &e.name)
                ));
            }
        }
    }
    let detail = match (count, items.len()) {
        (0, 0) => {
            return Vital::new("logins", Level::Info, title, "none stored yet")
                .fix("`remuda import <name>` or `remuda login <name>`");
        }
        (n, 0) => format!("{n} stored, all verified"),
        (n, 1) => format!("{n} stored, 1 needs attention"),
        (n, k) => format!("{n} stored, {k} need attention"),
    };
    Vital::new("logins", level, title, detail).items(items)
}

fn live(c: &Checkup, automated: &[&str]) -> Vital {
    let title = "Signed-in CLIs";
    let mut level = Level::Ok;
    let mut items = Vec::new();
    let mut on = 0;
    for l in c.lives {
        let p = l.provider;
        let auto = automated.contains(&p.id());
        match &l.state {
            LiveState::SignedOut => {}
            LiveState::Stored { synced: true, .. } => on += 1,
            LiveState::Stored {
                name,
                synced: false,
            } => {
                on += 1;
                level = level.max(Level::Warn);
                items.push(format!(
                    "{}: on a login that could not be confirmed as {name}, so its tokens are not copied back",
                    p.name()
                ));
            }
            LiveState::Unstored { email } => {
                on += 1;
                level = level.max(if auto { Level::Warn } else { Level::Info });
                let email = email
                    .as_deref()
                    .map(|e| format!(" ({e})"))
                    .unwrap_or_default();
                let held = if auto { ", and auto mode holds" } else { "" };
                let import = if p.id() == crate::commands::DEFAULT_PROVIDER {
                    "`remuda import <name>`".to_owned()
                } else {
                    format!("`remuda import -p {} <name>`", p.id())
                };
                items.push(format!(
                    "{}: on a login remuda has not stored{email}{held}: {import}",
                    p.name()
                ));
            }
            LiveState::Foreign { what } => {
                on += 1;
                level = level.max(if auto { Level::Warn } else { Level::Info });
                items.push(format!(
                    "{}: on {what}, which remuda cannot store",
                    p.name()
                ));
            }
            LiveState::Unreadable { error } => {
                level = Level::Bad;
                items.push(format!("{}: its files cannot be read: {error}", p.name()));
            }
        }
    }
    let detail = match (on, items.is_empty()) {
        (0, true) => "no CLI is signed in".to_owned(),
        (_, true) => "each is on a stored login".to_owned(),
        (_, false) => "not all on a stored login".to_owned(),
    };
    Vital::new("live", level, title, detail).items(items)
}

fn versions(c: &Checkup) -> Vec<Vital> {
    let Some(search) = c.search else {
        return Vec::new();
    };
    c.lives
        .iter()
        .filter_map(|l| {
            let p = l.provider;
            let mirrored = p.mirrors()?;
            let bin = which(p.run_spec()?.command, search)?;
            let id = format!("version-{}", p.id());
            let title = format!("{} version", p.name());
            let Some(installed) = installed_version(&bin) else {
                return Some(Vital::new(
                    &id,
                    Level::Info,
                    &title,
                    format!("{} did not say its version", bin.display()),
                ));
            };
            Some(if installed == mirrored {
                Vital::new(
                    &id,
                    Level::Ok,
                    &title,
                    format!("{installed}, the release remuda's sign-in was read from"),
                )
            } else {
                Vital::new(
                    &id,
                    Level::Info,
                    &title,
                    format!("{installed} is installed; remuda's sign-in was read from {mirrored}"),
                )
                .fix("if signing in or refreshing fails, `remuda --update`")
            })
        })
        .collect()
}

fn which(name: &str, search: &OsStr) -> Option<PathBuf> {
    std::env::split_paths(search)
        .map(|d| d.join(name))
        .find(|p| {
            p.metadata()
                .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
}

const ASK_FOR: Duration = Duration::from_secs(5);
const KEEP_FOR: Duration = Duration::from_secs(3600);

type Asked = HashMap<PathBuf, (Option<SystemTime>, Instant, Option<String>)>;

/// Asked once per binary and kept an hour, so the page can ask on every state.
/// Kept by the path found rather than where it links: version managers link
/// every CLI to one launcher that answers by the name it was run as.
fn installed_version(bin: &Path) -> Option<String> {
    static ASKED: Mutex<Option<Asked>> = Mutex::new(None);
    let modified = bin.metadata().and_then(|m| m.modified()).ok();
    if let Some((m, at, v)) = ASKED.lock().unwrap().get_or_insert_default().get(bin)
        && *m == modified
        && at.elapsed() < KEEP_FOR
    {
        return v.clone();
    }
    let version = ask_version(bin);
    ASKED
        .lock()
        .unwrap()
        .get_or_insert_default()
        .insert(bin.to_owned(), (modified, Instant::now(), version.clone()));
    version
}

fn ask_version(bin: &Path) -> Option<String> {
    let ran = child::run("the CLI", bin, &["--version"], ASK_FOR, 4096).ok()?;
    String::from_utf8_lossy(&ran.out)
        .split_whitespace()
        .find(|w| {
            w.split('.').count() >= 2
                && w.split('.')
                    .all(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
        })
        .map(str::to_owned)
}

fn pin(c: &Checkup) -> Option<Vital> {
    let file = pins::untrusted(c.store, c.here?)?;
    Some(
        Vital::new(
            "pin",
            Level::Warn,
            "Pins here",
            format!(
                "{} is not trusted, so its pins are not read",
                file.display()
            ),
        )
        .fix("read it, then `remuda pin --allow`"),
    )
}

pub fn print(vitals: &[Vital], now: i64, json: bool, out: &mut dyn Write) -> Result<()> {
    if json {
        writeln!(out, "{}", serde_json::to_string_pretty(vitals)?)?;
    }
    let width = vitals.iter().map(|v| v.title.len()).max().unwrap_or(0);
    let pad = " ".repeat(6 + width + 2);
    for v in vitals.iter().filter(|_| !json) {
        writeln!(
            out,
            "{:<6}{:<width$}  {}",
            v.level.as_str(),
            v.title,
            v.detail_at(now)
        )?;
        for item in &v.items {
            writeln!(out, "{pad}- {item}")?;
        }
        if let Some(fix) = &v.fix {
            writeln!(out, "{pad}fix: {fix}")?;
        }
    }
    let bad = vitals.iter().filter(|v| v.level == Level::Bad).count();
    if bad > 0 {
        bail!("{bad} vital(s) failing");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fsx::{now_ms, write_json};
    use crate::ops::testing::*;
    use serde_json::json;

    struct Setup {
        units: Vec<PathBuf>,
        snapshot: PathBuf,
        config: PathBuf,
        here: PathBuf,
    }

    fn setup(e: &Env) -> Setup {
        let units = e.tmp.path().join("units");
        for (unit, wants) in [
            (units::TIMER, "timers.target.wants"),
            (units::SERVE, "default.target.wants"),
        ] {
            std::fs::create_dir_all(units.join(wants)).unwrap();
            std::fs::write(units.join(unit), "").unwrap();
            std::os::unix::fs::symlink(units.join(unit), units.join(wants).join(unit)).unwrap();
        }
        let here = e.tmp.path().join("repo");
        std::fs::create_dir_all(&here).unwrap();
        let s = Setup {
            units: vec![units],
            snapshot: e.tmp.path().join("tokengauge-usage.json"),
            config: e.tmp.path().join("config.toml"),
            here,
        };
        snapshot(&s, now_ms(), json!([]));
        runs::record(
            &e.store,
            &runs::Run {
                at: now_ms(),
                refreshed: vec!["claude/spare".into()],
                problems: vec![],
            },
        );
        s
    }

    fn snapshot(s: &Setup, at: i64, errors: Value) {
        write_json(
            &s.snapshot,
            &json!({"meta": {"updatedAtMs": at}, "payloads": [], "errors": errors}),
        )
        .unwrap();
    }

    fn check(e: &Env, s: &Setup, live: LiveState, serving: Option<&str>) -> Vec<Vital> {
        check_with(e, s, live, serving.map(|v| (v.to_owned(), true)))
    }

    fn check_with(
        e: &Env,
        s: &Setup,
        live: LiveState,
        serving: Option<(String, bool)>,
    ) -> Vec<Vital> {
        let lives = [Live {
            provider: &e.claude,
            state: live,
        }];
        vitals(&Checkup {
            store: &e.store,
            lives: &lives,
            units: &s.units,
            snapshot: &s.snapshot,
            config: &s.config,
            serving,
            search: None,
            here: Some(&s.here),
            now: now_ms(),
        })
    }

    fn on(name: &str) -> LiveState {
        LiveState::Stored {
            name: name.into(),
            synced: true,
        }
    }

    const OURS: &str = env!("CARGO_PKG_VERSION");

    fn find<'a>(vitals: &'a [Vital], id: &str) -> &'a Vital {
        vitals.iter().find(|v| v.id == id).unwrap()
    }

    #[test]
    fn a_setup_with_nothing_wrong_is_all_ok() {
        let e = env(&OFFLINE);
        e.stored("work", "u-work", HOUR);
        let s = setup(&e);
        let vitals = check(&e, &s, on("work"), Some(OURS));
        let ids: Vec<&str> = vitals.iter().map(|v| v.id.as_str()).collect();
        assert_eq!(
            ids,
            ["serve", "timer", "refresh", "tokengauge", "logins", "live"]
        );
        for v in &vitals {
            assert_eq!(v.level, Level::Ok, "{v:?}");
        }
        let mut out = Vec::new();
        print(&vitals, now_ms(), false, &mut out).unwrap();
        let out = String::from_utf8(out).unwrap();
        assert!(
            out.contains("ok    Last refresh    under a minute ago, refreshed 1 login"),
            "{out}"
        );
    }

    #[test]
    fn serve_down_is_bad_only_when_something_runs_inside_it() {
        let e = env(&OFFLINE);
        let s = setup(&e);
        let serve = check(&e, &s, LiveState::SignedOut, None);
        assert_eq!(find(&serve, "serve").level, Level::Info);

        std::fs::write(
            &s.config,
            "[auto.claude]\nenabled = true\n\n[warm]\nclaude = [\"work\"]\n",
        )
        .unwrap();
        let vitals = check(&e, &s, LiveState::SignedOut, None);
        let serve = find(&vitals, "serve");
        assert_eq!(serve.level, Level::Bad);
        assert_eq!(
            serve.detail,
            "not running, so auto mode and keep-warm do nothing"
        );
        assert_eq!(
            serve.fix.as_deref(),
            Some("`systemctl --user enable --now remuda-serve.service`")
        );

        let mut out = Vec::new();
        let err = print(&vitals, now_ms(), false, &mut out).unwrap_err();
        assert_eq!(err.to_string(), "1 vital(s) failing");
    }

    #[test]
    fn a_serve_of_another_version_says_which_side_to_bring_up_to_date() {
        let e = env(&OFFLINE);
        let s = setup(&e);
        let older = find(&check(&e, &s, LiveState::SignedOut, Some("0.0.1")), "serve").clone();
        assert_eq!(older.level, Level::Warn);
        assert!(older.fix.unwrap().contains("restart remuda-serve.service"));
        let newer = find(
            &check(&e, &s, LiveState::SignedOut, Some("999.0.0")),
            "serve",
        )
        .clone();
        assert!(newer.fix.unwrap().contains("remuda --update"));
    }

    #[test]
    fn a_serve_by_hand_is_noted_and_warned_about_once_auto_mode_needs_it() {
        let e = env(&OFFLINE);
        let s = setup(&e);
        let by_hand = || {
            let vitals = check_with(&e, &s, LiveState::SignedOut, Some((OURS.into(), false)));
            find(&vitals, "serve").clone()
        };
        assert_eq!(by_hand().level, Level::Info);
        assert_eq!(
            by_hand().fix.as_deref(),
            Some("`systemctl --user enable --now remuda-serve.service`")
        );
        std::fs::write(&s.config, "[auto.claude]\nenabled = true\n").unwrap();
        assert_eq!(by_hand().level, Level::Warn);
    }

    #[test]
    fn a_config_that_cannot_be_read_is_bad() {
        let e = env(&OFFLINE);
        let s = setup(&e);
        std::fs::write(&s.config, "[auto.claude]\nnope = 1\n").unwrap();
        let vitals = check(&e, &s, LiveState::SignedOut, Some(OURS));
        assert_eq!(find(&vitals, "config").level, Level::Bad);
    }

    #[test]
    fn the_timer_says_how_to_turn_it_on() {
        let e = env(&OFFLINE);
        let s = setup(&e);
        std::fs::remove_file(s.units[0].join("timers.target.wants").join(units::TIMER)).unwrap();
        let timer = find(&check(&e, &s, LiveState::SignedOut, Some(OURS)), "timer").clone();
        assert_eq!(timer.level, Level::Warn);
        assert_eq!(
            timer.fix.as_deref(),
            Some("`systemctl --user enable --now remuda-refresh.timer`")
        );
    }

    #[test]
    fn a_refresh_with_problems_lists_them_and_a_late_one_warns() {
        let e = env(&OFFLINE);
        let s = setup(&e);
        let at = now_ms() - 2 * HOUR;
        runs::record(
            &e.store,
            &runs::Run {
                at,
                refreshed: vec![],
                problems: vec!["claude/perso: invalid_grant".into()],
            },
        );
        let vitals = check(&e, &s, LiveState::SignedOut, Some(OURS));
        let refresh = find(&vitals, "refresh");
        assert_eq!(refresh.level, Level::Bad);
        assert_eq!(refresh.since, Some(at));
        assert_eq!(
            refresh.detail_at(now_ms()),
            "2 hours ago, refreshed 0 logins, 1 problem"
        );
        assert_eq!(refresh.items, ["claude/perso: invalid_grant"]);

        runs::record(
            &e.store,
            &runs::Run {
                at,
                refreshed: vec![],
                problems: vec![],
            },
        );
        let refresh = find(&check(&e, &s, LiveState::SignedOut, Some(OURS)), "refresh").clone();
        assert_eq!(refresh.level, Level::Warn);
    }

    #[test]
    fn a_stale_snapshot_says_auto_mode_holds_and_errors_are_listed() {
        let e = env(&OFFLINE);
        let s = setup(&e);
        std::fs::write(&s.config, "[auto.claude]\nenabled = true\n").unwrap();
        snapshot(
            &s,
            now_ms() - HOUR,
            json!([{"provider": "claude", "credential": "work", "message": "401 Unauthorized"}]),
        );
        let vitals = check(&e, &s, LiveState::SignedOut, Some(OURS));
        let gauge = find(&vitals, "tokengauge");
        assert_eq!(gauge.level, Level::Warn);
        assert_eq!(
            gauge.detail_at(now_ms()),
            "figures from 1 hour ago, so its daemon has likely stopped: auto mode and keep-warm hold until it fetches again"
        );
        assert_eq!(gauge.items, ["claude/work: 401 Unauthorized"]);

        std::fs::remove_file(&s.snapshot).unwrap();
        let gauge = find(
            &check(&e, &s, LiveState::SignedOut, Some(OURS)),
            "tokengauge",
        )
        .clone();
        assert_eq!(gauge.level, Level::Warn);
        std::fs::write(&s.config, "").unwrap();
        let gauge = find(
            &check(&e, &s, LiveState::SignedOut, Some(OURS)),
            "tokengauge",
        )
        .clone();
        assert_eq!(gauge.level, Level::Info);
    }

    #[test]
    fn logins_to_sign_in_again_are_named_with_the_command() {
        let e = env(&OFFLINE);
        e.stored("work", "u-work", HOUR);
        e.stored("gone", "u-gone", HOUR);
        let mut gone = e.store.get("claude", "gone").unwrap().unwrap();
        gone.creds[crate::claude::OAUTH_KEY]["refreshTokenExpiresAt"] = json!(now_ms() - HOUR);
        e.store.save(&gone).unwrap();
        write_json(
            &e.store.root().join("claude/work.json"),
            &oauth("a-torn", "r-torn", HOUR),
        )
        .unwrap();
        let s = setup(&e);
        let vitals = check(&e, &s, LiveState::SignedOut, Some(OURS));
        let logins = find(&vitals, "logins");
        assert_eq!(logins.level, Level::Bad);
        assert_eq!(logins.detail, "2 stored, 2 need attention");
        assert_eq!(
            logins.items,
            [
                "claude/gone: its refresh token has expired; sign it in again: `remuda login gone --force`",
                "claude/work: unverified, so it is not refreshed; sign it in again: `remuda login work --force`",
            ]
        );
    }

    #[test]
    fn an_unstored_live_login_matters_once_auto_mode_is_on() {
        let e = env(&OFFLINE);
        let s = setup(&e);
        let unstored = || LiveState::Unstored {
            email: Some("x@example.com".into()),
        };
        let live = find(&check(&e, &s, unstored(), Some(OURS)), "live").clone();
        assert_eq!(live.level, Level::Info);
        assert_eq!(
            live.items,
            [
                "Claude Code: on a login remuda has not stored (x@example.com): `remuda import <name>`"
            ]
        );
        std::fs::write(&s.config, "[auto.claude]\nenabled = true\n").unwrap();
        let live = find(&check(&e, &s, unstored(), Some(OURS)), "live").clone();
        assert_eq!(live.level, Level::Warn);
        let unreadable = LiveState::Unreadable {
            error: "bad json".into(),
        };
        let live = find(&check(&e, &s, unreadable, Some(OURS)), "live").clone();
        assert_eq!(live.level, Level::Bad);
    }

    fn fake_cli(dir: &Path, says: &str) -> std::ffi::OsString {
        std::fs::create_dir_all(dir).unwrap();
        let script = dir.join("claude");
        std::fs::write(&script, format!("#!/bin/sh\necho '{says}'\n")).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        dir.as_os_str().to_owned()
    }

    #[test]
    fn the_installed_cli_is_compared_with_the_release_sign_in_was_read_from() {
        let e = env(&OFFLINE);
        let s = setup(&e);
        let version = |search: &OsStr| {
            let lives = [Live {
                provider: &e.claude,
                state: LiveState::SignedOut,
            }];
            versions(&Checkup {
                store: &e.store,
                lives: &lives,
                units: &s.units,
                snapshot: &s.snapshot,
                config: &s.config,
                serving: None,
                search: Some(search),
                here: None,
                now: now_ms(),
            })
        };
        let same = fake_cli(&e.tmp.path().join("same"), "2.1.282 (Claude Code)");
        let same = version(&same);
        assert_eq!(same.len(), 1);
        assert_eq!(same[0].id, "version-claude");
        assert_eq!(same[0].level, Level::Ok);

        let newer = fake_cli(&e.tmp.path().join("newer"), "2.1.314 (Claude Code)");
        let newer = version(&newer);
        assert_eq!(newer[0].level, Level::Info);
        assert_eq!(
            newer[0].detail,
            "2.1.314 is installed; remuda's sign-in was read from 2.1.282"
        );

        let mute = fake_cli(&e.tmp.path().join("mute"), "hello");
        assert!(
            version(&mute)[0]
                .detail
                .ends_with("did not say its version")
        );

        assert!(version(e.tmp.path().join("nowhere").as_os_str()).is_empty());
    }

    #[test]
    fn clis_that_link_to_one_launcher_each_say_their_own_version() {
        let tmp = tempfile::tempdir().unwrap();
        let launcher = tmp.path().join("launcher");
        std::fs::write(
            &launcher,
            "#!/bin/sh\ncase \"${0##*/}\" in claude) echo '2.1.314 (Claude Code)';; *) echo 'codex-cli 0.166.0';; esac\n",
        )
        .unwrap();
        std::fs::set_permissions(&launcher, std::fs::Permissions::from_mode(0o755)).unwrap();
        for name in ["claude", "codex"] {
            std::os::unix::fs::symlink(&launcher, tmp.path().join(name)).unwrap();
        }
        assert_eq!(
            installed_version(&tmp.path().join("claude")).as_deref(),
            Some("2.1.314")
        );
        assert_eq!(
            installed_version(&tmp.path().join("codex")).as_deref(),
            Some("0.166.0")
        );
    }

    #[test]
    fn an_untrusted_pin_file_here_is_reported_until_allowed() {
        let e = env(&OFFLINE);
        let s = setup(&e);
        std::fs::write(s.here.join(pins::FILE), "claude = \"work\"\n").unwrap();
        let vitals = check(&e, &s, LiveState::SignedOut, Some(OURS));
        let pin = find(&vitals, "pin");
        assert_eq!(pin.level, Level::Warn);
        assert_eq!(
            pin.fix.as_deref(),
            Some("read it, then `remuda pin --allow`")
        );
        pins::allow_nearest(&e.store, &s.here).unwrap();
        let vitals = check(&e, &s, LiveState::SignedOut, Some(OURS));
        assert!(vitals.iter().all(|v| v.id != "pin"));
    }

    #[test]
    fn ago_reads_as_a_person_would_say_it() {
        assert_eq!(ago(10_000), "under a minute ago");
        assert_eq!(ago(MIN), "1 minute ago");
        assert_eq!(ago(90 * MIN), "1 hour ago");
        assert_eq!(ago(47 * 60 * MIN), "1 day ago");
        assert_eq!(ago(72 * 60 * MIN), "3 days ago");
    }
}
