//! Which cores the station connects to.
//!
//! `station.toml` lists them, without their keys (`STATION.md` §5.1). Each active core's key is
//! the systemd credential `core-<uid>`: the unit loads it with `LoadCredentialEncrypted=`, and
//! the station reads it from `$CREDENTIALS_DIRECTORY` — a per-service ramfs, so the key is never a
//! file on the server's disk in the clear. `moon-remote cores` writes both halves.
//!
//! ```toml
//! [[core]]
//! uid = 3
//! name = "BinF1"
//! transport = "v1"   # optional; the key's own mode when absent
//! ```
//!
//! A `key =` line is refused rather than ignored: a key in a plain file is exactly what this
//! layout exists to prevent.
//!
//! Without the file, a build with the `terminal-config` feature reads the terminal's own
//! configuration from the same data root (a copy of its `cfg/`), which is how the probe runs on
//! the developer's machine. A COPY: loading may assign uids to entries that lack one and save
//! them, without the terminal's durable-store uid floor.

use std::path::Path;

use anyhow::Context;
use moon_core::config::{AppConfig, FeedFlags, Secret, ServerConfig, TransportVersion};
use serde::Deserialize;
use zeroize::Zeroizing;

/// `station.toml`.
#[derive(Deserialize)]
struct StationFile {
    #[serde(default, rename = "core")]
    cores: Vec<CoreEntry>,
}

/// One `[[core]]`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CoreEntry {
    /// The terminal's uid for this core, so report rows and traces carry the same key in both.
    /// Also names its credential, `core-<uid>`.
    uid: u64,
    name: String,
    /// Skip a core without deleting its entry.
    #[serde(default = "default_true")]
    active: bool,
    /// A transport mode the terminal overrides the key's with.
    #[serde(default)]
    transport: Option<TransportVersion>,
}

/// What the station reads from a core: its reports alone (`STATION.md` §3.2). Above all no
/// `log`: with it the feed writes every core's log to the data root, and the station keeps no
/// logs. The clock offset still samples `ServerLog` — that pass ignores this flag — and stores
/// nothing but the offset.
const STATION_FEED: FeedFlags = FeedFlags {
    orders: false,
    detects: false,
    reports: true,
    balance: false,
    strategies: false,
    log: false,
    alerts: false,
    arb: false,
};

fn default_true() -> bool {
    true
}

/// The station's configuration: every core of `station.toml` at `path`, or of the terminal's own
/// files when there is none.
pub fn load(path: &Path) -> anyhow::Result<AppConfig> {
    if path.exists() {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
        let creds = std::env::var_os("CREDENTIALS_DIRECTORY").map(std::path::PathBuf::from);
        return from_station_file(&text, creds.as_deref())
            .with_context(|| format!("parse {}", path.display()));
    }
    terminal_config(path)
}

/// `creds` is the credentials directory; `None` when the process was not given one.
fn from_station_file(text: &str, creds: Option<&Path>) -> anyhow::Result<AppConfig> {
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
            if entry.active {
                server.key =
                    core_key(creds, entry.uid).with_context(|| format!("core {:?}", entry.name))?;
            }
            server.name = entry.name;
            server.active = entry.active;
            server.feed = STATION_FEED;
            server.transport = entry.transport;
            Ok(server)
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    Ok(AppConfig::headless(servers))
}

/// The credential `core-<uid>`, as systemd placed it.
fn core_key(creds: Option<&Path>, uid: u64) -> anyhow::Result<Secret> {
    let dir = creds.ok_or_else(|| {
        anyhow::anyhow!(
            "no $CREDENTIALS_DIRECTORY: run under systemd with LoadCredentialEncrypted=core-{uid}:…"
        )
    })?;
    let path = dir.join(format!("core-{uid}"));
    let text = Zeroizing::new(
        std::fs::read_to_string(&path).with_context(|| format!("read credential core-{uid}"))?,
    );
    let key = text.trim();
    anyhow::ensure!(!key.is_empty(), "credential core-{uid} is empty");
    Ok(Secret::new(key))
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
