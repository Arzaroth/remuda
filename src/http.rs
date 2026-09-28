use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

use crate::serve::{App, Request, Response};

const MAX_HEAD: usize = 16 * 1024;
const MAX_BODY: usize = 64 * 1024;

#[derive(Clone, Copy)]
pub struct Limits {
    /// How long a client gets to send its request line, headers and body.
    pub read_for: Duration,
    /// Connections handled at once; more are answered 503 and closed.
    pub connections: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            read_for: Duration::from_secs(5),
            connections: 32,
        }
    }
}

pub fn bind(port: u16) -> Result<(TcpListener, u16)> {
    let listener = TcpListener::bind(("127.0.0.1", port))
        .with_context(|| format!("cannot listen on 127.0.0.1:{port}; pick another --port"))?;
    let port = listener.local_addr()?.port();
    Ok((listener, port))
}

struct Slot(Arc<AtomicUsize>);

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

pub fn serve(app: Arc<App>, listener: TcpListener, limits: Limits) {
    let busy = Arc::new(AtomicUsize::new(0));
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        if busy.fetch_add(1, Ordering::SeqCst) >= limits.connections {
            busy.fetch_sub(1, Ordering::SeqCst);
            // On the accepting thread: only what has already arrived is
            // drained, so no client can hold this loop.
            close_with(stream, &error(503, "busy"), Duration::ZERO);
            continue;
        }
        let slot = Slot(Arc::clone(&busy));
        let app = Arc::clone(&app);
        // A thread that cannot be started is a refused connection, not a
        // stopped server.
        let _ = std::thread::Builder::new().spawn(move || {
            let _slot = slot;
            answer(&app, stream, limits);
        });
    }
}

fn error(status: u16, message: &str) -> Response {
    Response::json(status, serde_json::json!({ "error": message }))
}

/// How long an answered connection is drained, all reads together.
const LINGER: Duration = Duration::from_millis(300);

/// Answers without reading the rest of the request. Closing with that still
/// unread resets the connection and the client never sees the answer, so
/// whatever arrives in the next moment is read and dropped.
fn close_with(mut stream: TcpStream, response: &Response, linger: Duration) {
    let _ = stream.set_write_timeout(Some(Duration::from_secs(1)));
    let _ = write_response(&mut stream, response);
    let _ = stream.shutdown(std::net::Shutdown::Write);
    let until = Instant::now() + linger;
    let mut sink = [0u8; 4096];
    let mut drained = 0;
    if linger.is_zero() {
        let _ = stream.set_nonblocking(true);
    }
    while drained < MAX_BODY {
        if !linger.is_zero() {
            let Some(left) = until.checked_duration_since(Instant::now()) else {
                break;
            };
            if stream.set_read_timeout(Some(left)).is_err() {
                break;
            }
        }
        match stream.read(&mut sink) {
            Ok(0) | Err(_) => break,
            Ok(n) => drained += n,
        }
    }
}

struct Head {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    /// Bytes past the blank line: the start of the body.
    rest: Vec<u8>,
}

impl Head {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

fn read_head(stream: &mut TcpStream, deadline: Instant) -> Option<Head> {
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    let end = loop {
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i;
        }
        if buf.len() > MAX_HEAD {
            return None;
        }
        let left = deadline.checked_duration_since(Instant::now())?;
        stream.set_read_timeout(Some(left)).ok()?;
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => return None,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    };
    let text = std::str::from_utf8(&buf[..end]).ok()?;
    let mut lines = text.split("\r\n");
    let mut request = lines.next()?.split(' ');
    let method = request.next()?.to_owned();
    let target = request.next()?;
    let path = target.split('?').next()?.to_owned();
    let headers = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned()))
        .collect();
    Some(Head {
        method,
        path,
        headers,
        rest: buf[end + 4..].to_vec(),
    })
}

fn read_body(stream: &mut TcpStream, head: &Head, deadline: Instant) -> Result<String, u16> {
    let length: usize = match head.header("Content-Length") {
        Some(v) => v.parse().map_err(|_| 400u16)?,
        None => 0,
    };
    if length > MAX_BODY {
        return Err(413);
    }
    let mut body = head.rest.clone();
    body.truncate(length);
    let mut chunk = [0u8; 4096];
    while body.len() < length {
        let left = deadline
            .checked_duration_since(Instant::now())
            .ok_or(408u16)?;
        stream.set_read_timeout(Some(left)).map_err(|_| 408u16)?;
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => return Err(408),
            Ok(n) => body.extend_from_slice(&chunk[..n.min(length - body.len())]),
        }
    }
    String::from_utf8(body).map_err(|_| 400)
}

