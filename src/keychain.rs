use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, bail};
use sha2::{Digest, Sha256};

use crate::child::{self, Ran};

const LOCKED: i32 = 36;
const MISSING: i32 = 44;
const WAIT: Duration = Duration::from_secs(10);
/// `security -i` reads its commands into a 4096-byte buffer.
const LINE: usize = 4000;

/// Runs macOS's `security` tool.
pub trait Security: Send + Sync {
    fn run(&self, args: &[&str], input: Option<&[u8]>) -> Result<Ran>;
}

pub struct Tool(pub PathBuf);

impl Security for Tool {
    fn run(&self, args: &[&str], input: Option<&[u8]>) -> Result<Ran> {
        child::run_fed("security", &self.0, args, input, WAIT, 1024 * 1024)
    }
}

/// The `security` this remuda drives: macOS's own, or in a debug build the
/// one `REMUDA_TEST_SECURITY` names, on any system.
pub fn tool() -> Option<Arc<dyn Security>> {
    #[cfg(debug_assertions)]
    if let Some(program) = std::env::var_os("REMUDA_TEST_SECURITY").filter(|v| !v.is_empty()) {
        return Some(Arc::new(Tool(program.into())));
    }
    cfg!(target_os = "macos")
        .then(|| Arc::new(Tool("/usr/bin/security".into())) as Arc<dyn Security>)
}

pub enum Item {
    Held(String),
    Missing,
}

/// One generic password in the login Keychain, named the way Claude Code
/// names its own.
#[derive(Clone)]
pub struct Keychain {
    security: Arc<dyn Security>,
    service: String,
    account: String,
}

/// Claude Code's service for its directory: `CLAUDE_CONFIG_DIR` when it is
/// set, `None` for `~/.claude`.
pub fn service(dir: Option<&str>) -> String {
    match dir {
        None => "Claude Code-credentials".to_owned(),
        Some(dir) => {
            let digest = Sha256::digest(dir.as_bytes());
            let hex: String = digest[..4].iter().map(|b| format!("{b:02x}")).collect();
            format!("Claude Code-{hex}-credentials")
        }
    }
}

/// Claude Code's account: `$USER`, else the login name, and
/// `claude-code-user` for a name outside `[A-Za-z0-9._-]`.
pub fn account(user: Option<String>) -> String {
    user.filter(|u| !u.is_empty())
        .or_else(login_name)
        .filter(|u| {
            u.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        })
        .unwrap_or_else(|| "claude-code-user".to_owned())
}

fn login_name() -> Option<String> {
    let mut buf = vec![0 as libc::c_char; 4096];
    let mut pwd = std::mem::MaybeUninit::<libc::passwd>::uninit();
    let mut found = std::ptr::null_mut();
    // SAFETY: every pointer is valid for the call, and the name read back
    // lives in `buf`, which outlives it.
    unsafe {
        let rc = libc::getpwuid_r(
            libc::getuid(),
            pwd.as_mut_ptr(),
            buf.as_mut_ptr(),
            buf.len(),
            &mut found,
        );
        if rc != 0 || found.is_null() {
            return None;
        }
        let name = (*found).pw_name;
        (!name.is_null()).then(|| {
            std::ffi::CStr::from_ptr(name)
                .to_string_lossy()
                .into_owned()
        })
    }
}

/// `security -w` prints a password that is not printable as hex.
fn unhex(out: &[u8]) -> Option<Vec<u8>> {
    if out.is_empty() || !out.len().is_multiple_of(2) || !out.iter().all(u8::is_ascii_hexdigit) {
        return None;
    }
    let bytes: Vec<u8> = out
        .chunks(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok())
        .collect::<Option<_>>()?;
    bytes
        .iter()
        .any(|b| !(0x20..0x7f).contains(b))
        .then_some(bytes)
}

/// JSON with every character outside ASCII written as a `\u` escape, the
/// same value in a form `security` keeps printable.
pub fn ascii_json(value: &serde_json::Value) -> Result<String> {
    let text = serde_json::to_string(value)?;
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if c.is_ascii() {
            out.push(c);
        } else {
            let mut units = [0u16; 2];
            for unit in c.encode_utf16(&mut units) {
                out.push_str(&format!("\\u{unit:04x}"));
            }
        }
    }
    Ok(out)
}

fn locked() -> anyhow::Error {
    anyhow::anyhow!(
        "the login Keychain is locked in this session, so the Claude Code login it holds \
         cannot be read; run `security unlock-keychain` here, or remuda from the Mac's own session"
    )
}

fn failed(what: &str, ran: &Ran) -> anyhow::Error {
    let code = ran
        .code
        .map_or_else(|| "a signal".to_owned(), |c| c.to_string());
    anyhow::anyhow!("`security {what}` failed with {code}")
}

impl Keychain {
    pub fn new(security: Arc<dyn Security>, service: String, account: String) -> Self {
        Keychain {
            security,
            service,
            account,
        }
    }

    pub fn service(&self) -> &str {
        &self.service
    }

    pub fn with_service(&self, service: String) -> Self {
        Keychain {
            service,
            ..self.clone()
        }
    }

