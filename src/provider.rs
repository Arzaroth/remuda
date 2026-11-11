use anyhow::{Result, bail};
use serde_json::Value;

use crate::store::Entry;

/// The session id a CLI process (`pid`, started at `start` clock ticks) is
/// running, read from the CLI's directory.
pub type SessionOf = fn(&std::path::Path, u32, u64) -> Option<String>;

#[derive(Debug, Clone, PartialEq)]
pub struct Identity {
    pub account_id: String,
    pub email: String,
    /// Written back into the CLI's own config on a switch: Claude's
    /// `oauthAccount`. None for a CLI that keeps no such block.
    pub oauth_account: Option<Value>,
}

/// A wire format the proxy takes from clients and sends upstream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// OpenAI's `/v1/chat/completions`.
    Chat,
    /// OpenAI's `/v1/responses`.
    Responses,
    /// Anthropic's `/v1/messages`.
    Messages,
}

impl Format {
    pub fn name(self) -> &'static str {
        match self {
            Format::Chat => "OpenAI chat completions",
            Format::Responses => "OpenAI responses",
            Format::Messages => "Anthropic messages",
        }
    }
}

/// Where the proxy sends a request that spends a login, and with what.
pub struct Upstream {
    pub url: String,
    pub headers: Vec<(&'static str, String)>,
}

pub struct Login {
    pub creds: Value,
    pub identity: Identity,
}

/// An answer a sign-in asks for beside the code, stored under `name`: one of
/// `choices`, the first by default, or free text that may be left out when
/// there are none.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct Field {
    pub name: &'static str,
    pub label: &'static str,
    pub choices: &'static [&'static str],
}

/// How `remuda run` starts a CLI on a login of its own: the command, the
/// variable that moves its files, and what of them a session does not share.
pub struct RunSpec {
    pub command: &'static str,
    pub var: &'static str,
    /// Files a session starts from a copy of, without the login's keys, and
    /// whose other keys it merges back when it ends.
    pub copied: Vec<Copied>,
    /// Names in the CLI's directory a session neither links nor copies.
    pub private: &'static [&'static str],
}

pub struct Copied {
    pub from: std::path::PathBuf,
    pub name: &'static str,
    pub login_keys: &'static [&'static str],
}

/// A sign-in the browser has been sent to. Claude's ends with a callback or a
/// pasted code, Codex's with a callback or a device code.
pub trait PendingLogin: Send {
    fn url(&self) -> &str;
    fn needs_code(&self) -> bool;
    fn finish(self: Box<Self>, code: Option<&str>) -> Result<Login>;
    /// What `finish` takes, when `needs_code`.
    fn asks_for(&self) -> &'static str {
        "the code the page shows"
    }
    /// What `finish_with` takes beside the code.
    fn fields(&self) -> Vec<Field> {
        Vec::new()
    }
    /// `finish`, with the answers to `fields` by name.
    fn finish_with(
        self: Box<Self>,
        code: Option<&str>,
        _answers: &serde_json::Map<String, Value>,
    ) -> Result<Login> {
        self.finish(code)
    }
    /// Stops a `finish` that is still waiting, from another thread.
    fn canceller(&self) -> Box<dyn Fn() + Send + Sync> {
        Box::new(|| {})
    }
    /// The code to type on the page `url` opens, when the address does not
    /// carry it.
    fn user_code(&self) -> Option<&str> {
        None
    }
    /// The browser must be on this machine: it calls back to a local port.
    fn local_only(&self) -> bool {
        false
    }
}

