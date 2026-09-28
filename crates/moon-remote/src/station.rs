//! The station's cores on a prepared server: each core's key into its own encrypted credential,
//! `station.toml` without a key, a restart.
//!
//! Only to a server `setup` closed: the administrator must already be recorded in `hosts.toml`,
//! which `setup` writes only after its key login and sudo rule were verified. A key goes on the
//! SSH channel's stdin straight into `systemd-creds encrypt` — never argv, never a file.

use anyhow::Context;
use moon_core::config::{Secret, TransportVersion};
use serde::Serialize;

use crate::app_key;
use crate::hosts::Hosts;
use crate::script::{self, STEP_TIMEOUT};
use crate::ssh::{Auth, Conn, Target};

/// One core for the station.
pub struct CoreKey {
    pub uid: u64,
    pub name: String,
    pub transport: Option<TransportVersion>,
    pub key: Secret,
}

/// The terminal's window around a trade (`[trade_replay]`), sent as the station's `[tape]`.
#[derive(Clone, Copy, Serialize)]
pub struct TapeWindow {
    pub margin_s: u32,
    pub long_position_min: u32,
}

/// `station.toml` as the station reads it: no key, the credential is `core-<uid>`.
#[derive(Serialize)]
struct StationFile<'a> {
    core: Vec<StationCore<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tape: Option<TapeWindow>,
}

#[derive(Serialize)]
struct StationCore<'a> {
    uid: u64,
    name: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    transport: Option<TransportVersion>,
}

/// The station's `station.toml` for `cores`; without `tape` the station keeps its own window.
pub fn station_toml(cores: &[CoreKey], tape: Option<TapeWindow>) -> anyhow::Result<String> {
    let file = StationFile {
        core: cores
            .iter()
            .map(|c| StationCore {
                uid: c.uid,
                name: &c.name,
                transport: c.transport,
            })
            .collect(),
        tape,
    };
    Ok(toml::to_string_pretty(&file)?)
}

/// The administrator's connection to a server `setup` closed.
pub fn admin_conn(target: &Target) -> anyhow::Result<Conn> {
    let hosts = Hosts::load(&Hosts::path())?;
    let host = hosts
        .get(&target.addr())
        .ok_or_else(|| anyhow::anyhow!("{} was never set up here", target.addr()))?;
    let admin = host
        .admin
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("{}: the setup did not finish", target.addr()))?;
    let app = app_key::load_or_create()?;
    Ok(Conn::open(
        target,
        &Auth::Key {
            user: admin,
            key: &app,
        },
        Some(&host.fingerprint),
    )?)
}

/// Make `cores` the station's whole set: credentials for these, none for any other, the matching
/// `station.toml`, and the service (re)started.
pub fn push_cores(
    target: &Target,
    cores: &[CoreKey],
    tape: Option<TapeWindow>,
    say: &mut dyn FnMut(&str),
) -> anyhow::Result<()> {
    anyhow::ensure!(!cores.is_empty(), "no cores to send");
    let mut seen = std::collections::HashSet::new();
    for core in cores {
        anyhow::ensure!(core.uid != 0, "core {:?} has no uid", core.name);
        anyhow::ensure!(seen.insert(core.uid), "uid {} twice", core.uid);
        anyhow::ensure!(!core.key.is_empty(), "core {:?} has no key", core.name);
    }
    let conn = admin_conn(target)?;
    let run = |command: String, stdin: &[u8]| -> anyhow::Result<String> {
        Ok(script::checked(conn.run(&command, stdin, STEP_TIMEOUT)?)?.stdout_text())
    };

    for core in cores {
        let uid = core.uid.to_string();
        run(
            script::helper("put-cred", &[&uid]),
            core.key.expose().as_bytes(),
        )
        .with_context(|| format!("credential of {}", core.name))?;
        say(&format!(
            "core {} ({}): credential written",
            core.name, core.uid
        ));
    }
    let status = run(script::helper("status", &[]), &[])?;
    let wanted: std::collections::HashSet<String> =
        cores.iter().map(|c| format!("core-{}", c.uid)).collect();
    for stale in script::value(&status, "creds")
        .unwrap_or_default()
        .split_whitespace()
        .filter(|name| !wanted.contains(*name))
    {
        let uid = stale.trim_start_matches("core-");
        run(script::helper("drop-cred", &[uid]), &[])?;
        say(&format!("{stale}: dropped, not in the set"));
    }
    run(
        script::helper("put-config", &[]),
        station_toml(cores, tape)?.as_bytes(),
    )?;
    run(script::helper("start", &[]), &[])?;
    let status = run(script::helper("status", &[]), &[])?;
    for line in status.lines() {
        say(line);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
