//! Shared connection and metric presentation rules for both Core Status modes.

use moon_core::feed::ConnStatus;
use moon_ui::MoonPalette;
use rust_i18n::t;

use super::model::ApiKeyState;
use crate::backend::core_warn::LatencySeverity;

/// Visual metadata shared by Flat and By IP connection rows.
pub(super) struct ConnectionPresentation {
    /// Localized lifecycle label, including stage or failure details.
    pub(super) label: String,
}

/// Resolve one connection state into its shared lifecycle label.
///
/// Args:
///     status: Latest core connection state.
///
/// Returns:
///     A consistent label for either Core Status presentation.
pub(super) fn connection_presentation(status: &ConnStatus) -> ConnectionPresentation {
    match status {
        ConnStatus::Ready => ConnectionPresentation {
            label: t!("conn.status.ready").to_string(),
        },
        ConnStatus::Connecting => ConnectionPresentation {
            label: t!("conn.status.connecting").to_string(),
        },
        ConnStatus::Stage(stage) => ConnectionPresentation {
            label: t!("conn.status.stage", stage = stage.as_str()).to_string(),
        },
        ConnStatus::Failed(error) => ConnectionPresentation {
            label: t!("conn.status.failed", err = error.as_str()).to_string(),
        },
        ConnStatus::Disconnected => ConnectionPresentation {
            label: t!("conn.status.disconnected").to_string(),
        },
    }
}

/// Format an optional integer percentage.
///
/// Args:
///     value: Integer percentage from MoonProto.
///
/// Returns:
///     Percentage text or an ASCII unavailable marker.
pub(super) fn percent(value: Option<u8>) -> String {
    value
        .map(|value| format!("{value}%"))
        .unwrap_or_else(|| "-".to_string())
}

/// Format optional per-process or machine memory.
///
/// Args:
///     value: Decimal megabytes from MoonProto.
///
/// Returns:
///     Localized memory text or an ASCII unavailable marker.
pub(super) fn memory_u16(value: Option<u16>) -> String {
    value
        .map(|value| format!("{} {}", value, t!("core_status.mb")))
        .unwrap_or_else(|| "-".to_string())
}

/// Format an optional client↔core round-trip time in milliseconds, e.g. `142 ms`.
///
/// Args:
///     value: Round-trip time from `Event::KernelHealth`.
///
/// Returns:
///     Localized latency text or an ASCII unavailable marker.
pub(super) fn ping(value: Option<u32>) -> String {
    value
        .map(|value| format!("{} {}", value, t!("core_status.ms")))
        .unwrap_or_else(|| "-".to_string())
}

/// Format a latency in milliseconds WITHOUT the unit, e.g. `142`, for the By IP tree where the
/// column header carries the unit. `-` when unavailable.
///
/// Args:
///     value: Round-trip time from `Event::KernelHealth`.
///
/// Returns:
///     Bare millisecond number, or an ASCII unavailable marker.
pub(super) fn ping_plain(value: Option<u32>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "-".to_string())
}

/// Format one core's API-key state for its column.
///
/// A bare number: the unit lives in the column heading, so a column of counts reads as a column
/// instead of repeating "дн" on every row. The infinity glyph lives here, not in the dictionaries,
/// because `locales/README.md` keeps glyphs out of translated values.
///
/// Args:
///     state: The key's state as classified for this frame.
///
/// Returns:
///     Column text for the key's remaining lifetime.
pub(super) fn api_expiry_text(state: ApiKeyState) -> String {
    match state {
        ApiKeyState::Unknown => "-".to_string(),
        ApiKeyState::Perpetual => "\u{221e}".to_string(),
        ApiKeyState::Days(days) if days < 0 => t!("core_status.api_expired").to_string(),
        ApiKeyState::Days(days) => days.to_string(),
    }
}

/// Colour for an API-key cell.
///
/// The WARNING decision is the engine's — it owns the user's day threshold — so this reads that
/// decision instead of re-deriving one from its own day literals. A second set of steps here would
/// let the number stay grey under a lit warning triangle the moment the threshold is not the
/// default, the exact disagreement `lat_level` exists to prevent for the ping axes.
///
/// Args:
///     state: The key's state as classified for this frame.
///     warn: Whether the engine currently warns about this key.
///
/// Returns:
///     Red once the key is past its date, yellow while the engine warns, else no colour.
pub(super) fn api_expiry_level(state: ApiKeyState, warn: bool) -> LoadLevel {
    if state.is_expired() {
        LoadLevel::Critical
    } else if warn {
        LoadLevel::Warning
    } else {
        LoadLevel::Normal
    }
}

