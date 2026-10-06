//! Separators, dots and other small rule elements.

use super::*;

/// Vertical 1px group separator.
///
/// The height goes through `ui()`, matching MoonUI, which draws its own separators the same way
/// (the brand cluster in `MoonWindowFrame` is one). Control-tier terminal text also tracks `ui()`
/// via [`t_body`]; MoonUI-internal font-channel text still tracks `font_delta`, so the
/// rule can sit beside a MoonUI row that grew on that channel. The 1px width stays raw, also
/// matching MoonUI, since a hairline must not thicken with the font.
pub fn vline(cx: &App, height: f32, color: u32) -> impl IntoElement {
    // flex_none: a 1px rule inside a shrinking row would otherwise be the first thing squeezed
    // to nothing, silently dropping the group boundary it draws.
    div()
        .flex_none()
        .w(px(1.0))
        .h(ui_px(cx, height))
        .bg(rgb(color))
}

/// Group separator for the horizontal chrome strips — the toolbar and the window header.
///
/// One definition of the chrome-height rule, so those two strips cannot drift apart, and so the
/// height matches the separator MoonUI's own brand cluster draws.
///
/// [`CHROME_RULE_H`] is 20 of a 26px-tall chip rather than a shorter hairline: at 100% zoom a rule
/// that only spans the middle of the row reads as decoration floating inside the chips beside it,
/// not as a seam between them. Tall enough to visibly interrupt the row's own height is what makes
/// it read as a boundary.
///
/// Drawn in `border_hover` rather than `border`: against `shell_high`, `border` measures about
/// 1.2:1 in the dark palette and 1.3:1 in the light palette. These rules are the only thing marking
/// where one group ends and the next begins because spacing inside and between groups is identical.
/// `border_hover` is one step stronger in both palettes (about 1.5:1 dark and 1.7:1 light), which
/// reads as a boundary without reading as a frame.
pub fn chrome_divider(cx: &App, p: MoonPalette) -> impl IntoElement {
    vline(cx, CHROME_RULE_H, p.border_hover)
}

pub fn status_dot(color: u32, cx: &App) -> impl IntoElement {
    status_dot_sized(color, 5.0, cx)
}

/// Draw a status dot at a caller-selected logical size.
///
/// Args:
///     color: Theme-resolved RGB color.
///     size: Unscaled logical diameter.
///     cx: Application context used to apply the UI scale.
///
/// Returns:
///     A circular status marker whose diameter tracks the active UI scale.
pub fn status_dot_sized(color: u32, size: f32, cx: &App) -> impl IntoElement {
    dot(size, solid(color), cx)
}

/// Opacity a status colour carries when its source has not confirmed it on the current connection.
///
/// One value for every surface that draws such a state, so the same "second-hand" language cannot
/// mean two different things in two windows.
pub const STALE_ALPHA: f32 = 0.45;

/// Draw a status dot whose colour is FADED, for a value the source no longer confirms.
///
/// The same dot at the same size in the same colour, at reduced opacity: a reader must still see
/// WHICH state is being reported — a stale "running" is not the same fact as "unknown" — while the
/// fade says the claim is second-hand. Anything that changed the hue instead would collide with the
/// palette's own green/amber/red meanings.
///
/// Args:
///     color: Theme-resolved RGB colour of the confirmed state.
///     cx: Application context used to apply the UI scale.
///
/// Returns:
///     A circular status marker at [`STALE_ALPHA`].
pub fn status_dot_stale(color: u32, cx: &App) -> impl IntoElement {
    dot(5.0, moon_alpha(color, STALE_ALPHA), cx)
}

// ---- goal B: Auto workspace rail ----

/// Top gap paid out of the rail's fixed 30-unit cell before an exchange heading, so the section
/// separates from the row above it without a taller cell.
pub const RAIL_SECTION_GAP: f32 = 6.0;

/// Amount added to a core row's status-dot size for `Problem` and `Unavailable` — the two
/// statuses the rail summary's `problem` tally counts — so a counted core reads as alarmed by
/// size as well as by colour.
pub const RAIL_PROBLEM_DOT_STEP: f32 = 2.0;

/// Background alpha for the danger pill's tint, over the raw `p.red` hue.
///
/// `danger_color` is a TEXT token (the theme's legible red), never a fill — the pill follows the
/// tinted-background-plus-border idiom already used for amber warnings elsewhere in this crate
/// (`analytics/tuner/list/table.rs`, `analytics/calendar/day.rs`), with the strong colour carried
/// by the text instead.
pub const RAIL_PILL_BG_ALPHA: f32 = 0.16;

/// Border alpha for the danger pill's outline, over the raw `p.red` hue.
pub const RAIL_PILL_BORDER_ALPHA: f32 = 0.5;

/// Selection background alpha for a rail row, applied over the accent colour.
pub const RAIL_ROW_SELECTED_ALPHA: f32 = 0.18;

/// Hover background alpha for a rail row, applied over the accent colour.
///
/// Raised from the previous 0.10 to be visibly stronger on the light theme (owner's choice, not a
/// measured precedent: `panels/assets/table.rs:901` and `panels/core_status/by_ip_header.rs:124`
/// also use 0.14, but both are column-resize drag-handle hover strips with a mandatory
/// `.occlude()`, a different interaction from a virtualized list row).
pub const RAIL_ROW_HOVER_ALPHA: f32 = 0.14;

/// Flex-shrink factor for the summary bar's alarm segment (`проблем: N`), against its
/// `cores_ready` sibling's default `1.0`. Taffy distributes shrink proportionally to
/// `flex_basis * flex_shrink`, so this small non-zero ratio makes `cores_ready` give up
/// essentially all its own width first, while still letting the alarm segment shrink — and
/// therefore ellipsize instead of hard-clip under `overflow_hidden` — as the last resort.
pub const RAIL_ALARM_SHRINK: f32 = 0.05;

/// The one geometry both status dots draw, so a faded dot cannot drift from the solid one it
/// stands in for.
fn dot(size: f32, fill: impl Into<gpui::Fill>, cx: &App) -> impl IntoElement {
    div()
        .w(ui_px(cx, size))
        .h(ui_px(cx, size))
        .rounded(ui_px(cx, 999.0))
        .bg(fill)
}

/// Drawn width of a [`status_dot`], for a layout that must reserve its column.
///
/// One source with the dot itself: a row that leaves a gap for it and the dot that fills the gap
/// cannot drift apart when the UI scale changes.
pub fn status_dot_w(cx: &App) -> f32 {
    ui_value(cx, 5.0)
}
