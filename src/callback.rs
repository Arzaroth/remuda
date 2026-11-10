//! The local address a browser is sent back to once a sign-in is approved.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde_json::{Map, Value, json};

const WAIT: Duration = Duration::from_secs(10 * 60);

pub struct Callback {
    listeners: Vec<TcpListener>,
    path: &'static str,
    state: String,
    cancelled: Arc<AtomicBool>,
}

impl Callback {
    /// 127.0.0.1 and, when the machine has it, [::1]: the redirect names
    /// `localhost`, which a browser may resolve to either, and a port left
    /// free on one of them is a port another local user could answer on. A
    /// port still held by an attempt that is shutting down gets a moment to be
    /// released. `busy` says what probably holds a port that stays taken.
    pub fn bind(port: u16, path: &'static str, state: String, busy: &str) -> Result<Self> {
        let deadline = Instant::now() + Duration::from_secs(2);
        let v4 = loop {
            match TcpListener::bind(("127.0.0.1", port)) {
                Ok(listener) => break listener,
                Err(e)
                    if e.kind() == std::io::ErrorKind::AddrInUse && Instant::now() < deadline =>
                {
                    std::thread::sleep(Duration::from_millis(100));
                }
                Err(e) if port == 0 => {
                    return Err(e)
                        .context("could not open a local port for the browser to call back to");
                }
                Err(e) => {
                    return Err(e).with_context(|| format!("port {port} is taken; {busy}"));
                }
            }
        };
        let port = v4.local_addr()?.port();
        let mut listeners = vec![v4];
        if let Ok(v6) = TcpListener::bind(("::1", port)) {
            listeners.push(v6);
        }
        for listener in &listeners {
            listener.set_nonblocking(true)?;
        }
        Ok(Callback {
            listeners,
            path,
            state,
            cancelled: Arc::new(AtomicBool::new(false)),
        })
    }

    pub fn port(&self) -> u16 {
        self.listeners[0]
            .local_addr()
            .map(|a| a.port())
            .unwrap_or_default()
    }

    pub fn canceller(&self) -> Box<dyn Fn() + Send + Sync> {
        let cancelled = Arc::clone(&self.cancelled);
        Box::new(move || cancelled.store(true, Ordering::Relaxed))
    }

    /// The query of the first request to the path carrying this attempt's
    /// state. Anything else on the port, whether another path, a stale or
    /// forged state, or a connection that never sends a request, is answered
    /// or dropped and the wait goes on.
    pub fn wait(&self) -> Result<(TcpStream, Map<String, Value>)> {
        let deadline = Instant::now() + WAIT;
        loop {
            let mut accepted = false;
            for listener in &self.listeners {
                let stream = match listener.accept() {
                    Ok((stream, _)) => stream,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => continue,
                    Err(e) => return Err(e).context("callback listener failed"),
                };
                accepted = true;
                let Some(url) = read_request(&stream) else {
                    continue;
                };
                if url.path() != self.path {
                    respond(&stream, "404 Not Found", &page("Not found."));
                    continue;
                }
                let query: Map<String, Value> = url
                    .query_pairs()
                    .map(|(k, v)| (k.into_owned(), json!(v.into_owned())))
                    .collect();
                if query.get("state").and_then(Value::as_str) != Some(self.state.as_str()) {
                    respond(
                        &stream,
                        "400 Bad Request",
                        &page("This is not the sign-in remuda is waiting for."),
                    );
                    continue;
                }
                return Ok((stream, query));
            }
            if self.cancelled.load(Ordering::Relaxed) {
                bail!("the sign-in was cancelled");
            }
            if Instant::now() > deadline {
                bail!("gave up waiting for the browser to finish signing in");
            }
            if accepted {
                continue;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

fn read_request(stream: &TcpStream) -> Option<reqwest::Url> {
    stream.set_nonblocking(false).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(2))).ok()?;
    let mut line = String::new();
    BufReader::new(std::io::Read::take(stream, 8192))
        .read_line(&mut line)
        .ok()?;
    let target = line.split_whitespace().nth(1)?;
    reqwest::Url::parse(&format!("http://localhost{target}")).ok()
}

pub fn respond(mut stream: &TcpStream, status: &str, body: &str) {
    let _ = write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}

pub fn redirect(mut stream: &TcpStream, location: &str) {
    let _ = write!(
        stream,
        "HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    );
}

pub fn page(message: &str) -> String {
    let message = message
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    format!(
        "<!doctype html><meta charset=utf-8><title>remuda</title><body style=\"font:16px system-ui;margin:3rem\"><p>{message}</p>"
    )
}
