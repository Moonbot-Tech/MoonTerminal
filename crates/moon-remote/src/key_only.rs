//! Closing password logins on a server a MoonBot core was put on.
//!
//! A provider hands a server over with a root password, and a core's install leaves sshd as it
//! found it: the server then answers password guesses for as long as it runs, with the exchange
//! keys of its cores on it. [`close`] turns the password logins off — and only for a login that
//! came in by a key, so the way in the user holds keeps working: no account and no key is added,
//! and root's own login stays as it was (the core runs as root, its user logs in as root).
//!
//! The order is the station setup's (`setup.rs`): sshd is changed through the login that is
//! already open, the result is looked at from outside through NEW connections, and a look that
//! fails is rolled back through the open one.
//!
//! A closed server stays closed: taking the core out again (`core_move::remove`, a cancelled
//! move) does not reopen the password logins. They belong to the server, not to the instance —
//! other cores may run there, and the key that closed it still gets in.

use anyhow::Context;
use russh::keys::PrivateKey;

use crate::script::{self, Privilege, STEP_TIMEOUT};
use crate::ssh::{Auth, Conn, OpenError, Target};

/// What [`close`] found or did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Closed {
    /// This call turned the password logins off.
    Now,
    /// sshd took no password before the call; nothing was changed.
    Already,
}

/// Turn password logins off on the server `conn` is logged in to, and check from outside that
/// `user` still gets in with `key` while no password is asked for.
///
/// `conn` must be a login of `user` by `key` against the host key `pin`: it stays open as the
/// way back. A check that fails after sshd was changed puts sshd back as it was and returns the
/// failure; on a server that was closed already there is nothing to put back.
pub fn close(
    conn: &Conn,
    privilege: &Privilege,
    target: &Target,
    user: &str,
    key: &PrivateKey,
    pin: &str,
) -> anyhow::Result<Closed> {
    let out = privilege
        .run(conn, &script::bootstrap("key-only", &[]), &[], STEP_TIMEOUT)
        .context("close the password logins")?;
    let closed = said(&out.stdout_text())?;
    let Err(e) = seen_closed(target, user, key, pin) else {
        return Ok(closed);
    };
    if closed == Closed::Already {
        return Err(e);
    }
    let undo = privilege.run(
        conn,
        &script::bootstrap("key-only-off", &[]),
        &[],
        STEP_TIMEOUT,
    );
    Err(match undo {
        Ok(_) => e.context("closing the password logins failed; sshd is back as it was"),
        Err(undo) => e.context(format!(
            "closing the password logins failed AND the rollback failed: {undo:#}"
        )),
    })
}

/// What the `key-only` step reported on its `sshd=` line.
fn said(stdout: &str) -> anyhow::Result<Closed> {
    match script::value(stdout, "sshd") {
        Some("changed") => Ok(Closed::Now),
        Some("already") => Ok(Closed::Already),
        _ => anyhow::bail!("the step did not say what it did: {}", stdout.trim()),
    }
}

/// The closed server, seen from outside: `key` still opens a new login, and a login that offers
/// no credential is told that no password would be taken.
fn seen_closed(target: &Target, user: &str, key: &PrivateKey, pin: &str) -> anyhow::Result<()> {
    Conn::open(target, &Auth::Key { user, key }, pin)
        .map_err(anyhow::Error::new)
        .with_context(|| format!("{user} no longer gets in with the key"))?;
    password_refused(Conn::open(target, &Auth::None { user }, pin), user)
}

/// Read the server's answer to a login that offered no credential: closed means a refusal that
/// names neither `password` nor `keyboard-interactive` among the methods left to try.
pub(crate) fn password_refused<T>(answer: Result<T, OpenError>, user: &str) -> anyhow::Result<()> {
    match answer {
        Err(OpenError::Refused {
            password_offered: false,
        }) => Ok(()),
        Err(OpenError::Refused {
            password_offered: true,
        }) => anyhow::bail!("the server still offers a password login"),
        Ok(_) => anyhow::bail!("the server let {user} in without any credential"),
        Err(e) => Err(anyhow::Error::new(e).context("asking the server which logins it takes")),
    }
}

#[cfg(test)]
mod tests;
