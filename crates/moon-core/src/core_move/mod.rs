//! Moving a MoonBot core to a Linux server: everything about it that needs no SSH.
//!
//! - [`key_packet`] — the connection packet: the core keeps its MoonProto key, only the endpoint
//!   changes, so every terminal that knows the core changes just the address;
//! - [`report_db`] — the core's report database rebuilt from this terminal's replica, with the
//!   identity terminals resume by;
//! - [`archive`] — the settings archive the new core starts from;
//! - [`strategies`] — the old core's strategy set written to the new one over MoonProto;
//! - [`SettingsSource`] — where the settings come from.
//!
//! The server side (install, start, roll back) lives in `moon-remote`.

pub mod archive;
pub mod key_packet;
pub mod report_db;
pub mod strategies;

use std::path::PathBuf;

/// Where a move takes the core's settings from.
pub enum SettingsSource {
    /// A settings snapshot MoonBot's VPS wizard saved ("Get settings"), as a ZIP file.
    Snapshot(PathBuf),
    /// Straight from the running core over MoonProto.
    Core { uid: u64 },
}

impl SettingsSource {
    /// The snapshot bytes.
    ///
    /// [`SettingsSource::Core`] fails for now: MoonBot cores serve their settings snapshot over
    /// MoonProto, but the MoonProto library this terminal builds on has no call for it yet.
    pub fn read(&self) -> anyhow::Result<Vec<u8>> {
        match self {
            Self::Snapshot(path) => {
                std::fs::read(path).map_err(|e| anyhow::anyhow!("read {}: {e}", path.display()))
            }
            Self::Core { uid } => anyhow::bail!(
                "core {uid}: taking settings straight from a core waits for MoonProto library \
                 support; save a snapshot in MoonBot (VPS setup -> Get settings) and use its ZIP"
            ),
        }
    }
}
