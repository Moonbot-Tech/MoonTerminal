//! `remote/hosts.toml`: what the terminal knows about each server it prepared — the host key it
//! pinned at first contact and the administrator it logs in as. Nothing here is secret; the app's
//! private key lives sealed in `app_key.enc`.
//!
//! ```toml
//! [[host]]
//! addr = "203.0.113.7:22"
//! fingerprint = "SHA256:…"
//! admin = "moon"
//! ```

use std::path::{Path, PathBuf};

use anyhow::Context;
use serde::{Deserialize, Serialize};

#[derive(Default, Serialize, Deserialize)]
pub struct Hosts {
    #[serde(default, rename = "host")]
    hosts: Vec<Host>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Host {
    /// `host:port`, exactly as the user typed the host.
    pub addr: String,
    /// The SHA256 fingerprint pinned at first contact. A different key later is a hard refusal.
    pub fingerprint: String,
    /// The administrator the setup created; `None` until that step has been verified.
    #[serde(default)]
    pub admin: Option<String>,
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

    /// Pin `fingerprint` for `addr`. Refuses to replace a different pin: a changed host key is
    /// either a reinstalled server or someone in the middle, and only the user can tell which —
    /// by deleting the entry by hand.
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
}

#[cfg(test)]
mod tests;