/// Map the engine's latency severity (already computed against the core's baseline and the axis
/// thresholds) to the shared load level for colouring. The engine is the single source of truth, so
/// the row colour and the ping/exch warning always agree — a core whose high ping IS its normal (the
/// 20/60/200 ms case) stays `Normal`.
pub(super) fn lat_level(sev: LatencySeverity) -> LoadLevel {
    match sev {
        LatencySeverity::Normal => LoadLevel::Normal,
        LatencySeverity::Warning => LoadLevel::Warning,
        LatencySeverity::Critical => LoadLevel::Critical,
    }
}

/// Format machine CPU load with the machine's logical-core count, e.g. `34% (16 core)`.
///
/// Args:
///     system_cpu: Whole-machine CPU percentage.
///     cores: Logical CPU count of the machine, appended in parentheses when known.
///
/// Returns:
///     Localized CPU line; the core count is dropped when it has not arrived yet.
pub(super) fn cpu_load(system_cpu: Option<u8>, cores: Option<u8>) -> String {
    match cores {
        Some(cores) => t!(
            "core_status.cpu_load",
            value = percent(system_cpu),
            n = cores
        )
        .to_string(),
        None => percent(system_cpu),
    }
}

/// Format free-memory percent and the machine's TOTAL RAM in gigabytes, e.g. `12% free (2 GB)`.
///
/// MoonProto never reports total RAM, so it is reconstructed as `process RAM sum + free physical`
/// and rounded UP to the next whole gigabyte, because real RAM is always a whole number of
/// gigabytes. The parenthetical is that reconstructed total; the percentage is free of it.
///
/// Args:
///     process_mem_mb: Sum of per-process resident memory on the machine, decimal MB.
///     free_mb: Free physical memory on the machine, decimal MB.
///
/// Returns:
///     Localized "free% (total GB)" line, or an ASCII marker until free memory has arrived.
pub(super) fn memory_free(process_mem_mb: Option<u64>, free_mb: Option<u16>) -> String {
    let Some(free_mb) = free_mb else {
        return "-".to_string();
    };
    let free_mb = u64::from(free_mb);
    let total_mb = process_mem_mb.unwrap_or(0) + free_mb;
    let total_gb = total_mb.div_ceil(1024).max(1);
    let pct = (free_mb as f64 / (total_gb as f64 * 1024.0) * 100.0).round() as u64;
    t!("core_status.mem_free", pct = pct, gb = total_gb).to_string()
}

/// Operational load state of a core along one axis (CPU load or free memory).
///
/// Ordered `Normal < Warning < Critical`, so severities combine with `max`. Exposed for reuse:
/// today it colors the metric numbers and drives the warnings sort; later it can drive core badges
/// and a tab indicator without recomputing the thresholds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum LoadLevel {
    /// Comfortable headroom.
    Normal,
    /// Getting tight.
    Warning,
    /// Near exhaustion.
    Critical,
}

/// Classify CPU load, where a higher percentage is worse.
///
/// Args:
///     percent: CPU percentage, process or whole-machine.
///
/// Returns:
///     `Warning` from 70%, `Critical` from 90%, else `Normal` (including unknown).
pub(super) fn cpu_level(percent: Option<u8>) -> LoadLevel {
    match percent {
        Some(percent) if percent >= 90 => LoadLevel::Critical,
        Some(percent) if percent >= 70 => LoadLevel::Warning,
        _ => LoadLevel::Normal,
    }
}

/// Classify free memory, where a lower free share is worse.
///
/// Uses the same reconstructed total as [`memory_free`] (process RAM sum + free physical).
///
/// Args:
///     process_mem_mb: Sum of per-process resident memory on the machine, decimal MB.
///     free_mb: Free physical memory on the machine, decimal MB.
///
/// Returns:
///     `Warning` below 10% free, `Critical` below 5% free, else `Normal` (including unknown).
pub(super) fn free_mem_level(process_mem_mb: Option<u64>, free_mb: Option<u16>) -> LoadLevel {
    let Some(free_mb) = free_mb else {
        return LoadLevel::Normal;
    };
    let free_mb = u64::from(free_mb);
    let total_mb = process_mem_mb.unwrap_or(0) + free_mb;
    if total_mb == 0 {
        return LoadLevel::Normal;
    }
    let free_pct = free_mb * 100 / total_mb;
    if free_pct < 5 {
        LoadLevel::Critical
    } else if free_pct < 10 {
        LoadLevel::Warning
    } else {
        LoadLevel::Normal
    }
}

/// Text color for a load level: soft text when normal, then yellow, then red.
///
/// Args:
///     level: Classified load state.
///     palette: Active Moon palette.
///
/// Returns:
///     A palette color the metric number is drawn in.
pub(super) fn level_color(level: LoadLevel, palette: MoonPalette) -> u32 {
    match level {
        LoadLevel::Normal => palette.text_soft,
        LoadLevel::Warning => palette.yellow,
        LoadLevel::Critical => palette.red,
    }
}

#[cfg(test)]
mod tests;
