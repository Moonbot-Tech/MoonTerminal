//! Which cores the station connects to.
//!
//! `station.toml` in the data root lists them. The keys sit in that file for now — a probe on a
//! test server; the installer phase moves them into systemd credentials and leaves only the
//! non-secret part here (`STATION.md` §3).
//!
//! ```toml
//! [[core]]
//! uid = 3
//! name = "BinF1"
//! key = "<exported MoonBot key>"
//! transport = "v1"   # optional; the key's own mode when absent
//! ```
//!
//! Without the file, a build with the `terminal-config` feature reads the terminal's own
//! configuration from the same data root (a copy of its `cfg/`), which is how the probe runs on
//! the developer's machine. A COPY: loading may assign uids to entries that lack one and save
//! them, without the terminal's durable-store uid floor.

use std::path::Path;

use anyhow::Context;
use moon_core::config::{AppConfig, Secret, ServerConfig, TransportVersion};
use serde::Deserialize;

/// `station.toml`.
#[derive(Deserialize)]
struct StationFile {
    #[serde(default, rename = "core")]
    cores: Vec<CoreEntry>,
}

/// One `[[core]]`.
#[derive(Deserialize)]
struct CoreEntry {
    /// The terminal's uid for this core, so report rows and traces carry the same key in both.
    uid: u64,
    name: String,
    key: String,
    /// Skip a core without deleting its entry.
    #[serde(default = "default_true")]
    active: bool,
    /// A transport mode the terminal overrides the key's with.
    #[serde(default)]
    transport: Option<TransportVersion>,
}

fn default_true() -> bool {
    true
}

/// The station's configuration: every core of `station.toml`, or of the terminal's own files.
pub fn load(data_root: &Path) -> anyhow::Result<AppConfig> {
    let path = data_root.join("station.toml");
    if path.exists() {
        let text =
            std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
        return from_station_file(&text).with_context(|| format!("parse {}", path.display()));
    }
    terminal_config(&path)
}

fn from_station_file(text: &str) -> anyhow::Result<AppConfig> {
    let file: StationFile = toml::from_str(text)?;
    anyhow::ensure!(!file.cores.is_empty(), "no [[core]] entries");
    // A uid keys a core's reports, traces and tape: two entries sharing one would merge two
    // cores' histories, and 0 means "unassigned" everywhere in the terminal.
    let mut seen = std::collections::HashSet::new();
    for entry in &file.cores {
        anyhow::ensure!(entry.uid != 0, "core {:?}: uid 0 is not a uid", entry.name);
        anyhow::ensure!(
            seen.insert(entry.uid),
            "core {:?}: uid {} is used twice",
            entry.name,
            entry.uid
        );
    }
    let servers = file
        .cores
        .into_iter()
        .map(|entry| {
            // Every other field keeps the terminal's own default for a new server.
            // `id` alone has no serde default; it is set from the uid below.
            let mut server: ServerConfig = toml::from_str("id = 0")?;
            server.uid = entry.uid;
            server.name = entry.name;
            server.key = Secret::new(entry.key);
            server.active = entry.active;
            server.transport = entry.transport;
            Ok(server)
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    Ok(AppConfig::headless(servers))
}

#[cfg(feature = "terminal-config")]
fn terminal_config(_missing: &Path) -> anyhow::Result<AppConfig> {
    log::info!("no station.toml: reading the terminal's configuration from the data root");
    AppConfig::load(None, false)
}

#[cfg(not(feature = "terminal-config"))]
fn terminal_config(missing: &Path) -> anyhow::Result<AppConfig> {
    anyhow::bail!(
        "{} not found: it lists the cores to connect to",
        missing.display()
    )
}

#[cfg(test)]
mod tests;
