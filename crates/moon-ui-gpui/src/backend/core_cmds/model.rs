//! Shared data for core server commands; worker and window behavior lands in later batches.

use std::collections::HashMap;
use std::sync::mpsc;
use std::time::Instant;

use moon_remote::core_cmds::{Cmd, Located, LogLines, LogTail, ServiceInfo};
use moon_remote::core_new::{CoreState, CoreStatus, Exchange};
use moon_remote::setup::FirstAccess;
use zeroize::Zeroizing;

/// The command menu's form for the current login and location knowledge.
#[allow(dead_code)] // used from batch 3
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MenuForm {
    Ours,
    Hand,
    NoLogin,
    /// A login exists but no `Located` value has been cached yet.
    Checking,
}

/// A core command failure suitable for the command window.
#[allow(dead_code)] // used from batch 3
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CmdError {
    Ssh(String),
    NoCore { port: u16 },
    NotOurs,
    Refused(String),
    Timeout,
}

/// The lifecycle of one command for a core.
#[allow(dead_code)] // used from batch 3
#[derive(Debug)]
pub(crate) enum CmdPhase {
    Idle,
    Running {
        since: Instant,
        seen: Option<CoreState>,
    },
    Done,
    Failed(CmdError),
}

/// Cached server facts and command results for one configured core.
#[allow(dead_code)] // used from batch 3
pub(crate) struct CoreFacts {
    pub(crate) target: String,
    pub(crate) port: u16,
    pub(crate) located: Option<Located>,
    pub(crate) status: Option<CoreStatus>,
    pub(crate) info: Option<ServiceInfo>,
    pub(crate) log: Option<LogTail>,
    pub(crate) key: Option<Zeroizing<String>>,
    pub(crate) exchanges: Option<Vec<Exchange>>,
    pub(crate) phases: HashMap<Cmd, CmdPhase>,
    pub(crate) checked_at: Option<Instant>,
}

impl std::fmt::Debug for CoreFacts {
    /// Format core facts without exposing the MoonProto connection key.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CoreFacts")
            .field("target", &self.target)
            .field("port", &self.port)
            .field("located", &self.located)
            .field("status", &self.status)
            .field("info", &self.info)
            .field("log", &self.log)
            .field("key", &self.key.as_ref().map(|_| "<hidden>"))
            .field("exchanges", &self.exchanges)
            .field("phases", &self.phases)
            .field("checked_at", &self.checked_at)
            .finish()
    }
}

/// A request queued for a core's command worker.
#[allow(dead_code)] // used from batch 3
#[derive(Debug)]
pub(crate) enum CmdRequest {
    Run { cmd: Cmd, args: CmdArgs },
    Refresh,
    Forget,
}

/// Arguments for a core command, with secret material held in zeroizing storage.
#[allow(dead_code)] // used from batch 3
pub(crate) enum CmdArgs {
    None,
    Log(LogLines),
    Exchange(String),
    ApiKeys {
        key: Zeroizing<String>,
        secret: Zeroizing<String>,
    },
    Address {
        host: String,
        port: u16,
    },
}

impl std::fmt::Debug for CmdArgs {
    /// Format command arguments without exposing either exchange API credential.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::None => f.write_str("None"),
            Self::Log(lines) => f.debug_tuple("Log").field(lines).finish(),
            Self::Exchange(exchange) => f.debug_tuple("Exchange").field(exchange).finish(),
            Self::ApiKeys { .. } => f
                .debug_struct("ApiKeys")
                .field("key", &"<hidden>")
                .field("secret", &"<hidden>")
                .finish(),
            Self::Address { host, port } => f
                .debug_struct("Address")
                .field("host", host)
                .field("port", port)
                .finish(),
        }
    }
}

/// A provider login held only for this terminal session.
#[allow(dead_code)] // used from batch 3
pub(crate) struct SessionLogin {
    pub(crate) target: String,
    pub(crate) first: FirstAccess,
    pub(crate) pin: String,
}

impl std::fmt::Debug for SessionLogin {
    /// Format a session login without exposing its password, private key, or sudo secret.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionLogin")
            .field("target", &self.target)
            .field("first", &"<hidden>")
            .field("pin", &self.pin)
            .finish()
    }
}

/// Backend-owned queues, cached facts, session logins, and worker channels for core commands.
#[allow(dead_code)] // used from batch 3
#[derive(Default)]
pub(crate) struct CoreCmdsState {
    pub pending: Vec<(u64, CmdRequest)>,
    pub facts: HashMap<u64, CoreFacts>,
    pub logins: HashMap<String, SessionLogin>,
    pub workers: HashMap<u64, mpsc::Sender<CmdRequest>>,
}

/// Choose the menu form from login presence and the cached installation identity.
#[allow(dead_code)] // used from batch 3
pub(crate) fn menu_form(has_login: bool, located: Option<&Located>) -> MenuForm {
    if !has_login {
        return MenuForm::NoLogin;
    }
    match located {
        None => MenuForm::Checking,
        Some(located) if located.ours => MenuForm::Ours,
        Some(_) => MenuForm::Hand,
    }
}
