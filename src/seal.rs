use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicI32, AtomicPtr, Ordering};

use age::secrecy::SecretString;
use anyhow::{Context, Result, bail};

/// Seals `plain` under a passphrase in age's format, so `age -d` opens it too.
pub fn seal(plain: &[u8], passphrase: &str) -> Result<Vec<u8>> {
    let encryptor = age::Encryptor::with_user_passphrase(SecretString::from(passphrase.to_owned()));
    let mut sealed = Vec::new();
    let mut writer = encryptor
        .wrap_output(&mut sealed)
        .context("could not start sealing")?;
    writer.write_all(plain)?;
    writer.finish().context("could not finish sealing")?;
    Ok(sealed)
}

pub fn open(sealed: &[u8], passphrase: &str) -> Result<Vec<u8>> {
    let decryptor = age::Decryptor::new(sealed).context("not a file `remuda export` sealed")?;
    if !decryptor.is_scrypt() {
        bail!("this file is sealed to keys, not a passphrase");
    }
    let identity = age::scrypt::Identity::new(SecretString::from(passphrase.to_owned()));
    let mut reader = decryptor
        .decrypt(std::iter::once(&identity as &dyn age::Identity))
        .map_err(|_| anyhow::anyhow!("wrong passphrase, or the file was damaged"))?;
    let mut plain = Vec::new();
    reader
        .read_to_end(&mut plain)
        .context("the file was damaged")?;
    Ok(plain)
}

/// The passphrase from the first line of `file`, or typed at the terminal
/// with echo off, twice when `confirm`.
pub fn passphrase(file: Option<&Path>, confirm: bool) -> Result<String> {
    if let Some(file) = file {
        let text = std::fs::read_to_string(file)
            .with_context(|| format!("could not read {}", file.display()))?;
        let line = text.lines().next().unwrap_or_default().to_owned();
        if line.is_empty() {
            bail!("{} holds no passphrase on its first line", file.display());
        }
        return Ok(line);
    }
    let first = read_hidden("Passphrase: ")?;
    if first.is_empty() {
        bail!("an empty passphrase seals nothing");
    }
    if confirm && read_hidden("Again: ")? != first {
        bail!("the two passphrases differ");
    }
    Ok(first)
}

fn read_hidden(prompt: &str) -> Result<String> {
    use std::io::BufRead;
    use std::os::fd::AsRawFd;
    let tty = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")
        .context("no terminal to type the passphrase in; pass --passphrase-file")?;
    let _quiet = Quiet::new(tty.as_raw_fd())?;
    let mut out = &tty;
    let typed = write!(out, "{prompt}")
        .and_then(|_| out.flush())
        .and_then(|_| {
            let mut line = String::new();
            std::io::BufReader::new(&tty)
                .read_line(&mut line)
                .map(|_| line)
        });
    Ok(typed
        .context("could not read the passphrase")?
        .trim_end_matches(['\r', '\n'])
        .to_owned())
}

const ENDING: [libc::c_int; 4] = [libc::SIGINT, libc::SIGQUIT, libc::SIGHUP, libc::SIGTERM];

static QUIET_FD: AtomicI32 = AtomicI32::new(-1);
static SAVED: AtomicPtr<libc::termios> = AtomicPtr::new(std::ptr::null_mut());

/// The terminal with echo off, its settings put back when dropped or when a
/// signal ends remuda first.
struct Quiet {
    fd: libc::c_int,
    saved: Box<libc::termios>,
    before: Vec<libc::sighandler_t>,
}

extern "C" fn put_back_and_end(signal: libc::c_int) {
    let fd = QUIET_FD.load(Ordering::SeqCst);
    let saved = SAVED.load(Ordering::SeqCst);
    // SAFETY: tcsetattr, signal and raise are async-signal-safe, and `saved`
    // points at settings `Quiet` keeps alive while the handler is installed.
    unsafe {
        if fd >= 0 && !saved.is_null() {
            libc::tcsetattr(fd, libc::TCSANOW, saved);
        }
        libc::signal(signal, libc::SIG_DFL);
        libc::raise(signal);
    }
}

impl Quiet {
    fn new(fd: libc::c_int) -> Result<Quiet> {
        let mut saved = std::mem::MaybeUninit::<libc::termios>::uninit();
        // SAFETY: `saved` is written by tcgetattr before it is read.
        let saved = unsafe {
            if libc::tcgetattr(fd, saved.as_mut_ptr()) != 0 {
                bail!("could not read the terminal's settings");
            }
            Box::new(saved.assume_init())
        };
        let mut quiet = *saved;
        quiet.c_lflag &= !libc::ECHO;
        quiet.c_lflag |= libc::ECHONL;
        QUIET_FD.store(fd, Ordering::SeqCst);
        SAVED.store(std::ptr::from_ref(&*saved).cast_mut(), Ordering::SeqCst);
        let handler = put_back_and_end as extern "C" fn(libc::c_int) as libc::sighandler_t;
        // SAFETY: the handler only touches what `Quiet` keeps until drop,
        // where the previous handlers are put back.
        let before = ENDING
            .iter()
            .map(|s| unsafe { libc::signal(*s, handler) })
            .collect();
        // SAFETY: fd is an open terminal and `quiet` a copy of its own settings.
        unsafe { libc::tcsetattr(fd, libc::TCSANOW, &quiet) };
        Ok(Quiet { fd, saved, before })
    }
}

impl Drop for Quiet {
    fn drop(&mut self) {
        // SAFETY: restores the settings read in `new` on the same terminal,
        // then the handlers that were there before.
        unsafe {
            libc::tcsetattr(self.fd, libc::TCSANOW, &*self.saved);
            for (s, h) in ENDING.iter().zip(&self.before) {
                libc::signal(*s, *h);
            }
        }
        SAVED.store(std::ptr::null_mut(), Ordering::SeqCst);
        QUIET_FD.store(-1, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_is_sealed_opens_with_its_passphrase_only() {
        let sealed = seal(b"the bundle", "correct horse").unwrap();
        assert!(sealed.starts_with(b"age-encryption.org/v1\n-> scrypt "));
        assert!(!sealed.windows(10).any(|w| w == b"the bundle"));
        assert_eq!(open(&sealed, "correct horse").unwrap(), b"the bundle");
        let error = open(&sealed, "wrong").err().unwrap();
        assert!(error.to_string().contains("wrong passphrase"), "{error}");
        assert!(open(b"not age at all", "correct horse").is_err());
    }

    #[test]
    fn a_passphrase_file_gives_its_first_line() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("pass");
        std::fs::write(&file, "two words\nignored\n").unwrap();
        assert_eq!(passphrase(Some(&file), true).unwrap(), "two words");
        std::fs::write(&file, "\n").unwrap();
        assert!(passphrase(Some(&file), false).is_err());
    }
}
