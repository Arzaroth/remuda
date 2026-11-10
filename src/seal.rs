use std::io::{Read, Write};
use std::path::Path;

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
    let fd = tty.as_raw_fd();
    let mut saved = std::mem::MaybeUninit::<libc::termios>::uninit();
    // SAFETY: fd is an open terminal and `saved` is written by tcgetattr before use.
    let saved = unsafe {
        if libc::tcgetattr(fd, saved.as_mut_ptr()) != 0 {
            bail!("could not read the terminal's settings");
        }
        saved.assume_init()
    };
    let mut quiet = saved;
    quiet.c_lflag &= !libc::ECHO;
    quiet.c_lflag |= libc::ECHONL;
    // SAFETY: fd is an open terminal and `quiet` a copy of its own settings.
    unsafe { libc::tcsetattr(fd, libc::TCSANOW, &quiet) };
    let mut out = &tty;
    let typed = write!(out, "{prompt}")
        .and_then(|_| out.flush())
        .and_then(|_| {
            let mut line = String::new();
            std::io::BufReader::new(&tty)
                .read_line(&mut line)
                .map(|_| line)
        });
    // SAFETY: restores the settings read above on the same terminal.
    unsafe { libc::tcsetattr(fd, libc::TCSANOW, &saved) };
    Ok(typed
        .context("could not read the passphrase")?
        .trim_end_matches(['\r', '\n'])
        .to_owned())
}