/// One coding CLI whose login remuda keeps copies of. `creds` is always the
/// store's shape for that CLI: the part of its credential file that belongs to
/// the login.
pub trait Provider: Send + Sync {
    fn id(&self) -> &'static str;
    fn name(&self) -> &'static str;
    /// The directory the CLI keeps its login in, as this process resolves it.
    fn home(&self) -> std::path::PathBuf;
    fn access_token<'a>(&self, creds: &'a Value) -> Option<&'a str>;
    fn refresh_token<'a>(&self, creds: &'a Value) -> Option<&'a str>;
    fn expires_at(&self, creds: &Value) -> Option<i64>;
    fn refresh_expires_at(&self, _creds: &Value) -> Option<i64> {
        None
    }
    /// False for a credential that never expires and has nothing to refresh.
    fn renews(&self) -> bool {
        true
    }
    /// Its tokens work from two machines at once, so another machine can be
    /// given a copy rather than a sign-in of its own: nothing to refresh, or a
    /// refresh that does not spend the refresh token.
    fn portable(&self) -> bool {
        !self.renews()
    }
    /// The line a shell evaluates to use `entry`, for a CLI that reads its
    /// login from the environment.
    fn env_line(&self, _entry: &Entry) -> Result<String> {
        anyhow::bail!(
            "{} does not read its login from the environment",
            self.name()
        )
    }
    fn plan(&self, creds: &Value) -> String;
    /// The login the CLI is signed into, or None when it is signed out.
    fn live(&self) -> Result<Option<Value>>;
    /// The stored credential once the live login, read as `live`, is synced
    /// into it, or once a sign-in of the same account replaces it.
    fn synced(&self, _stored: &Value, live: Value) -> Value {
        live
    }
    /// A login the CLI holds that `live` does not return because it cannot be
    /// stored, described for the user.
    fn foreign_login(&self) -> Result<Option<String>> {
        Ok(None)
    }
    /// Who the CLI's own files say that login belongs to, without asking the
    /// provider.
    fn live_identity(&self, creds: &Value) -> Result<Option<Identity>>;
    /// Whose tokens these are, according to the tokens or the provider rather
    /// than to a file that can be out of step with them.
    fn identify(&self, creds: &Value) -> Result<Identity>;
    /// Makes `entry` the live login, but only while the live login is still
    /// `outgoing` (store shape); otherwise fails with [`crate::fsx::Changed`]
    /// and writes nothing. `None` replaces whatever is there.
    fn install(&self, entry: &Entry, outgoing: Option<&Value>) -> Result<()>;
    fn signs_out(&self) -> bool {
        false
    }
    /// Takes the login out of the CLI's files and leaves the rest, but only
    /// while the live login is still `outgoing` (store shape); otherwise fails
    /// with [`crate::fsx::Changed`] and writes nothing.
    fn sign_out(&self, _outgoing: &Value) -> Result<()> {
        anyhow::bail!("remuda does not sign {} out", self.name())
    }
    /// The file the live login is read from. Its mtime dates a login that
    /// changed outside remuda, which remuda only notices later.
    fn live_file(&self) -> Option<std::path::PathBuf> {
        None
    }
    /// A lock other tools that rewrite the live login take too, held across a
    /// switch's last read and its write.
    fn lock_live(&self) -> Result<Option<std::fs::File>> {
        Ok(None)
    }
    /// The processes whose running sessions a switch made unattended must
    /// wait out, because they keep the old login and would write it back.
    fn sessions(&self) -> &'static [&'static str] {
        &[]
    }
    /// Returns the account id the token endpoint answered for, when it says.
    fn refresh(&self, creds: &mut Value) -> Result<Option<String>>;
    /// How to read which session a CLI process started at a start time is
    /// running, from the directory a `remuda run` gave it, for a CLI whose
    /// transcripts name a session but no login.
    fn run_sessions(&self) -> Option<SessionOf> {
        None
    }
    /// None for a CLI `remuda run` cannot point at a directory of its own.
    fn run_spec(&self) -> Option<RunSpec> {
        None
    }
    /// The release of the CLI its sign-in was read out of.
    fn mirrors(&self) -> Option<&'static str> {
        None
    }
    /// The same provider with the CLI's files under `dir`, as a `remuda run`
    /// session sees them.
    fn relocated(&self, _dir: &std::path::Path) -> Option<Box<dyn Provider>> {
        None
    }
    /// Spends one of the login's banked usage-limit resets.
    fn redeem_reset(&self, _entry: &Entry, _kind: Option<&str>) -> Result<String> {
        bail!("{} has no banked resets", self.name())
    }
    /// `paste` asks for a sign-in the browser can finish from anywhere rather
    /// than through a callback to this machine, where the CLI offers one: a
    /// code to paste back, or a device code to type in.
    fn begin_login(&self, paste: bool) -> Result<Box<dyn PendingLogin>>;
    /// The formats the CLI's inference endpoint takes for `model`, best
    /// first. Empty when the proxy cannot spend this login.
    fn formats(&self, _model: &str) -> Vec<Format> {
        Vec::new()
    }
    /// Where a `format` request for `model` goes, spending `creds`.
    fn upstream(&self, _creds: &Value, _format: Format, _model: &str) -> Option<Upstream> {
        None
    }
    /// Brings a request body to what the endpoint accepts from a client
    /// sending `user_agent`, or says why it cannot.
    fn shape(&self, _format: Format, _body: &mut Value, _user_agent: Option<&str>) -> Result<()> {
        Ok(())
    }
}
