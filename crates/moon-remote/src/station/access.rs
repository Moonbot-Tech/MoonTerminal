//! Explicit station address changes and credential removal. A probe never authenticates;
//! only a confirmed fingerprint permits key login. Removal keeps the administrator reachable.

use anyhow::Context;

use crate::app_key;
use crate::hosts::{Host, HostEditError, Hosts};
use crate::script::{self, STEP_TIMEOUT};
use crate::ssh::{Auth, Conn, OpenError, Target};

/// The exact source record, destination and fingerprint the user must confirm together.
#[derive(Clone)]
pub struct AddressChange {
    pub source: Host,
    pub target: Target,
    pub fingerprint: String,
}

/// Observe a destination key without authenticating or changing any pin. An impossible pin
/// makes the existing SSH verifier refuse at key exchange, before it sends login credentials.
pub fn probe_address(source: Host, target: Target) -> anyhow::Result<AddressChange> {
    Hosts::load(&Hosts::path())?.check_address(&source, &target.addr())?;
    let app = app_key::load_or_create()?;
    let auth = Auth::Key {
        user: source.admin.as_deref().ok_or(HostEditError::NotSetUp)?,
        key: &app,
    };
    match Conn::open(&target, &auth, Some("")) {
        Err(OpenError::HostKeyChanged { presented, .. }) => Ok(AddressChange {
            source,
            target,
            fingerprint: presented,
        }),
        Err(e) => Err(e.into()),
        Ok(_) => Err(HostEditError::NoFingerprint.into()),
    }
}

/// After explicit UI confirmation, verify the same key and administrator at the destination,
/// then move the local record. A changed key, failed login or stale record retains the old one.
pub fn change_address(change: &AddressChange) -> anyhow::Result<()> {
    Hosts::load(&Hosts::path())?.check_address(&change.source, &change.target.addr())?;
    let app = app_key::load_or_create()?;
    let admin = change
        .source
        .admin
        .as_deref()
        .ok_or(HostEditError::NotSetUp)?;
    let _conn = Conn::open(
        &change.target,
        &Auth::Key {
            user: admin,
            key: &app,
        },
        Some(&change.fingerprint),
    )?;
    let path = Hosts::path();
    let mut hosts = Hosts::load(&path)?;
    hosts.change_address(&change.source, &change.target.addr(), &change.fingerprint)?;
    hosts.save(&path)
}

/// Removal facts the UI must distinguish, especially after the remote wipe already succeeded.
#[derive(Debug)]
pub enum RemovalError {
    Unsupported,
    Unconfirmed,
    LocalForgetFailed,
    NotConfigured,
}

impl std::fmt::Display for RemovalError {
    /// Keep the core fact language-neutral for callers that provide localized advice.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Unsupported => "the installed helper does not support station removal",
            Self::Unconfirmed => "the helper did not confirm station removal",
            Self::LocalForgetFailed => "station removed, but the local server record was kept",
            Self::NotConfigured => {
                "the station has no configuration; automatic core transfer refused"
            }
        })
    }
}

impl std::error::Error for RemovalError {}

/// Refuse missing or unknown capabilities before issuing any destructive command.
fn require_removal(status: &str) -> anyhow::Result<()> {
    if script::value(status, "remove_station") != Some("yes")
        || script::value(status, "removal_guard") != Some("yes")
    {
        return Err(RemovalError::Unsupported.into());
    }
    Ok(())
}

/// Stop and disable the station and delete its trading/bot secrets, then forget its local pin.
/// Older helpers are refused without automatic replacement: the user updates the service first.
/// A failure retains the host record so the administrator can retry with the same key.
pub fn remove_station(source: &Host) -> anyhow::Result<()> {
    let path = Hosts::path();
    if Hosts::load(&path)?.get(&source.addr) != Some(source) {
        return Err(HostEditError::Changed.into());
    }
    let (host, port) = source
        .addr
        .rsplit_once(':')
        .ok_or_else(|| anyhow::anyhow!("invalid station address"))?;
    let target = Target {
        host: host.trim_matches(['[', ']']).to_owned(),
        port: port.parse()?,
    };
    let conn = super::admin_conn(&target)?;
    let status = script::checked(conn.run(&script::helper("status", &[]), &[], STEP_TIMEOUT)?)?
        .stdout_text();
    require_removal(&status)?;
    let out =
        script::checked(conn.run(&script::helper("remove-station", &[]), &[], STEP_TIMEOUT)?)?;
    if script::value(&out.stdout_text(), "removed") != Some("yes") {
        return Err(RemovalError::Unconfirmed.into());
    }
    let mut hosts = Hosts::load(&path).context(RemovalError::LocalForgetFailed)?;
    if hosts.get(&source.addr) != Some(source) {
        return Err(RemovalError::LocalForgetFailed.into());
    }
    hosts.forget(&source.addr);
    hosts.save(&path).context(RemovalError::LocalForgetFailed)
}

#[cfg(test)]
mod tests;
