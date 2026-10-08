//! Credential selection for station installation, independent of presentation and remote I/O.

use super::CoreKeyEntry;

/// Enabled credentials to send and the names of enabled cores skipped for missing keys.
pub struct StationCoreSelection {
    pub keyed: Vec<CoreKeyEntry>,
    pub skipped: Vec<String>,
}

/// Keep enabled keyed cores without allowing a keyless row to block the rest.
/// An empty `keyed` list requires the caller to refuse before changing the server.
pub fn select_station_cores(entries: Vec<CoreKeyEntry>) -> StationCoreSelection {
    let mut selection = StationCoreSelection {
        keyed: Vec::new(),
        skipped: Vec::new(),
    };
    for entry in entries.into_iter().filter(|entry| entry.active) {
        if entry.key.is_empty() {
            selection.skipped.push(entry.name);
        } else {
            selection.keyed.push(entry);
        }
    }
    selection
}

#[cfg(test)]
mod tests;
