use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

/// What a program printed, once it exited within its deadline.
pub struct Ran {
    pub ok: bool,
    pub out: Vec<u8>,
    pub err: Vec<u8>,
}

impl Ran {
    pub fn said(&self) -> String {
        String::from_utf8_lossy(&self.err).trim().to_owned()
    }
}

/// Runs `program` with nothing on its stdin, killing it at `wait`. `who`
/// names it in the errors.
pub fn run(who: &str, program: &Path, args: &[&str], wait: Duration, limit: u64) -> Result<Ran> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("could not run {}", program.display()))?;
    let out = drain(child.stdout.take().context("no output")?, limit);
    let err = drain(child.stderr.take().context("no error output")?, 4096);
    let until = Instant::now() + wait;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= until {
            let _ = child.kill();
            let _ = child.wait();
            bail!("{who} took longer than {}s to answer", wait.as_secs());
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    // A process the child left behind can hold the pipes open after it exits,
    // so reading them to their end shares the same deadline.
    let left = || until.saturating_duration_since(Instant::now());
    let out = out
        .recv_timeout(left())
        .map_err(|_| anyhow::anyhow!("{who}'s output did not end within {}s", wait.as_secs()))?;
    let err = if status.success() {
        Vec::new()
    } else {
        err.recv_timeout(left()).unwrap_or_default()
    };
    Ok(Ran {
        ok: status.success(),
        out,
        err,
    })
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
