//! `remote/hosts.toml`: what the terminal knows about each server it prepared — the host key the
//! user confirmed and the account it logs in as with its app key: the station's administrator,
//! or the login of a core's server whose password logins a core install closed. Nothing here is
//! secret; the app's private key lives sealed in `app_key.enc`.
//!
//! ```toml
//! [[host]]
//! addr = "203.0.113.7:22"
//! fingerprint = "SHA256:…"
//! admin = "moon"
//!
//! [[host]]
//! addr = "198.51.100.4:22"
//! fingerprint = "SHA256:…"
//! key_login = "root"
//! ```

use std::path::{Path, PathBuf};

use anyhow::Context;
use serde::{Deserialize, Serialize};

/// A rejected host-record edit, kept typed so the terminal can explain it in the user's locale.
#[derive(Debug)]
pub enum HostEditError {
    Changed,
    NotSetUp,
    SameAddress,
    AddressKnown,
    NoFingerprint,
}

impl std::fmt::Display for HostEditError {
    /// Stable diagnostic facts; user-facing recovery advice belongs to the UI.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Changed => "the known server changed; read it again",
            Self::NotSetUp => "the server setup did not finish",
            Self::SameAddress => "the address has not changed",
            Self::AddressKnown => "the destination is already a known server",
            Self::NoFingerprint => "no confirmed host key",
        })
    }
}

impl std::error::Error for HostEditError {}

#[derive(Default, Serialize, Deserialize)]
pub struct Hosts {
    #[serde(default, rename = "host")]
    hosts: Vec<Host>,
}

/// A pinned server and the verified administrator whose key login survives address changes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Host {
    /// `host:port`, exactly as the user typed the host.
    pub addr: String,
    /// The SHA256 fingerprint of the host key the user confirmed. A different key later is a hard
    /// refusal.
    pub fingerprint: String,
    /// The administrator the setup created; `None` until that step has been verified.
    #[serde(default)]
    pub admin: Option<String>,
    /// A core server's own login (typically `root`) that a core install authorized the app key
    /// for and proved from a new connection, before it turned the password logins off. Kept apart
    /// from `admin`: that one marks the station's server, and a core server is not one — the
    /// station's lookups ([`Hosts::first_set_up`], the address change) read `admin` only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_login: Option<String>,
}

impl Host {
    /// The account the app key logs in as: the station's administrator, else a core server's
    /// proven login.
    pub fn login_user(&self) -> Option<&str> {
        self.admin.as_deref().or(self.key_login.as_deref())
    }
}

impl Hosts {
    pub fn path() -> PathBuf {
        moon_core::config::paths::remote_dir().join("hosts.toml")
    }

    pub fn load(path: &Path) -> anyhow::Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text).with_context(|| format!("parse {}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e).with_context(|| format!("read {}", path.display())),
        }
    }

    /// Written whole, through a temporary file, so a crash never leaves half a pin behind.
    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, toml::to_string_pretty(self)?)
            .with_context(|| format!("write {}", tmp.display()))?;
        std::fs::rename(&tmp, path).with_context(|| format!("replace {}", path.display()))
    }

    pub fn get(&self, addr: &str) -> Option<&Host> {
        self.hosts.iter().find(|h| h.addr == addr)
    }

    /// Find a host the app key logs in to ([`Host::login_user`]) by the IPv4 host part of its
    /// pinned `host:port` address.
    pub fn app_login_by_ip(&self, ip: &str) -> Option<&Host> {
        self.hosts.iter().find(|host| {
            host.login_user().is_some()
                && host
                    .addr
                    .rsplit_once(':')
                    .is_some_and(|(host, _port)| host == ip)
        })
    }

    /// Forget `addr`: its pinned key and recorded login. The way back to a server that was
    /// reinstalled — it presents a new host key, which the pin refuses.
    pub fn forget(&mut self, addr: &str) -> bool {
        let before = self.hosts.len();
        self.hosts.retain(|h| h.addr != addr);
        self.hosts.len() != before
    }

    /// Validate a proposed address change against the host the user saw. Refuses stale records,
    /// unfinished setups and collisions so an existing administrator is never lost or replaced.
    pub fn check_address(&self, source: &Host, addr: &str) -> anyhow::Result<()> {
        if self.get(&source.addr) != Some(source) {
            return Err(HostEditError::Changed.into());
        }
        if source.admin.is_none() {
            return Err(HostEditError::NotSetUp.into());
        }
        if addr == source.addr {
            return Err(HostEditError::SameAddress.into());
        }
        if self.get(addr).is_some() {
            return Err(HostEditError::AddressKnown.into());
        }
        Ok(())
    }

    /// Move a verified record to the explicitly confirmed address and fingerprint, preserving
    /// its administrator and list position. The caller verifies key login before saving this.
    pub fn change_address(
        &mut self,
        source: &Host,
        addr: &str,
        fingerprint: &str,
    ) -> anyhow::Result<()> {
        self.check_address(source, addr)?;
        if fingerprint.is_empty() {
            return Err(HostEditError::NoFingerprint.into());
        }
        let host = self
            .hosts
            .iter_mut()
            .find(|h| h.addr == source.addr)
            .ok_or_else(|| anyhow::anyhow!("the known server disappeared"))?;
        host.addr = addr.to_owned();
        host.fingerprint = fingerprint.to_owned();
        Ok(())
    }

    /// The first host whose setup finished — its administrator verified.
    pub fn first_set_up(&self) -> Option<&Host> {
        self.hosts.iter().find(|h| h.admin.is_some())
    }

    /// Pin `fingerprint` for `addr`. Refuses to replace a different pin: a changed host key is
    /// either a reinstalled server or someone in the middle, and only the user can tell which —
    /// through the terminal's explicit address-change or forget confirmation.
    pub fn pin(&mut self, addr: &str, fingerprint: &str) -> anyhow::Result<()> {
        match self.hosts.iter().find(|h| h.addr == addr) {
            Some(host) if host.fingerprint == fingerprint => Ok(()),
            Some(host) => anyhow::bail!(
                "{addr}: pinned {}, presented {fingerprint}",
                host.fingerprint
            ),
            None => {
                self.hosts.push(Host {
                    addr: addr.to_owned(),
                    fingerprint: fingerprint.to_owned(),
                    admin: None,
                    key_login: None,
                });
                Ok(())
            }
        }
    }

    /// Record the verified administrator of an already pinned host.
    pub fn set_admin(&mut self, addr: &str, admin: &str) -> anyhow::Result<()> {
        let host = self
            .hosts
            .iter_mut()
            .find(|h| h.addr == addr)
            .ok_or_else(|| anyhow::anyhow!("{addr} is not pinned"))?;
        host.admin = Some(admin.to_owned());
        Ok(())
    }

    /// Record the core-server login the app key was proven for on an already pinned host.
    pub fn set_key_login(&mut self, addr: &str, login: &str) -> anyhow::Result<()> {
        let host = self
            .hosts
            .iter_mut()
            .find(|h| h.addr == addr)
            .ok_or_else(|| anyhow::anyhow!("{addr} is not pinned"))?;
        host.key_login = Some(login.to_owned());
        Ok(())
    }
}

#[cfg(test)]
mod tests;