fn request<'a>(head: &'a Head, body: &'a str) -> Request<'a> {
    Request {
        method: &head.method,
        path: &head.path,
        host: head.header("Host"),
        origin: head.header("Origin"),
        token: head.header("X-Remuda-Token"),
        body,
    }
}

fn answer(app: &App, mut stream: TcpStream, limits: Limits) {
    let deadline = Instant::now() + limits.read_for;
    let Some(head) = read_head(&mut stream, deadline) else {
        close_with(stream, &error(400, "bad request"), LINGER);
        return;
    };
    // Nobody may make remuda wait on a body before showing it may ask.
    if let Some(refused) = app.check(&request(&head, "")) {
        close_with(stream, &refused, LINGER);
        return;
    }
    if head.header("Transfer-Encoding").is_some() {
        close_with(stream, &error(411, "send a Content-Length"), LINGER);
        return;
    }
    let body = match read_body(&mut stream, &head, deadline) {
        Ok(body) => body,
        Err(status) => {
            close_with(
                stream,
                &error(status, "the request body could not be read"),
                LINGER,
            );
            return;
        }
    };
    let response = app.handle(&request(&head, &body));
    close_with(stream, &response, LINGER);
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        408 => "Request Timeout",
        411 => "Length Required",
        413 => "Payload Too Large",
        503 => "Service Unavailable",
        _ => "",
    }
}

