use std::time::{Duration, Instant};

use selvedge::usage::TokenCounts;
use selvedge::usage::requests::{Outcome, Request};

use super::translate::Seen;
use crate::provider::Format;

/// What one request went through, gathered as it is forwarded and written to
/// the request log once it is over (`selvedge::usage::requests`).
pub struct Trace {
    pub at: i64,
    pub started: Instant,
    pub key: Option<String>,
    pub format: Format,
    pub stream: bool,
    pub provider: Option<&'static str>,
    pub model: String,
    /// The served login's account id and store name.
    pub login: Option<(String, String)>,
    pub status: u16,
    pub first_byte: Option<Duration>,
    pub request_id: Option<String>,
    pub seen: Seen,
    pub canceled: bool,
}

impl Trace {
    pub fn new(at: i64, started: Instant, key: Option<String>, format: Format) -> Self {
        Trace {
            at,
            started,
            key,
            format,
            stream: false,
            provider: None,
            model: String::new(),
            login: None,
            status: 0,
            first_byte: None,
            request_id: None,
            seen: Seen::default(),
            canceled: false,
        }
    }

    /// The log line, or `None` for a request no provider was picked for.
    pub fn request(self, refused: Option<u16>) -> Option<Request> {
        let provider = self.provider?;
        let status = refused.unwrap_or(self.status);
        let outcome = if self.canceled {
            Outcome::Canceled
        } else if refused.is_some() || status >= 400 || self.seen.failed {
            Outcome::Failed
        } else {
            Outcome::Ok
        };
        let usage = self.seen.usage.unwrap_or_default();
        let (account_id, name) = self.login.unzip();
        Some(Request {
            at: self.at,
            provider: provider.to_owned(),
            model: self.model,
            account_id,
            name,
            key: self.key,
            format: format_name(self.format).to_owned(),
            stream: self.stream,
            outcome,
            status,
            first_byte_ms: self.first_byte.map(millis),
            latency_ms: millis(self.started.elapsed()),
            tokens: TokenCounts {
                input: usage
                    .input
                    .saturating_sub(usage.cache_read + usage.cache_write),
                output: usage.output,
                cache_write_5m: usage.cache_write,
                cache_write_1h: 0,
                cache_read: usage.cache_read,
            },
            reasoning: usage.reasoning,
            message_id: self.seen.id,
            request_id: self.request_id,
            speed: None,
        })
    }
}

fn format_name(format: Format) -> &'static str {
    match format {
        Format::Messages => "messages",
        Format::Chat => "chat",
        Format::Responses => "responses",
    }
}

fn millis(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}
