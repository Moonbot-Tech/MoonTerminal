//! Station progress separates readable steps from helper diagnostics.

/// A completed or pending station step, localized by the consuming UI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// Read a candidate destination key before asking for user consent.
    AddressProbe,
    /// Authenticate only against the destination key the user confirmed.
    AddressVerify,
    /// Stop remote polling and remove trading and bot credentials.
    StationRemove,
    /// Read a new server's host key, before any credential, for the user to confirm.
    InstallProbe,
    Login,
    Pin,
    Probe,
    Admin,
    Helper,
    Service,
    Harden,
    Firewall,
    /// An inactive firewall is about to close the ports that listen now; the step's diagnostic
    /// is their list (`443/tcp, 51820/udp`).
    FirewallWillClose,
    /// The ports that listen now could not be listed before the firewall closes them.
    FirewallPortsUnknown,
    Extras,
    Install,
    Download,
    TapePending,
    TapeWritten,
    TapeReload,
    TapeApplied,
    TokenWritten,
    TokenDropped,
    ChatsDropped,
    CoreWritten,
    CoreDropped,
    Status,
    Update,
    NoNewRelease,
    Unversioned,
    ValuationSend,
    ValuationKept,
    ValuationWritten,
    ChatsTransferred,
    BotStarted,
    UndoHandover,
    BotRemoved,
    BotReady,
}

/// Helper output is for the app log; only steps and UI-produced text reach progress.
#[derive(Debug)]
pub enum Progress {
    Step {
        step: Step,
        diagnostic: String,
    },
    Diagnostic(String),
    /// Already localized text produced by the terminal, never a helper response.
    Text(String),
}

impl Progress {
    /// Retain CLI diagnostics while giving the UI a language-independent step.
    pub fn step(step: Step, diagnostic: impl Into<String>) -> Self {
        Self::Step {
            step,
            diagnostic: diagnostic.into(),
        }
    }
}

impl std::fmt::Display for Progress {
    /// The command-line tool retains its existing diagnostic output.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Step { diagnostic, .. }
            | Self::Diagnostic(diagnostic)
            | Self::Text(diagnostic) => f.write_str(diagnostic),
        }
    }
}
