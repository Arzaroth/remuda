use anyhow::Result;
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

pub struct Login {
    pub creds: Value,
    pub identity: Identity,
}

/// A sign-in the browser has been sent to. Claude's ends with a code the user
/// pastes back, Codex's with a callback to a local listener.
pub trait PendingLogin: Send {
    fn url(&self) -> &str;
    fn needs_code(&self) -> bool;
    fn finish(self: Box<Self>, code: Option<&str>) -> Result<Login>;
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
    fn access_token<'a>(&self, creds: &'a Value) -> Option<&'a str>;
    fn refresh_token<'a>(&self, creds: &'a Value) -> Option<&'a str>;
    fn expires_at(&self, creds: &Value) -> Option<i64>;
    fn refresh_expires_at(&self, _creds: &Value) -> Option<i64> {
        None
    }
    fn plan(&self, creds: &Value) -> String;
    /// The login the CLI is signed into, or None when it is signed out.
    fn live(&self) -> Result<Option<Value>>;
    /// Who the CLI's own files say that login belongs to, without asking the
    /// provider.
    fn live_identity(&self, creds: &Value) -> Result<Option<Identity>>;
    /// Whose tokens these are, according to the tokens or the provider rather
    /// than to a file that can be out of step with them.
    fn identify(&self, creds: &Value) -> Result<Identity>;
    fn install(&self, entry: &Entry) -> Result<()>;
    /// A lock other tools that rewrite the live login take too, held across a
    /// switch's last read and its write.
    fn lock_live(&self) -> Result<Option<std::fs::File>> {
        Ok(None)
    }
    /// Returns the account id the token endpoint answered for, when it says.
    fn refresh(&self, creds: &mut Value) -> Result<Option<String>>;
    fn begin_login(&self) -> Result<Box<dyn PendingLogin>>;
}
