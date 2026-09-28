//! The server-side scripts this crate carries, and how a command reaches root.
//!
//! The scripts are embedded, not installed from anywhere: the binary that runs the setup is the
//! one whose scripts run on the server. `bootstrap.sh` travels in the command line each time
//! (`sh -c '<script>' …`), so stdin stays free for the secrets a step reads.

use std::time::Duration;

use zeroize::Zeroizing;

use crate::ssh::{Conn, Output};

/// The one-time, full-privilege steps.
pub const BOOTSTRAP: &str = include_str!("../remote/bootstrap.sh");
/// The narrow commands the administrator may run without a password.
pub const HELPER: &str = include_str!("../remote/moon-station-admin.sh");
/// The station's unit.
pub const UNIT: &str = include_str!("../remote/moon-station.service");
/// Where `bootstrap.sh helper` installs [`HELPER`].
pub const HELPER_PATH: &str = "/usr/local/sbin/moon-station-admin";

/// An ordinary step's deadline.
pub const STEP_TIMEOUT: Duration = Duration::from_secs(120);
/// A step that installs packages: `apt-get update` plus installs on one slow vCPU.
pub const APT_TIMEOUT: Duration = Duration::from_secs(900);

/// Quote `s` as one word for a POSIX shell.
pub fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// How a connection reaches root.
pub enum Privilege {
    /// Logged in as root.
    Root,
    /// `sudo -n` works for anything (a cloud user with `NOPASSWD: ALL`).
    SudoNoPassword,
    /// `sudo` asks for this password. It is the first line of stdin, and `-k` makes sudo ask even
    /// when it has cached credentials: a prompt that did not come would leave the password line
    /// for the script to read as its own input.
    SudoPassword(Zeroizing<String>),
}

impl Privilege {
    /// Find out how `conn` reaches root. `password` is the one sudo would ask for, if it asks.
    pub fn detect(conn: &Conn, password: Option<&Zeroizing<String>>) -> anyhow::Result<Self> {
        if conn.user() == "root" {
            return Ok(Self::Root);
        }
        if conn.run("sudo -n true", &[], STEP_TIMEOUT)?.ok() {
            return Ok(Self::SudoNoPassword);
        }
        match password {
            Some(password) => Ok(Self::SudoPassword(password.clone())),
            None => anyhow::bail!(
                "{} needs a password for sudo and none was given",
                conn.user()
            ),
        }
    }

    /// Run `command` as root with `stdin`; a non-zero exit is an error carrying its stderr.
    pub fn run(
        &self,
        conn: &Conn,
        command: &str,
        stdin: &[u8],
        timeout: Duration,
    ) -> anyhow::Result<Output> {
        let (command, stdin) = match self {
            Self::Root => (command.to_owned(), Zeroizing::new(stdin.to_vec())),
            Self::SudoNoPassword => (format!("sudo -n {command}"), Zeroizing::new(stdin.to_vec())),
            Self::SudoPassword(password) => {
                let mut input =
                    Zeroizing::new(Vec::with_capacity(password.len() + 1 + stdin.len()));
                input.extend_from_slice(password.as_bytes());
                input.push(b'\n');
                input.extend_from_slice(stdin);
                (format!("sudo -k -S -p '' {command}"), input)
            }
        };
        checked(conn.run(&command, &stdin, timeout)?)
    }
}

/// The command line for one bootstrap step.
pub fn bootstrap(step: &str, args: &[&str]) -> String {
    let mut command = format!("sh -c {} moon-bootstrap {step}", sh_quote(BOOTSTRAP));
    for arg in args {
        command.push(' ');
        command.push_str(&sh_quote(arg));
    }
    command
}

/// The command line for one helper command, through its passwordless sudo rule.
pub fn helper(command: &str, args: &[&str]) -> String {
    let mut line = format!("sudo -n {HELPER_PATH} {command}");
    for arg in args {
        line.push(' ');
        line.push_str(&sh_quote(arg));
    }
    line
}

/// A non-zero exit becomes an error with the command's own words.
pub fn checked(out: Output) -> anyhow::Result<Output> {
    if out.ok() {
        return Ok(out);
    }
    let detail = match out.stderr_text() {
        text if text.is_empty() => out.stdout_text().trim().to_owned(),
        text => text,
    };
    match out.status {
        Some(code) => anyhow::bail!("exit {code}: {detail}"),
        None => anyhow::bail!("ended without an exit status: {detail}"),
    }
}

/// `key=value` lines of a step's output.
pub fn value<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    text.lines()
        .filter_map(|line| line.split_once('='))
        .find(|(k, _)| *k == key)
        .map(|(_, v)| v.trim())
}

#[cfg(test)]
mod tests;
