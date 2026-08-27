//! Pure geometry for the Assets wallets roster column: its default/min/max BASE width and the
//! resize arithmetic. GPUI-free so both functions are unit-testable without a GPUI context —
//! tested from the panel's existing `panels/assets/tests.rs`, the house pattern also used by
//! `balances.rs::footer_facts`, rather than a local `#[cfg(test)] mod tests` here.
//!
//! Persisted unit is the BASE (unscaled) `f32`, never rendered pixels — a rendered value would
//! re-break at the next Font-slider move, which is the defect this module exists to fix. Render
//! through [`crate::design::font_w_px`].

use std::collections::HashMap;

/// Key inside `layout.table_column_widths`'s roster bag. NEVER rename: it is a persisted map key,
/// and a rename orphans every width a user has already dragged (mirrors `ByIpCol::key`'s
/// warning in `core_status/by_ip_widths.rs`).
pub(super) const WIDTH_KEY: &str = "roster";

/// Default BASE width. Renders within 0.02 px of the old fixed `420.0` at the shipped default
/// Font-slider delta (`font_width_scale = 13/11`, exactly): `355.4 * 13/11 = 420.02`. Widens
/// proportionally once the slider is raised, which is the fix; substituting 420 here as the base
/// would silently widen the column for every user at the shipped default.
pub(super) const DEFAULT_BASE_W: f32 = 355.4;

/// Narrowest a user may drag the roster, in base units. Same conversion as the default:
/// `203.1 * 13/11 = 240.02`, preserving the existing sound floor restated in base units, within
/// 0.02 px of the old fixed `240.0`.
pub(super) const MIN_BASE_W: f32 = 203.1;

/// Widest a user may drag the roster, in base units. Renders ~851 px at the shipped default; on a
/// ~1280 px window that still leaves ~430 px, ~140 px per wallet column — the point past which a
/// ticker plus an amount stops being readable. A cap is needed at all because nothing else stops
/// a drag from starving the `flex_1` wallet columns (same argument as
/// `core_status::by_ip_widths::MAX_COL_W`).
pub(super) const MAX_BASE_W: f32 = 720.0;

/// Resolve the roster's BASE width: the default, overridden by a finite stored value clamped to
/// `[MIN_BASE_W, MAX_BASE_W]`.
///
/// A non-finite stored value is SKIPPED rather than clamped — `f32::clamp` panics on NaN, and
/// `layout.toml` is hand-editable untrusted input rather than something only the drag handler
/// ever writes (precedent: `core_status::by_ip_widths::ByIpWidths::resolved`).
///
/// Args:
///     user: Stored per-column widths, keyed by [`WIDTH_KEY`].
///
/// Returns:
///     The default width, or the clamped stored override.
pub(super) fn resolved(user: &HashMap<String, f32>) -> f32 {
    match user.get(WIDTH_KEY) {
        Some(&value) if value.is_finite() => value.clamp(MIN_BASE_W, MAX_BASE_W),
        _ => DEFAULT_BASE_W,
    }
}

/// Compute the roster's next BASE width from a live divider drag.
///
/// `anchor_base` and `anchor_x` are captured ONCE at drag start, past GPUI's drag threshold —
/// mandatory here because the grabbed edge is the column's own right edge, so a per-frame
/// `pointer_x - origin_x` would compound. The POINTER delta is divided by `scale` because the
/// persisted unit is the base width while the pointer moves in rendered pixels
/// (`rendered = base * scale`, so `d_base = d_rendered / scale`) — dividing by the scale is what
/// keeps the edge tracking the cursor 1:1 on screen.
///
/// Args:
///     anchor_base: The roster's base width at the grab.
///     anchor_x: Pointer x at the grab, in window pixels.
///     pointer_x: Current pointer x, in window pixels.
///     scale: Current font-width scale ([`crate::design::font_scale`]).
///
/// Returns:
///     `None` when `pointer_x` is non-finite or `scale` is non-finite or `<= 0.0`; otherwise the
///     clamped next base width.
pub(super) fn dragged(anchor_base: f32, anchor_x: f32, pointer_x: f32, scale: f32) -> Option<f32> {
    if !pointer_x.is_finite() || !scale.is_finite() || scale <= 0.0 {
        return None;
    }
    Some((anchor_base + (pointer_x - anchor_x) / scale).clamp(MIN_BASE_W, MAX_BASE_W))
}
