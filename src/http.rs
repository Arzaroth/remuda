use std::io::{ErrorKind, Read, Write};
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
    /// Event streams held open at once, within `connections`.
    pub streams: usize,
    /// How often an event stream looks for a change.
    pub watch_every: Duration,
    /// How long an event stream stays silent before it sends a comment, so a
    /// client that vanished without closing is found by the failed write.
    pub keepalive: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            read_for: Duration::from_secs(5),
            connections: 32,
            streams: 8,
            watch_every: Duration::from_secs(2),
            keepalive: Duration::from_secs(15),
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

impl Slot {
    fn take(taken: &Arc<AtomicUsize>, limit: usize) -> Option<Slot> {
        if taken.fetch_add(1, Ordering::SeqCst) >= limit {
            taken.fetch_sub(1, Ordering::SeqCst);
            return None;
        }
        Some(Slot(Arc::clone(taken)))
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

pub fn serve(app: Arc<App>, listener: TcpListener, limits: Limits) {
    let busy = Arc::new(AtomicUsize::new(0));
    let streams = Arc::new(AtomicUsize::new(0));
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let Some(slot) = Slot::take(&busy, limits.connections) else {
            // On the accepting thread: only what has already arrived is
            // drained, so no client can hold this loop.
            close_with(stream, &error(503, "busy"), Duration::ZERO);
            continue;
        };
        let app = Arc::clone(&app);
        let streams = Arc::clone(&streams);
        // A thread that cannot be started is a refused connection, not a
        // stopped server.
        let _ = std::thread::Builder::new().spawn(move || {
            let _slot = slot;
            answer(&app, stream, limits, &streams);
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

fn answer(app: &App, mut stream: TcpStream, limits: Limits, streams: &Arc<AtomicUsize>) {
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
    if app.streams(&request(&head, "")) {
        match Slot::take(streams, limits.streams) {
            Some(_slot) => follow(app, stream, limits),
            None => close_with(stream, &error(503, "too many pages open"), LINGER),
        }
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

/// The client sends nothing after its head, so anything it does send, or its
/// closing, ends the stream.
fn follow(app: &App, mut stream: TcpStream, limits: Limits) {
    let mut seen = app.fingerprint();
    let _ = stream.set_write_timeout(Some(Duration::from_secs(1)));
    if write_head(&mut stream, 200, "text/event-stream", None)
        .and_then(|()| stream.write_all(b": connected\n\n"))
        .and_then(|()| stream.flush())
        .is_err()
        || stream.set_read_timeout(Some(limits.watch_every)).is_err()
    {
        return;
    }
    let mut quiet = Instant::now();
    let mut sink = [0u8; 64];
    loop {
        match stream.read(&mut sink) {
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            _ => return,
        }
        let now = app.fingerprint();
        let message: &[u8] = if now != seen {
            seen = now;
            b"data: changed\n\n"
        } else if quiet.elapsed() >= limits.keepalive {
            b": \n\n"
        } else {
            continue;
        };
        quiet = Instant::now();
        if stream
            .write_all(message)
            .and_then(|()| stream.flush())
            .is_err()
        {
            return;
        }
    }
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

/// Without a length, the body runs until the connection closes.
fn write_head(
    stream: &mut TcpStream,
    status: u16,
    content_type: &str,
    length: Option<usize>,
) -> std::io::Result<()> {
    write!(
        stream,
        "HTTP/1.1 {status} {}\r\nContent-Type: {content_type}\r\n",
        reason(status)
    )?;
    if let Some(length) = length {
        write!(stream, "Content-Length: {length}\r\n")?;
    }
    write!(
        stream,
        "Cache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\n\
         Referrer-Policy: no-referrer\r\nConnection: close\r\n\r\n"
    )
}

fn write_response(stream: &mut TcpStream, r: &Response) -> std::io::Result<()> {
    write_head(stream, r.status, r.content_type, Some(r.body.len()))?;
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
                units: vec![e.tmp.path().join("units")],
                snapshot: e.tmp.path().join("tokengauge-usage.json"),
            },
            providers,
            TOKEN.into(),
            port,
            Box::new(|_| false),
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
            ..Limits::default()
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
                ..Limits::default()
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
                ..Limits::default()
            },
        );
        let _holder = TcpStream::connect(("127.0.0.1", port)).unwrap();
        std::thread::sleep(Duration::from_millis(200));
        let turned_away = exchange(port, "GET / HTTP/1.1\r\n\r\n");
        assert!(turned_away.starts_with("HTTP/1.1 503"), "{turned_away}");
    }

    fn watching(keepalive: Duration) -> Limits {
        Limits {
            read_for: Duration::from_millis(300),
            connections: 4,
            streams: 1,
            watch_every: Duration::from_millis(20),
            keepalive,
        }
    }

    fn follow_events(port: u16, token: &str) -> TcpStream {
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        s.write_all(
            format!("GET /api/events HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nX-Remuda-Token: {token}\r\n\r\n")
                .as_bytes(),
        )
        .unwrap();
        s.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        s
    }

    fn read_until(s: &mut TcpStream, what: &str) -> String {
        let mut got = Vec::new();
        let mut chunk = [0u8; 1024];
        while !String::from_utf8_lossy(&got).contains(what) {
            match s.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => got.extend_from_slice(&chunk[..n]),
            }
        }
        String::from_utf8(got).unwrap()
    }

    struct Events {
        s: TcpStream,
        pending: String,
    }

    impl Events {
        fn open(port: u16) -> (String, Events) {
            let mut s = follow_events(port, TOKEN);
            let head = read_until(&mut s, ": connected\n\n");
            let pending = String::new();
            (head, Events { s, pending })
        }

        /// Waits for a slot a closed stream is still giving back.
        fn open_when_free(port: u16) -> Events {
            let until = Instant::now() + Duration::from_secs(2);
            loop {
                let (head, events) = Events::open(port);
                if head.starts_with("HTTP/1.1 200") {
                    return events;
                }
                assert!(head.starts_with("HTTP/1.1 503"), "{head}");
                assert!(Instant::now() < until, "the slot was never freed");
                std::thread::sleep(Duration::from_millis(10));
            }
        }

        /// The next event or comment; None when the stream ended or stayed
        /// silent for `wait`.
        fn next(&mut self, wait: Duration) -> Option<String> {
            let until = Instant::now() + wait;
            let mut chunk = [0u8; 256];
            loop {
                if let Some(i) = self.pending.find("\n\n") {
                    let event = self.pending[..i].to_owned();
                    self.pending.drain(..i + 2);
                    return Some(event);
                }
                let left = until.checked_duration_since(Instant::now())?;
                self.s
                    .set_read_timeout(Some(left.max(Duration::from_millis(1))))
                    .unwrap();
                match self.s.read(&mut chunk) {
                    Ok(0) | Err(_) => return None,
                    Ok(n) => self
                        .pending
                        .push_str(std::str::from_utf8(&chunk[..n]).unwrap()),
                }
            }
        }

        /// Drops what a write in several steps may still be reporting.
        fn settle(&mut self) {
            while self.next(Duration::from_millis(150)).is_some() {}
        }

        fn ended(&mut self) -> bool {
            self.s
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            matches!(self.s.read(&mut [0u8; 256]), Ok(0))
        }
    }

    const NEVER: Duration = Duration::from_secs(60);
    const SOON: Duration = Duration::from_secs(2);

    #[test]
    fn an_event_stream_says_when_the_store_or_the_snapshot_changes() {
        let e = env(&OFFLINE);
        let port = start(&e, watching(NEVER));
        assert!(
            read_until(&mut follow_events(port, "wrong"), "\r\n\r\n").starts_with("HTTP/1.1 401")
        );

        let (head, mut events) = Events::open(port);
        assert!(head.starts_with("HTTP/1.1 200 OK\r\n"), "{head}");
        assert!(head.contains("Content-Type: text/event-stream"), "{head}");
        assert!(!head.contains("Content-Length"), "{head}");
        assert!(head.contains("Cache-Control: no-store"), "{head}");

        std::fs::write(e.tmp.path().join("tokengauge-usage.json"), "{}").unwrap();
        assert_eq!(events.next(SOON).as_deref(), Some("data: changed"));
        events.settle();
        e.stored("work", "u-work", HOUR);
        assert_eq!(events.next(SOON).as_deref(), Some("data: changed"));
        events.settle();

        let root = e.store.root();
        std::fs::write(root.join("claude").join(".work.json.remuda-1"), "{}").unwrap();
        std::fs::write(root.join("open-x.html"), "<p>").unwrap();
        assert_eq!(events.next(Duration::from_millis(300)), None);
    }

    #[test]
    fn a_quiet_event_stream_sends_a_keepalive() {
        let e = env(&OFFLINE);
        let port = start(&e, watching(Duration::from_millis(100)));
        let (_, mut events) = Events::open(port);
        assert_eq!(events.next(SOON).as_deref(), Some(": "));
    }

    #[test]
    fn event_streams_past_their_limit_are_turned_away_until_one_ends() {
        let e = env(&OFFLINE);
        let port = start(&e, watching(NEVER));
        let (_, first) = Events::open(port);
        let (turned_away, _) = Events::open(port);
        assert!(turned_away.starts_with("HTTP/1.1 503"), "{turned_away}");

        drop(first);
        let mut second = Events::open_when_free(port);
        second.s.write_all(b"x").unwrap();
        assert!(second.ended());
        Events::open_when_free(port);
    }
}
