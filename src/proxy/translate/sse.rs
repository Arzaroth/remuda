use std::io::{BufRead, Read, Write};

/// A line longer than this ends the stream rather than the memory.
pub const MAX_LINE: u64 = 16 * 1024 * 1024;

/// One server-sent event: its name, when it has one, and its data lines
/// joined.
#[derive(Debug, PartialEq)]
pub struct Sse {
    pub event: Option<String>,
    pub data: String,
}

/// The events of a stream, ending where the stream ends or breaks.
pub struct Reader<R> {
    r: R,
}

impl<R: BufRead> Reader<R> {
    pub fn new(r: R) -> Self {
        Reader { r }
    }

    fn line(&mut self) -> Option<String> {
        let mut buf = Vec::new();
        let n = (&mut self.r)
            .take(MAX_LINE)
            .read_until(b'\n', &mut buf)
            .ok()?;
        if n == 0 || (n as u64 == MAX_LINE && !buf.ends_with(b"\n")) {
            return None;
        }
        if buf.ends_with(b"\n") {
            buf.pop();
        }
        Some(String::from_utf8_lossy(&buf).into_owned())
    }
}

impl<R: BufRead> Iterator for Reader<R> {
    type Item = Sse;

    fn next(&mut self) -> Option<Sse> {
        let mut event = None;
        let mut data: Vec<String> = Vec::new();
        loop {
            let line = match self.line() {
                Some(line) => line,
                _ if !data.is_empty() => {
                    return Some(Sse {
                        event,
                        data: data.join("\n"),
                    });
                }
                _ => return None,
            };
            let line = line.strip_suffix('\r').unwrap_or(&line);
            if line.is_empty() {
                if data.is_empty() {
                    event = None;
                    continue;
                }
                return Some(Sse {
                    event,
                    data: data.join("\n"),
                });
            }
            if line.starts_with(':') {
                continue;
            }
            let (field, value) = line.split_once(':').unwrap_or((line, ""));
            let value = value.strip_prefix(' ').unwrap_or(value);
            match field {
                "event" => event = Some(value.to_owned()),
                "data" => data.push(value.to_owned()),
                _ => {}
            }
        }
    }
}

pub fn write(out: &mut impl Write, event: Option<&str>, data: &str) -> std::io::Result<()> {
    if let Some(e) = event {
        writeln!(out, "event: {e}")?;
    }
    write!(out, "data: {data}\n\n")?;
    out.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_are_split_on_blank_lines() {
        let raw =
            ": ping\n\nevent: a\ndata: {\"x\":1}\n\ndata: one\r\ndata: two\r\n\r\ndata: [DONE]";
        let events: Vec<Sse> = Reader::new(raw.as_bytes()).collect();
        assert_eq!(
            events,
            vec![
                Sse {
                    event: Some("a".into()),
                    data: "{\"x\":1}".into()
                },
                Sse {
                    event: None,
                    data: "one\ntwo".into()
                },
                Sse {
                    event: None,
                    data: "[DONE]".into()
                },
            ]
        );
    }
}
