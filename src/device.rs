use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde_json::Value;

use crate::oauth;
use crate::provider::{Login, PendingLogin};

const MAX_WAIT: Duration = Duration::from_secs(15 * 60);

/// Where a device sign-in is started and finished, and what the finished
/// tokens become.
pub struct DeviceFlow {
    pub client: reqwest::blocking::Client,
    pub device_url: String,
    pub token_url: String,
    pub client_id: String,
    pub scope: Option<String>,
    pub headers: Vec<(&'static str, String)>,
    /// The token endpoint's answer, as the login to store.
    pub into_login: Box<dyn FnOnce(Value) -> Result<Login> + Send>,
}

/// An RFC 8628 sign-in: the user approves a code in the browser while this
/// side polls the token endpoint. Nothing listens, so no port is needed.
pub struct DevicePending {
    flow: DeviceFlow,
    device_code: String,
    url: String,
    interval: Duration,
    deadline: Instant,
    cancelled: Arc<AtomicBool>,
}

impl DeviceFlow {
    fn post(&self, url: &str, form: &[(&str, &str)]) -> reqwest::blocking::RequestBuilder {
        let mut req = self.client.post(url).form(form);
        for (name, value) in &self.headers {
            req = req.header(*name, value);
        }
        req
    }

    pub fn begin(self) -> Result<Box<dyn PendingLogin>> {
        let mut form = vec![("client_id", self.client_id.as_str())];
        if let Some(scope) = &self.scope {
            form.push(("scope", scope));
        }
        let resp = self
            .post(&self.device_url, &form)
            .send()
            .context("device authorization request failed")?;
        let status = resp.status();
        let body = resp.text().unwrap_or_default();
        if !status.is_success() {
            bail!(
                "device authorization answered {status}: {}",
                body.chars().take(200).collect::<String>()
            );
        }
        let answer: Value =
            serde_json::from_str(&body).context("malformed device authorization response")?;
        let device_code =
            oauth::text(&answer, "device_code").context("no device code was issued")?;
        let url = oauth::text(&answer, "verification_uri_complete")
            .or_else(|| {
                let base = oauth::text(&answer, "verification_uri")?;
                let code = oauth::text(&answer, "user_code")?;
                let sep = if base.contains('?') { '&' } else { '?' };
                Some(format!("{base}{sep}user_code={code}"))
            })
            .context("no verification address was issued")?;
        let secs = |k: &str, default: u64| answer.get(k).and_then(Value::as_u64).unwrap_or(default);
        let interval = Duration::from_secs(secs("interval", 5).max(1));
        let deadline = Instant::now() + Duration::from_secs(secs("expires_in", 600)).min(MAX_WAIT);
        Ok(Box::new(DevicePending {
            flow: self,
            device_code,
            url,
            interval,
            deadline,
            cancelled: Arc::new(AtomicBool::new(false)),
        }))
    }
}

impl DevicePending {
    /// Sleeps `d` in short steps so a cancel lands promptly.
    fn wait(&self, d: Duration) -> Result<()> {
        let until = Instant::now() + d;
        while Instant::now() < until {
            if self.cancelled.load(Ordering::Relaxed) {
                bail!("the sign-in was cancelled");
            }
            std::thread::sleep(Duration::from_millis(100).min(until - Instant::now()));
        }
        Ok(())
    }

    fn poll(&mut self) -> Result<Value> {
        loop {
            self.wait(self.interval)?;
            if Instant::now() >= self.deadline {
                bail!("the sign-in was not approved in time");
            }
            let resp = self
                .flow
                .post(
                    &self.flow.token_url,
                    &[
                        ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                        ("device_code", &self.device_code),
                        ("client_id", &self.flow.client_id),
                    ],
                )
                .send()
                .context("token request failed")?;
            let status = resp.status();
            let body = resp.text().unwrap_or_default();
            let answer: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
            if status.is_success() {
                return Ok(answer);
            }
            match oauth::text(&answer, "error").as_deref() {
                Some("authorization_pending") => {}
                Some("slow_down") => self.interval += Duration::from_secs(5),
                Some("access_denied") => bail!("the sign-in was refused"),
                Some("expired_token") => bail!("the sign-in was not approved in time"),
                _ => bail!(
                    "token endpoint answered {status}: {}",
                    body.chars().take(200).collect::<String>()
                ),
            }
        }
    }
}

impl PendingLogin for DevicePending {
    fn url(&self) -> &str {
        &self.url
    }

