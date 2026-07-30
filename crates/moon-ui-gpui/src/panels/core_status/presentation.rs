//! Shared connection and metric presentation rules for both Core Status modes.

use gpui::*;
use moon_core::feed::ConnStatus;
use moon_ui::MoonPalette;
use rust_i18n::t;

use crate::backend::core_warn::{LatencySeverity, latency_severity};

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

/// Classify a latency against the core's own rolling BASELINE, where "far above the usual" is worse.
///
/// Purely relative — a link that is always slow makes that its own baseline and stays `Normal`, while
/// a spike above the usual tints — so it never yellows a core whose high ping IS its normal (the
/// 20/60/200 ms case). Delegates to the engine's `latency_severity`, the single source of truth, so
/// the colour and the ping/exch warning always fire at the same point.
///
/// Args:
///     value: Current smoothed latency in ms.
///     baseline: The core's rolling mean latency in ms, or `None` until it is established.
///
/// Returns:
///     `Warning` from baseline ×1.10, `Critical` from ×1.30 (each with a small absolute floor), else
///     `Normal` (including an unknown value or an unestablished baseline).
fn latency_level(value: Option<u32>, baseline: Option<u32>) -> LoadLevel {
    match value {
        Some(v) => match latency_severity(v, baseline) {
            LatencySeverity::Normal => LoadLevel::Normal,
            LatencySeverity::Warning => LoadLevel::Warning,
            LatencySeverity::Critical => LoadLevel::Critical,
        },
        None => LoadLevel::Normal,
    }
}

/// Colour level for a client↔core round-trip, relative to the core's baseline. See [`latency_level`].
pub(super) fn ping_level(value: Option<u32>, baseline: Option<u32>) -> LoadLevel {
    latency_level(value, baseline)
}

/// Colour level for a core→exchange order latency, relative to the core's baseline.
pub(super) fn order_level(value: Option<u16>, baseline: Option<u16>) -> LoadLevel {
    latency_level(value.map(u32::from), baseline.map(u32::from))
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

/// Render a themed metric glyph sized to sit inline with a metric value.
///
/// Args:
///     path: Bundled MoonUI icon path (for example `icons/cpu.svg`).
///     palette: Active Moon palette supplying the soft metric color.
///
/// Returns:
///     A fixed-size SVG tinted to the metric text color.
pub(super) fn metric_icon(path: &'static str, palette: MoonPalette) -> impl IntoElement {
    svg()
        .path(path)
        .size(px(12.0))
        .flex_none()
        .text_color(rgb(palette.text_soft))
}
