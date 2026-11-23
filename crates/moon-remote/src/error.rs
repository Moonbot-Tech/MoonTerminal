//! Actionable station failures without localization or credential data.

/// The user action needed after station validation or a transport deadline fails.
#[derive(Debug, PartialEq, Eq)]
pub enum StationError {
    EmptyPassword,
    PasswordOneLine,
    NoCorePicked,
    CoreMissing(u64),
    CoreWithoutKey(String),
    HelperTooOld,
    ServiceTooOld,
    TerminalTooOld,
    /// A command on an open connection did not end within its deadline.
    Timeout,
    /// The SSH connection itself got no answer within its deadline. Kept apart from [`Self::Timeout`]
    /// because its causes differ: on a station server, fail2ban blocking this address after failed
    /// logins looks exactly like this.
    ConnectTimeout,
    SystemdMissing,
    SystemdTooOld,
    PasswordStillOffered,
    PasswordStillWorks,
    BotTokenMissing,
    ConfigMissing,
    BotAlreadyPresent,
    BotStopped,
    BotNotReady,
    /// The user has not confirmed the server's host key; nothing was sent to it.
    HostKeyUnconfirmed,
    /// The server has no `apt-get`; the setup needs Debian or Ubuntu.
    NoAptGet,
    /// The server's `sshd_config` does not include `sshd_config.d`.
    SshdNoInclude,
}

impl std::fmt::Display for StationError {
    /// Secret-free diagnostics for command-line callers and secondary details.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CoreMissing(uid) => write!(f, "core {uid} is not in servers.enc"),
            Self::CoreWithoutKey(name) => write!(f, "core {name:?} has no key"),
            kind => f.write_str(match kind {
                Self::EmptyPassword => "the password is empty",
                Self::PasswordOneLine => "a password must be one line",
                Self::NoCorePicked => "no core is picked",
                Self::HelperTooOld => "the station helper is too old: run setup again",
                Self::ServiceTooOld => "the station service is too old: update the service",
                Self::TerminalTooOld => "the terminal is too old: update the terminal",
                Self::Timeout => "the server did not answer within the deadline",
                Self::ConnectTimeout => "the server did not answer the SSH connection in time",
                Self::SystemdMissing => "no systemd on this server",
                Self::SystemdTooOld => "systemd is too old for encrypted credentials",
                Self::PasswordStillOffered => "the server still offers password logins",
                Self::PasswordStillWorks => "a password login still works",
                Self::BotTokenMissing => "the station has no bot token",
                Self::ConfigMissing => "the station has no station.toml: send its cores first",
                Self::BotAlreadyPresent => {
                    "the station already runs a bot: remove it before handing another over"
                }
                Self::BotStopped => "the station stopped after the bot was handed over",
                Self::BotNotReady => "the station bot did not start polling within the deadline",
                Self::HostKeyUnconfirmed => "the server's host key was not confirmed",
                Self::NoAptGet => "no apt-get on this server: Debian or Ubuntu is required",
                Self::SshdNoInclude => "the server's sshd_config does not include sshd_config.d",
                Self::CoreMissing(_) | Self::CoreWithoutKey(_) => unreachable!(),
            }),
        }
    }
}

impl std::error::Error for StationError {}