    fn needs_code(&self) -> bool {
        false
    }

    fn canceller(&self) -> Box<dyn Fn() + Send + Sync> {
        let cancelled = Arc::clone(&self.cancelled);
        Box::new(move || cancelled.store(true, Ordering::Relaxed))
    }

    fn finish(mut self: Box<Self>, _code: Option<&str>) -> Result<Login> {
        let answer = self.poll()?;
        oauth::text(&answer, "access_token").context("the token endpoint sent no access token")?;
        let DevicePending { flow, .. } = *self;
        (flow.into_login)(answer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::Identity;
    use mockito::Matcher;
    use serde_json::json;

    fn flow(base: &str) -> DeviceFlow {
        DeviceFlow {
            client: oauth::client().unwrap(),
            device_url: format!("{base}/device"),
            token_url: format!("{base}/token"),
            client_id: "cid".into(),
            scope: Some("openid".into()),
            headers: vec![("x-test", "1".into())],
            into_login: Box::new(|answer| {
                Ok(Login {
                    identity: Identity {
                        account_id: "u".into(),
                        email: String::new(),
                        oauth_account: None,
                    },
                    creds: answer,
                })
            }),
        }
    }

    #[test]
    fn a_device_login_polls_until_approved() {
        let mut server = mockito::Server::new();
        let device = server
            .mock("POST", "/device")
            .match_header("x-test", "1")
            .match_body(Matcher::AllOf(vec![
                Matcher::UrlEncoded("client_id".into(), "cid".into()),
                Matcher::UrlEncoded("scope".into(), "openid".into()),
            ]))
            .with_body(
                json!({"device_code": "dc", "user_code": "ABCD", "verification_uri": "https://x/verify", "interval": 1})
                    .to_string(),
            )
            .create();
        let pending = server
            .mock("POST", "/token")
            .with_status(400)
            .with_body(r#"{"error":"authorization_pending"}"#)
            .expect_at_least(1)
            .create();
        let login = flow(&server.url()).begin().unwrap();
        assert_eq!(login.url(), "https://x/verify?user_code=ABCD");
        assert!(!login.needs_code());
        device.assert();

        let finished = std::thread::spawn(move || login.finish(None));
        std::thread::sleep(Duration::from_millis(1500));
        pending.assert();
        pending.remove();
        server
            .mock("POST", "/token")
            .match_body(Matcher::AllOf(vec![
                Matcher::UrlEncoded(
                    "grant_type".into(),
                    "urn:ietf:params:oauth:grant-type:device_code".into(),
                ),
                Matcher::UrlEncoded("device_code".into(), "dc".into()),
            ]))
            .with_body(r#"{"access_token":"at","refresh_token":"rt"}"#)
            .create();
        let done = finished.join().unwrap().unwrap();
        assert_eq!(done.creds["refresh_token"], "rt");
    }

    #[test]
    fn a_refused_device_login_says_so() {
        let mut server = mockito::Server::new();
        server
            .mock("POST", "/device")
            .with_body(
                json!({"device_code": "dc", "verification_uri_complete": "https://x/v?c=1", "interval": 1})
                    .to_string(),
            )
            .create();
        server
            .mock("POST", "/token")
            .with_status(400)
            .with_body(r#"{"error":"access_denied"}"#)
            .create();
        let login = flow(&server.url()).begin().unwrap();
        assert_eq!(login.url(), "https://x/v?c=1");
        let err = login.finish(None).err().unwrap();
        assert!(err.to_string().contains("refused"), "{err}");
    }

    #[test]
    fn a_device_login_can_be_cancelled() {
        let mut server = mockito::Server::new();
        server
            .mock("POST", "/device")
            .with_body(json!({"device_code": "dc", "verification_uri_complete": "https://x", "interval": 30}).to_string())
            .create();
        let login = flow(&server.url()).begin().unwrap();
        let cancel = login.canceller();
        let started = Instant::now();
        let finished = std::thread::spawn(move || login.finish(None));
        std::thread::sleep(Duration::from_millis(200));
        cancel();
        let err = finished.join().unwrap().err().unwrap();
        assert!(err.to_string().contains("cancelled"), "{err}");
        assert!(started.elapsed() < Duration::from_secs(5));
    }
}
