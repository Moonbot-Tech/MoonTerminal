//! Shared connection and metric presentation rules for both Core Status modes.

use gpui::*;
use moon_core::feed::ConnStatus;
use moon_ui::{MoonPalette, MoonTone};
use rust_i18n::t;

/// Visual metadata shared by Flat and By IP connection rows.
pub(super) struct ConnectionPresentation {
    /// Palette color for the connection-status dot.
    pub(super) color: u32,
    /// Localized lifecycle label, including stage or failure details.
    pub(super) label: String,
    /// Badge tone paired with the lifecycle state.
    pub(super) tone: MoonTone,
}

/// Resolve one connection state into shared dot, label, and badge metadata.
///
/// Args:
///     status: Latest core connection state.
///     palette: Active Moon palette.
///
/// Returns:
///     Consistent visual metadata for either Core Status presentation.
pub(super) fn connection_presentation(
    status: &ConnStatus,
    palette: MoonPalette,
) -> ConnectionPresentation {
    match status {
        ConnStatus::Ready => ConnectionPresentation {
            color: palette.green,
            label: t!("conn.status.ready").to_string(),
            tone: MoonTone::Positive,
        },
        ConnStatus::Connecting => ConnectionPresentation {
            color: palette.amber,
            label: t!("conn.status.connecting").to_string(),
            tone: MoonTone::Warning,
        },
        ConnStatus::Stage(stage) => ConnectionPresentation {
            color: palette.amber,
            label: t!("conn.status.stage", stage = stage.as_str()).to_string(),
            tone: MoonTone::Warning,
        },
        ConnStatus::Failed(error) => ConnectionPresentation {
            color: palette.red,
            label: t!("conn.status.failed", err = error.as_str()).to_string(),
            tone: MoonTone::Danger,
        },
        ConnStatus::Disconnected => ConnectionPresentation {
            color: palette.text_soft,
            label: t!("conn.status.disconnected").to_string(),
            tone: MoonTone::Muted,
        },
    }
}

/// Build a layout-neutral rule between attention and normal row sections.
///
/// The absolute overlay stays inside an existing relative row container, so MoonTree's uniform
/// measurement and MoonDataTable's fixed row height remain unchanged.
///
/// Args:
///     palette: Active Moon palette supplying the operational warning color.
///
/// Returns:
///     A two-pixel amber line pinned to the row's top edge.
pub(super) fn normal_section_rule(palette: MoonPalette) -> Div {
    div()
        .absolute()
        .top_0()
        .left_0()
        .right_0()
        .h(px(2.0))
        .bg(rgb(palette.amber))
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

/// Format optional aggregated process memory without narrowing it.
///
/// Args:
///     value: Widened decimal-megabyte total.
///
/// Returns:
///     Localized memory text or an ASCII unavailable marker.
pub(super) fn memory_u64(value: Option<u64>) -> String {
    value
        .map(|value| format!("{} {}", value, t!("core_status.mb")))
        .unwrap_or_else(|| "-".to_string())
}

/// Format an optional small integer.
///
/// Args:
///     value: Logical CPU count.
///
/// Returns:
///     Decimal text or an ASCII unavailable marker.
pub(super) fn optional_u8(value: Option<u8>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "-".to_string())
}