fn write_response(stream: &mut TcpStream, r: &Response) -> std::io::Result<()> {
    write!(
        stream,
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nCache-Control: no-store\r\n\
         X-Content-Type-Options: nosniff\r\nReferrer-Policy: no-referrer\r\nConnection: close\r\n\r\n",
        r.status,
        reason(r.status),
        r.content_type,
        r.body.len()
    )?;
    stream.write_all(r.body.as_bytes())?;
    stream.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::claude::{Api, Claude};
    use crate::ops::testing::*;
    use crate::provider::Provider;
    use crate::store::Store;

    const TOKEN: &str = "t0ken";

    fn start(e: &Env, limits: Limits) -> u16 {
        let (listener, port) = bind(0).unwrap();
        let providers: Vec<Box<dyn Provider>> =
            vec![Box::new(Claude::at(e.tmp.path(), Api::local(&OFFLINE)))];
        let app = Arc::new(App::new(
            Store::open(e.store.root()),
            crate::serve::Places {
                units: e.tmp.path().join("units"),
                snapshot: e.tmp.path().join("tokengauge-usage.json"),
            },
            providers,
            TOKEN.into(),
            port,
        ));
        std::thread::spawn(move || serve(app, listener, limits));
        port
    }

    fn exchange(port: u16, raw: &str) -> String {
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        s.write_all(raw.as_bytes()).unwrap();
        let mut answer = String::new();
        s.read_to_string(&mut answer).unwrap();
        answer
    }

    fn quick() -> Limits {
        Limits {
            read_for: Duration::from_millis(300),
            connections: 4,
        }
    }

    #[test]
    fn the_listener_answers_over_a_real_socket() {
        let e = env(&OFFLINE);
        let port = start(&e, quick());
        let get = |path: &str, token: &str| {
            exchange(
                port,
                &format!(
                    "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nX-Remuda-Token: {token}\r\n\r\n"
                ),
            )
        };
        let page = get("/", "");
        assert!(page.starts_with("HTTP/1.1 200 OK\r\n"), "{page}");
        assert!(page.contains("Cache-Control: no-store"), "{page}");
        assert!(get("/api/state", "wrong").starts_with("HTTP/1.1 401"));
        let state = get("/api/state", TOKEN);
        assert!(state.starts_with("HTTP/1.1 200"), "{state}");
        assert!(state.contains("\"providers\""), "{state}");

        let body = r#"{"name":"nope"}"#;
        let posted = exchange(
            port,
            &format!(
                "POST /api/use HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nX-Remuda-Token: {TOKEN}\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            ),
        );
        assert!(posted.contains("no credential named nope"), "{posted}");
    }

    #[test]
    fn a_request_that_may_not_be_served_is_refused_before_its_body() {
        let e = env(&OFFLINE);
        let port = start(&e, quick());
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        s.write_all(
            format!("POST /api/use HTTP/1.1\r\nHost: evil.example:{port}\r\nContent-Length: 60000\r\n\r\n")
                .as_bytes(),
        )
        .unwrap();
        s.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let mut answer = String::new();
        s.read_to_string(&mut answer).unwrap();
        assert!(answer.starts_with("HTTP/1.1 403"), "{answer}");
    }

    #[test]
    fn oversized_and_stalled_requests_are_cut_off() {
        let e = env(&OFFLINE);
        let port = start(&e, quick());
        let big = exchange(
            port,
            &format!(
                "POST /api/use HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nX-Remuda-Token: {TOKEN}\r\nContent-Length: 999999\r\n\r\n"
            ),
        );
        assert!(big.starts_with("HTTP/1.1 413"), "{big}");

        let started = Instant::now();
        let stalled = exchange(
            port,
            &format!(
                "POST /api/use HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nX-Remuda-Token: {TOKEN}\r\nContent-Length: 10\r\n\r\n{{"
            ),
        );
        assert!(stalled.starts_with("HTTP/1.1 408"), "{stalled}");
        assert!(started.elapsed() < Duration::from_secs(3));

        let garbage = exchange(port, "\u{1}\u{2}\r\n\r\n");
        assert!(garbage.starts_with("HTTP/1.1 400"), "{garbage}");
    }

    fn drip(port: u16, head: &str) -> std::thread::JoinHandle<()> {
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        s.write_all(head.as_bytes()).unwrap();
        std::thread::spawn(move || {
            for _ in 0..100 {
                if s.write_all(b"x").is_err() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        })
    }

    #[test]
    fn a_refused_client_that_keeps_sending_is_cut_off() {
        let e = env(&OFFLINE);
        let port = start(&e, quick());
        let started = Instant::now();
        let dripper = drip(port, "POST /api/use HTTP/1.1\r\nHost: evil.example\r\n\r\n");
        dripper.join().unwrap();
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "{:?}",
            started.elapsed()
        );
    }

    #[test]
    fn a_turned_away_client_that_keeps_sending_does_not_hold_the_door() {
        let e = env(&OFFLINE);
        let port = start(
            &e,
            Limits {
                read_for: Duration::from_secs(5),
                connections: 1,
            },
        );
        let _holder = TcpStream::connect(("127.0.0.1", port)).unwrap();
        std::thread::sleep(Duration::from_millis(100));
        let _dripper = drip(port, "GET / HTTP/1.1\r\n");
        std::thread::sleep(Duration::from_millis(100));
        let started = Instant::now();
        let turned_away = exchange(port, "GET / HTTP/1.1\r\n\r\n");
        assert!(turned_away.starts_with("HTTP/1.1 503"), "{turned_away}");
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "{:?}",
            started.elapsed()
        );
    }

    #[test]
    fn a_body_sent_after_its_head_is_read_and_odd_requests_are_refused() {
        let e = env(&OFFLINE);
        let port = start(&e, quick());
        let body = r#"{"name":"nope"}"#;
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        s.write_all(
            format!(
                "POST /api/use HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nX-Remuda-Token: {TOKEN}\r\nContent-Length: {}\r\n\r\n",
                body.len()
            )
            .as_bytes(),
        )
        .unwrap();
        std::thread::sleep(Duration::from_millis(50));
        s.write_all(&body.as_bytes()[..5]).unwrap();
        std::thread::sleep(Duration::from_millis(50));
        s.write_all(&body.as_bytes()[5..]).unwrap();
        let mut answer = String::new();
        s.read_to_string(&mut answer).unwrap();
        assert!(answer.contains("no credential named nope"), "{answer}");

        let chunked = exchange(
            port,
            &format!(
                "POST /api/use HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nX-Remuda-Token: {TOKEN}\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n"
            ),
        );
        assert!(chunked.starts_with("HTTP/1.1 411"), "{chunked}");

        let huge = format!("GET / HTTP/1.1\r\nX: {}\r\n\r\n", "a".repeat(20_000));
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        let _ = s.write_all(huge.as_bytes());
        let mut answer = String::new();
        let _ = s.read_to_string(&mut answer);
        assert!(answer.starts_with("HTTP/1.1 400"), "{answer}");
    }

    #[test]
    fn connections_past_the_limit_are_turned_away_not_queued() {
        let e = env(&OFFLINE);
        let port = start(
            &e,
            Limits {
                read_for: Duration::from_secs(5),
                connections: 1,
            },
        );
        let _holder = TcpStream::connect(("127.0.0.1", port)).unwrap();
        std::thread::sleep(Duration::from_millis(200));
        let turned_away = exchange(port, "GET / HTTP/1.1\r\n\r\n");
        assert!(turned_away.starts_with("HTTP/1.1 503"), "{turned_away}");
    }
}
