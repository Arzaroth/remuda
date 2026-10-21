use std::io::{ErrorKind, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Weak};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use serde_json::{Value, json};

use super::Backend;
use super::forward;
use crate::http::{Head, Slot, read_head};
use crate::provider::Format;

/// Requests carry whole conversations, attachments included.
const MAX_BODY: usize = 32 * 1024 * 1024;
const CONNECTIONS: usize = 32;
const HEAD_WITHIN: Duration = Duration::from_secs(10);
/// How long a client may go quiet while it sends its body.
const BODY_STALL: Duration = Duration::from_secs(30);
const POLL: Duration = Duration::from_millis(50);

/// A listener that answers until it is told to stop.
pub struct Running {
    pub port: u16,
    stop: Arc<AtomicBool>,
    accepting: Option<JoinHandle<()>>,
}

impl Running {
    /// Stops accepting and frees the port. Requests already being answered
    /// run to their end.
    pub fn stop(mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.accepting.take() {
            let _ = t.join();
        }
    }
}

pub fn start(backend: Weak<dyn Backend>, port: u16) -> Result<Running> {
    let listener = TcpListener::bind(("127.0.0.1", port))
        .with_context(|| format!("cannot listen on 127.0.0.1:{port}"))?;
    let port = listener.local_addr()?.port();
    listener.set_nonblocking(true)?;
    let stop = Arc::new(AtomicBool::new(false));
    let stopping = Arc::clone(&stop);
    let accepting = std::thread::Builder::new()
        .name("remuda-proxy".into())
        .spawn(move || accept(listener, backend, &stopping))?;
    Ok(Running {
        port,
        stop,
        accepting: Some(accepting),
    })
}

fn accept(listener: TcpListener, backend: Weak<dyn Backend>, stop: &AtomicBool) {
    let busy = Arc::new(AtomicUsize::new(0));
    while !stop.load(Ordering::SeqCst) {
        let stream = match listener.accept() {
            Ok((stream, _)) => stream,
            Err(e) if e.kind() == ErrorKind::WouldBlock => {
                std::thread::sleep(POLL);
                continue;
            }
            Err(_) => {
                std::thread::sleep(POLL);
                continue;
            }
        };
        if stream.set_nonblocking(false).is_err() {
            continue;
        }
        let Some(slot) = Slot::take(&busy, CONNECTIONS) else {
            refuse(stream, 503, None, "the proxy is busy");
            continue;
        };
        let Some(backend) = backend.upgrade() else {
            return;
        };
        let _ = std::thread::Builder::new().spawn(move || {
            let _slot = slot;
            answer(backend.as_ref(), stream);
        });
    }
}

/// The format a path speaks, for its errors as much as its requests.
fn format_of(path: &str) -> Option<Format> {
    match path {
        "/v1/chat/completions" | "/chat/completions" => Some(Format::Chat),
        "/v1/responses" | "/responses" => Some(Format::Responses),
        "/v1/messages" | "/v1/messages/count_tokens" => Some(Format::Messages),
        _ => None,
    }
}

/// An error in the shape the client's SDK reads.
pub fn error_body(format: Option<Format>, status: u16, message: &str) -> Value {
    let kind = match status {
        400 | 413 => "invalid_request_error",
        401 => "authentication_error",
        403 => "permission_error",
        404 => "not_found_error",
        429 => "rate_limit_error",
        502 | 503 => "api_error",
        _ => "api_error",
    };
    match format {
        Some(Format::Messages) => {
            json!({"type": "error", "error": {"type": kind, "message": message}})
        }
        _ => json!({"error": {"message": message, "type": kind, "code": status}}),
    }
}

fn refuse(mut stream: TcpStream, status: u16, format: Option<Format>, message: &str) {
    let body = error_body(format, status, message).to_string();
    let _ = stream.set_write_timeout(Some(Duration::from_secs(1)));
    let _ = write_head(
        &mut stream,
        status,
        "application/json",
        Some(body.len()),
        &[],
    )
    .and_then(|()| stream.write_all(body.as_bytes()));
    let _ = stream.flush();
    let _ = stream.shutdown(std::net::Shutdown::Write);
}

/// The key, as OpenAI's SDKs and Anthropic's each send it.
fn presented(head: &Head) -> Option<&str> {
    head.header("Authorization")
        .and_then(|v| {
            v.strip_prefix("Bearer ")
                .or_else(|| v.strip_prefix("bearer "))
        })
        .or_else(|| head.header("x-api-key"))
        .map(str::trim)
}

