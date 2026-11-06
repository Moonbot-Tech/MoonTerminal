//! Open a privileged SSH session with either a saved administrator or a provider login.

use crate::app_key;
use crate::hosts::Hosts;
use crate::script::Privilege;
use crate::setup::{self, FirstAccess};
use crate::ssh::{Auth, Conn, Target};

/// A login that can reach the server-side core commands with root privilege.
pub enum Login {
    /// The administrator recorded by server setup, authenticated with the terminal's app key.
    Admin,
    /// A provider credential held only for this session, with its confirmed host-key pin.
    Provider { first: FirstAccess, pin: String },
}

impl std::fmt::Debug for Login {
    /// Show the login kind without formatting provider credentials or sudo secrets.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Admin => f.write_str("Admin"),
            Self::Provider { .. } => f.write_str("Provider { .. }"),
        }
    }
}

/// Open `target` with `login`, verify its pinned host key, and detect how it reaches root.
pub fn open(target: &Target, login: &Login) -> anyhow::Result<(Conn, Privilege)> {
    match login {
        Login::Admin => {
            let hosts = Hosts::load(&Hosts::path())?;
            let host = hosts
                .get(&target.addr())
                .ok_or_else(|| anyhow::anyhow!("{} was never set up here", target.addr()))?;
            let user = host
                .admin
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("{}: the setup did not finish", target.addr()))?;
            let app = app_key::load_or_create()?;
            let conn = Conn::open(target, &Auth::Key { user, key: &app }, &host.fingerprint)?;
            let privilege = Privilege::detect(&conn, None)?;
            Ok((conn, privilege))
        }
        Login::Provider { first, pin } => setup::provider_login(target, first, pin),
    }
}
