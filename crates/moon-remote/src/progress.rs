//! Station progress separates readable steps from helper diagnostics.

/// A completed or pending station step, localized by the consuming UI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Login,
    Pin,
    Probe,
    Admin,
    Helper,
    Service,
    Harden,
    Firewall,
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