    fn find(&self, password: bool) -> Result<Ran> {
        let mut args = vec![
            "find-generic-password",
            "-a",
            &self.account,
            "-s",
            &self.service,
        ];
        if password {
            args.push("-w");
        }
        self.security.run(&args, None)
    }

    /// The password, or none. A locked Keychain with no such item is the
    /// same as none, since Claude Code then reads its file as well; one
    /// holding it is an error.
    pub fn read(&self) -> Result<Item> {
        let ran = self.find(true)?;
        match ran.code {
            Some(0) => {
                let out = ran.out.strip_suffix(b"\n").unwrap_or(&ran.out);
                let bytes = unhex(out).unwrap_or_else(|| out.to_vec());
                let text = String::from_utf8(bytes)
                    .map_err(|_| anyhow::anyhow!("the Keychain's {} is not text", self.service))?;
                Ok(Item::Held(text))
            }
            Some(MISSING) => Ok(Item::Missing),
            Some(LOCKED) => {
                let held = self.find(false)?;
                match held.code {
                    Some(0) => Err(locked()),
                    Some(MISSING) => Ok(Item::Missing),
                    _ => Err(failed("find-generic-password", &held)),
                }
            }
            _ => Err(failed("find-generic-password", &ran)),
        }
    }

    /// Sets the password through `security -i`, so it never reaches a
    /// command line, and reads it back. False, writing nothing, when it is
    /// too long for that or not printable ASCII, which `security` would print
    /// back as hex to Claude Code.
    pub fn write(&self, password: &str) -> Result<bool> {
        if unhex(password.as_bytes()).is_some() {
            return Ok(false);
        }
        let Some(line) = self.line(password) else {
            return Ok(false);
        };
        let ran = self.security.run(&["-i"], Some(line.as_bytes()))?;
        match ran.code {
            Some(0) => {}
            Some(LOCKED) => return Err(locked()),
            _ => return Err(failed("add-generic-password", &ran)),
        }
        match self.read()? {
            Item::Held(back) if back == password => Ok(true),
            _ => bail!(
                "the Keychain's {} did not keep what was written",
                self.service
            ),
        }
    }

    fn line(&self, password: &str) -> Option<String> {
        if !password.bytes().all(|b| (0x20..0x7f).contains(&b)) {
            return None;
        }
        let quoted = password.replace('\\', "\\\\").replace('"', "\\\"");
        let line = format!(
            "add-generic-password -U -a \"{}\" -s \"{}\" -w \"{quoted}\"\n",
            self.account, self.service
        );
        (line.len() <= LINE).then_some(line)
    }

    pub fn delete(&self) -> Result<()> {
        let ran = self.security.run(
            &[
                "delete-generic-password",
                "-a",
                &self.account,
                "-s",
                &self.service,
            ],
            None,
        )?;
        match ran.code {
            Some(0 | MISSING) => Ok(()),
            Some(LOCKED) => Err(locked()),
            _ => Err(failed("delete-generic-password", &ran)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn services_are_named_as_claude_code_names_them() {
        assert_eq!(service(None), "Claude Code-credentials");
        assert_eq!(service(Some("abc")), "Claude Code-ba7816bf-credentials");
    }

    #[test]
    fn an_account_outside_the_simple_names_is_claude_codes_default() {
        assert_eq!(account(Some("marc.e-b_1".into())), "marc.e-b_1");
        assert_eq!(account(Some("marc e".into())), "claude-code-user");
        assert_eq!(account(Some("é".into())), "claude-code-user");
    }

    #[test]
    fn a_password_is_quoted_and_only_printable_ascii_is_taken() {
        let k = Keychain::new(Arc::new(Tool("/nowhere".into())), "S".into(), "me".into());
        assert_eq!(
            k.line(r#"{"a":"b\\c"}"#).unwrap(),
            "add-generic-password -U -a \"me\" -s \"S\" -w \"{\\\"a\\\":\\\"b\\\\\\\\c\\\"}\"\n"
        );
        assert!(k.line("é").is_none());
        assert!(k.line("a\tb").is_none());
        assert!(k.line(&"a".repeat(3900)).is_some());
        assert!(k.line(&"a".repeat(4000)).is_none());
    }

    /// Only where the login Keychain can be written: a Mac's own session,
    /// or one unlocked with `security unlock-keychain`.
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "writes the login Keychain of whoever runs it"]
    fn the_real_keychain_takes_a_long_item_through_stdin_and_gives_it_back() {
        let k = Keychain::new(
            tool().unwrap(),
            format!("remuda-test-{}-credentials", std::process::id()),
            account(std::env::var("USER").ok()),
        );
        let value = format!("{{\"a\":\"{}\\\\\"}}", "x\"y".repeat(900));
        match k.write(&value) {
            Err(e) if e.to_string().contains("locked") => {
                eprintln!("skipped: {e}");
                return;
            }
            written => assert!(written.unwrap()),
        }
        let back = k.read();
        k.delete().unwrap();
        match back.unwrap() {
            Item::Held(text) => assert_eq!(text, value),
            Item::Missing => panic!("the item was not written"),
        }
        assert!(matches!(k.read().unwrap(), Item::Missing));
    }
}
