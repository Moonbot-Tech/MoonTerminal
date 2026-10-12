//! Header collapse accessors and window decoration choices.

use super::*;

/// Ceiling for a header selector label (core, manual strategy).
///
/// Those pills size to their content and both names are arbitrary user text, so without a ceiling
/// one long name pushes the right-hand cluster — clock and window controls included — off the
/// window. Matches the other selectors' ceiling; the full name stays in the open menu.
/// This is the unscaled width passed through [`font_w`].
pub const HEADER_LABEL_MAX_W: f32 = 260.0;

/// Return whether the header ticker is drawn at collapse `level`.
///
/// ONE predicate for the header that renders it and the popup layer that must not outlive its
/// trigger. The level is chosen from measured widths (`HeaderWidths::level`), so a larger font or
/// a longer locale makes the ticker yield sooner on its own.
pub fn ticker_visible(level: HeaderLevel) -> bool {
    level.ticker()
}

/// Return whether the ticker's 1h delta is drawn at collapse `level`.
///
/// It yields one level before the ticker itself, so the price stays readable after the delta goes.
pub fn ticker_deltas_visible(level: HeaderLevel) -> bool {
    level.delta_1h()
}

/// Transparent macOS titlebars keep native traffic-light buttons over the client
/// area. Keep terminal chrome content and drag hitboxes out of that strip.
pub fn titlebar_leading_inset() -> f32 {
    if cfg!(target_os = "macos") {
        76.0
    } else {
        HEADER_PAD_X
    }
}

pub fn show_custom_window_controls() -> bool {
    !cfg!(target_os = "macos")
}

pub fn platform_window_decorations() -> Option<WindowDecorations> {
    if cfg!(target_os = "linux") {
        Some(WindowDecorations::Client)
    } else {
        None
    }
}