fn answer(backend: &dyn Backend, mut stream: TcpStream) {
    let Some(head) = read_head(&mut stream, Instant::now() + HEAD_WITHIN) else {
        refuse(stream, 400, None, "bad request");
        return;
    };
    let format = format_of(&head.path);
    let accepted = match backend.settings() {
        Ok(s) => presented(&head).is_some_and(|k| s.accepts(k)),
        Err(_) => false,
    };
    if !accepted {
        refuse(
            stream,
            401,
            format,
            "send a remuda proxy key as a bearer token or x-api-key; the remuda page makes one",
        );
        return;
    }
    match (head.method.as_str(), head.path.as_str()) {
        ("GET", "/v1/models" | "/models") => {
            let anthropic = head.header("anthropic-version").is_some();
            let body = forward::models(backend, anthropic).to_string();
            let _ = write_head(&mut stream, 200, "application/json", Some(body.len()), &[])
                .and_then(|()| stream.write_all(body.as_bytes()));
        }
        ("POST", path) if format.is_some() => {
            let format = format.unwrap_or(Format::Chat);
            let body = match read_body(&mut stream, &head) {
                Ok(body) => body,
                Err((status, message)) => return refuse(stream, status, Some(format), message),
            };
            let body: Value = match serde_json::from_slice(&body) {
                Ok(v @ Value::Object(_)) => v,
                _ => {
                    return refuse(
                        stream,
                        400,
                        Some(format),
                        "the request body is not a JSON object",
                    );
                }
            };
            let call = forward::Call {
                format,
                count_tokens: path.ends_with("/count_tokens"),
                head: &head,
                body,
            };
            if let Err(e) = forward::forward(backend, call, &mut stream) {
                refuse(stream, e.status, Some(format), &e.message);
            }
        }
        _ => refuse(stream, 404, format, "the proxy has no such endpoint"),
    }
}

fn read_body(stream: &mut TcpStream, head: &Head) -> Result<Vec<u8>, (u16, &'static str)> {
    stream
        .set_read_timeout(Some(BODY_STALL))
        .map_err(|_| (408, "the request body could not be read"))?;
    let mut reader = head.rest.as_slice().chain(stream);
    let chunked = head
        .header("Transfer-Encoding")
        .is_some_and(|v| v.to_ascii_lowercase().contains("chunked"));
    if chunked {
        return read_chunked(&mut reader);
    }
    let length: usize = match head.header("Content-Length") {
        Some(v) => v.trim().parse().map_err(|_| (400, "bad Content-Length"))?,
        None => 0,
    };
    if length > MAX_BODY {
        return Err((413, "the request body is too large"));
    }
    let mut body = vec![0u8; length];
    reader
        .read_exact(&mut body)
        .map_err(|_| (408, "the request body was cut short"))?;
    Ok(body)
}

fn read_chunked(reader: &mut impl Read) -> Result<Vec<u8>, (u16, &'static str)> {
    let cut = (408, "the request body was cut short");
    let mut body = Vec::new();
    loop {
        let line = read_line(reader).ok_or(cut)?;
        let size = line.split(';').next().unwrap_or_default().trim();
        let size = usize::from_str_radix(size, 16).map_err(|_| (400, "bad chunk size"))?;
        if size == 0 {
            while !read_line(reader).ok_or(cut)?.is_empty() {}
            return Ok(body);
        }
        if body.len() + size > MAX_BODY {
            return Err((413, "the request body is too large"));
        }
        let start = body.len();
        body.resize(start + size, 0);
        reader.read_exact(&mut body[start..]).map_err(|_| cut)?;
        if !read_line(reader).ok_or(cut)?.is_empty() {
            return Err((400, "bad chunk"));
        }
    }
}

fn read_line(reader: &mut impl Read) -> Option<String> {
    let mut line = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        reader.read_exact(&mut byte).ok()?;
        if byte[0] == b'\n' {
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            return String::from_utf8(line).ok();
        }
        line.push(byte[0]);
        if line.len() > 1024 {
            return None;
        }
    }
}

/// Without a length, the body runs until the connection closes.
pub fn write_head(
    stream: &mut impl Write,
    status: u16,
    content_type: &str,
    length: Option<usize>,
    extra: &[(&str, String)],
) -> std::io::Result<()> {
    let reason = reqwest::StatusCode::from_u16(status)
        .ok()
        .and_then(|s| s.canonical_reason())
        .unwrap_or("");
    write!(
        stream,
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\n"
    )?;
    if let Some(length) = length {
        write!(stream, "Content-Length: {length}\r\n")?;
    }
    for (name, value) in extra {
        write!(stream, "{name}: {value}\r\n")?;
    }
    write!(
        stream,
        "Cache-Control: no-store\r\nConnection: close\r\n\r\n"
    )?;
    stream.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_chunked_body_is_put_back_together() {
        let raw = b"5\r\nhello\r\n6;x=y\r\n world\r\n0\r\nTrailer: a\r\n\r\n";
        assert_eq!(read_chunked(&mut &raw[..]).unwrap(), b"hello world");
        assert_eq!(read_chunked(&mut &b"5\r\nhel"[..]).unwrap_err().0, 408);
        assert_eq!(read_chunked(&mut &b"zz\r\n"[..]).unwrap_err().0, 400);
        assert_eq!(read_chunked(&mut &b"2\r\nabX\r\n"[..]).unwrap_err().0, 400);
    }

    #[test]
    fn paths_say_their_format() {
        assert_eq!(format_of("/v1/chat/completions"), Some(Format::Chat));
        assert_eq!(format_of("/v1/responses"), Some(Format::Responses));
        assert_eq!(format_of("/v1/messages"), Some(Format::Messages));
        assert_eq!(
            format_of("/v1/messages/count_tokens"),
            Some(Format::Messages)
        );
        assert_eq!(format_of("/v1/models"), None);
    }

    #[test]
    fn errors_take_the_clients_shape() {
        let anthropic = error_body(Some(Format::Messages), 401, "no");
        assert_eq!(anthropic["type"], "error");
        assert_eq!(anthropic["error"]["type"], "authentication_error");
        let openai = error_body(Some(Format::Chat), 404, "no");
        assert_eq!(openai["error"]["message"], "no");
        assert_eq!(openai["error"]["code"], 404);
    }
}
