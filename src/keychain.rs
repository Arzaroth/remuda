use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
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
                let text = String::from_utf8(ran.out)
                    .map_err(|_| anyhow::anyhow!("the Keychain's {} is not text", self.service))?;
                Ok(Item::Held(text.trim_end_matches('\n').to_owned()))
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
    /// command line. False, writing nothing, when it is too long for that.
    pub fn write(&self, password: &str) -> Result<bool> {
        let Some(line) = self.line(password) else {
            return Ok(false);
        };
        let ran = self.security.run(&["-i"], Some(line.as_bytes()))?;
        match ran.code {
            Some(0) => Ok(true),
            Some(LOCKED) => Err(locked()),
            _ => Err(failed("add-generic-password", &ran)),
        }
    }

    fn line(&self, password: &str) -> Option<String> {
        let value = if password.bytes().all(|b| (0x20..0x7f).contains(&b)) {
            let quoted = password.replace('\\', "\\\\").replace('"', "\\\"");
            format!("-w \"{quoted}\"")
        } else {
            let hex: String = password.bytes().map(|b| format!("{b:02x}")).collect();
            format!("-X \"{hex}\"")
        };
        let line = format!(
            "add-generic-password -U -a \"{}\" -s \"{}\" {value}\n",
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
    fn a_password_is_quoted_when_printable_and_hex_otherwise() {
        let k = Keychain::new(Arc::new(Tool("/nowhere".into())), "S".into(), "me".into());
        assert_eq!(
            k.line(r#"{"a":"b\\c"}"#).unwrap(),
            "add-generic-password -U -a \"me\" -s \"S\" -w \"{\\\"a\\\":\\\"b\\\\\\\\c\\\"}\"\n"
        );
        assert_eq!(
            k.line("é").unwrap(),
            "add-generic-password -U -a \"me\" -s \"S\" -X \"c3a9\"\n"
        );
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
