use anyhow::{Result, bail};
use serde_json::Value;

use crate::store::Entry;

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

/// A sign-in the browser has been sent to. Claude's ends with a code the user
/// pastes back, Codex's with a callback to a local listener.
pub trait PendingLogin: Send {
    fn url(&self) -> &str;
    fn needs_code(&self) -> bool;
    fn finish(self: Box<Self>, code: Option<&str>) -> Result<Login>;
    /// What `finish` takes, when `needs_code`.
    fn asks_for(&self) -> &'static str {
        "the code the page shows"
    }
    /// Stops a `finish` that is still waiting, from another thread.
    fn canceller(&self) -> Box<dyn Fn() + Send + Sync> {
        Box::new(|| {})
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
    /// None for a CLI `remuda run` cannot point at a directory of its own.
    fn run_spec(&self) -> Option<RunSpec> {
        None
    }
    /// The same provider with the CLI's files under `dir`, as a `remuda run`
    /// session sees them.
    fn relocated(&self, _dir: &std::path::Path) -> Option<Box<dyn Provider>> {
        None
    }
    /// Spends one of the login's banked usage-limit resets.
    fn redeem_reset(&self, _entry: &Entry) -> Result<String> {
        bail!("{} has no banked resets", self.name())
    }
    fn begin_login(&self) -> Result<Box<dyn PendingLogin>>;
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
