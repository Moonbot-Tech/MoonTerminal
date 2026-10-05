//! Persist retired core identities under the station's report-replica lease.

use std::path::Path;

use anyhow::Context;

/// Raise the station-owned watermark before admitting cores, never lowering it after removal.
/// The caller holds the report-replica lease, which serializes stations sharing this data root.
pub fn raise(observed: u64) -> anyhow::Result<u64> {
    raise_at(&moon_core::config::paths::station_core_uid_path(), observed)
}

/// Read and atomically replace a watermark; malformed or inaccessible state refuses admission.
fn raise_at(path: &Path, observed: u64) -> anyhow::Result<u64> {
    let saved = match std::fs::read_to_string(path) {
        Ok(text) => Some(
            text.trim()
                .parse::<u64>()
                .context("invalid station uid watermark")?,
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error).context("read station uid watermark"),
    };
    let high_water = observed.max(saved.unwrap_or(0));
    // Persist the exhausted sentinel too: recovery may replace unreadable report history,
    // so a later restart must not mistake a clean replacement for an unused uid namespace.
    if saved != Some(high_water) {
        moon_core::config::write_file_atomic(
            path,
            format!("{high_water}\n").as_bytes(),
            "station uid watermark",
        )?;
    }
    Ok(high_water)
}

#[cfg(test)]
mod tests;
