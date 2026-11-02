//! The page's Usage tab: what was spent, from TokenGauge, and what the proxy
//! answered, from its request log. remuda prices nothing itself: the spend is
//! `tokengauge --usage-json` as TokenGauge printed it, so the page and the
//! panel can never price a token two ways.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use selvedge::usage::export::{self, UsageExport};
use selvedge::usage::requests::{self, Outcome};
use selvedge::usage::{TokenCounts, account_digest};
use serde::Serialize;
use serde_json::{Value, json};

use crate::provider::Provider;
use crate::store::Store;

/// How long TokenGauge gets to print its export, and how much of it is read.
const WAIT: Duration = Duration::from_secs(10);
const MAX_EXPORT: u64 = 32 << 20;

/// The first local day of a range `days` long that ends today.
fn first_day(days: u32) -> String {
    let today = jiff::Zoned::now().date();
    today
        .checked_sub(jiff::Span::new().days(i64::from(days.max(1)) - 1))
        .unwrap_or(today)
        .to_string()
}

/// Every stored login by the digest TokenGauge names it with.
pub fn logins(store: &Store, providers: &[&dyn Provider]) -> Value {
    let mut out = serde_json::Map::new();
    for p in providers {
        for entry in store.list(p.id()).unwrap_or_default() {
            out.insert(
                account_digest(&entry.meta.account_id),
                json!({"provider": p.id(), "name": entry.name, "label": entry.meta.label}),
            );
        }
    }
    Value::Object(out)
}

/// `tokengauge --usage-json --since <first day>`, parsed.
pub fn spend(tokengauge: Option<&Path>, days: u32) -> Result<UsageExport> {
    let program = tokengauge.context("TokenGauge is not installed")?;
    let mut child = Command::new(program)
        .args([export::FLAG, export::SINCE, &first_day(days)])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("could not run {}", program.display()))?;
    let out = drain(child.stdout.take().context("no output")?, MAX_EXPORT);
    let err = drain(child.stderr.take().context("no error output")?, 4096);
    let until = Instant::now() + WAIT;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= until {
            let _ = child.kill();
            let _ = child.wait();
            bail!("TokenGauge took longer than {}s to answer", WAIT.as_secs());
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    // A process TokenGauge left behind can hold the pipes open after it exits,
    // so reading them to their end shares the same deadline.
    let left = || until.saturating_duration_since(Instant::now());
    let late = || anyhow::anyhow!("TokenGauge's output did not end within {}s", WAIT.as_secs());
    let bytes = out.recv_timeout(left()).map_err(|_| late())?;
    if !status.success() {
        let said = err.recv_timeout(left()).unwrap_or_default();
        let said = String::from_utf8_lossy(&said);
        let said = said.trim();
        bail!(
            "TokenGauge could not export its usage{}",
            if said.contains("--usage-json") || said.is_empty() {
                ": it needs 0.52.0 or later".to_owned()
            } else {
                format!(": {said}")
            }
        );
    }
    Ok(UsageExport::parse(&bytes)?)
}

/// Everything `pipe` holds, up to `limit` bytes, once it ends.
fn drain(pipe: impl Read + Send + 'static, limit: u64) -> mpsc::Receiver<Vec<u8>> {
    let (send, receive) = mpsc::channel();
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = pipe.take(limit).read_to_end(&mut bytes);
        let _ = send.send(bytes);
    });
    receive
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct Group {
    requests: u64,
    tokens: TokenCounts,
    reasoning: u64,
    latency_ms: u64,
    /// The time from the first byte to the last, over the answers that had
    /// both, and the output tokens those answers wrote.
    generating_ms: u64,
    generated: u64,
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Key {
    at: i64,
    provider: String,
    model: String,
    login: Option<String>,
    key: Option<String>,
    outcome: &'static str,
}

/// Every time zone sits a whole number of quarter hours from UTC, so no
/// bucket this wide crosses a local midnight the page splits days at.
const BUCKET_MS: i64 = 900_000;

/// The proxy's requests over the last `days`, summed per quarter hour, provider,
/// model, login, key and outcome: small enough for the page to filter and
/// sum again however it is asked.
pub fn proxied(store: &Store, keys: &BTreeMap<String, String>, days: u32) -> Result<Value> {
    let since = jiff::Zoned::now()
        .start_of_day()?
        .checked_sub(jiff::Span::new().days(i64::from(days.max(1)) - 1))?
        .timestamp()
        .as_millisecond();
    let mut groups: BTreeMap<Key, Group> = BTreeMap::new();
    for r in requests::read(store.root())? {
        if r.at < since {
            continue;
        }
        let key = Key {
            at: r.at - r.at.rem_euclid(BUCKET_MS),
            provider: r.provider.clone(),
            model: r.model.clone(),
            login: r.name.clone(),
            key: r
                .key
                .as_ref()
                .map(|id| keys.get(id).cloned().unwrap_or_else(|| id.clone())),
            outcome: match r.outcome {
                Outcome::Ok => "ok",
                Outcome::Failed => "failed",
                Outcome::Canceled => "canceled",
            },
        };
        let g = groups.entry(key).or_default();
        g.requests += 1;
        g.tokens.add(&r.tokens);
        g.reasoning += r.reasoning;
        g.latency_ms += r.latency_ms;
        if let Some(first) = r.first_byte_ms.filter(|_| r.tokens.output > 0) {
            g.generating_ms += r.latency_ms.saturating_sub(first);
            g.generated += r.tokens.output;
        }
    }
    let rows: Vec<Value> = groups
        .into_iter()
        .map(|(k, g)| {
            let mut row = serde_json::to_value(g).unwrap_or_default();
            row["at"] = json!(k.at);
            row["provider"] = json!(k.provider);
            row["model"] = json!(k.model);
            row["login"] = json!(k.login);
            row["key"] = json!(k.key);
            row["outcome"] = json!(k.outcome);
            row
        })
        .collect();
    Ok(json!({ "since": since, "rows": rows }))
}
