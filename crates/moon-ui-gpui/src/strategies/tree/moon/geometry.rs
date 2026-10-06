//! MoonTree geometry helpers.

use super::*;

// ── Node ID encoding: stable string IDs for MoonTree ─────────────────────────
/// Build the stable tree ID for an exchange identity or the single unidentified section.
///
/// Args:
///     venue: Nameable venue behind a section, or `None` for every unidentified core.
///
/// Returns:
///     Identity-derived tree ID that is independent of localized or wire-reported captions.
pub(super) fn id_exchange(venue: Option<&CoreVenue>) -> SharedString {
    match venue {
        Some(venue) => SharedString::from(format!("x:{}:{}", venue.id.code, venue.id.dex)),
        None => SharedString::from("x:unknown"),
    }
}

pub(super) fn id_core(core: CoreId) -> SharedString {
    SharedString::from(format!("c:{core}"))
}
pub(super) fn id_folder(core: CoreId, path: &str) -> SharedString {
    SharedString::from(format!("f:{core}:{path}"))
}
pub(crate) fn id_strat(core: CoreId, id: u64) -> SharedString {
    SharedString::from(format!("s:{core}:{id}"))
}
pub(super) fn id_del_folder(core: CoreId) -> SharedString {
    SharedString::from(format!("d:{core}"))
}
pub(crate) fn id_del_strat(core: CoreId, id: u64) -> SharedString {
    SharedString::from(format!("ds:{core}:{id}"))
}

/// Unscaled row height, at the tree's local text step of zero.
///
/// The single home for this number: every row kind must derive its height from [`row_h`], never
/// from this literal directly, because `MoonTree` renders through GPUI's `uniform_list`, which
/// measures ONE row (index 0) and applies that height to every row. Any divergence between row
/// kinds is silently resolved in favour of whichever kind is first, so the failure is intermittent
/// and reads as a rendering glitch rather than a compile-time mistake.
pub(super) const ROW_H_BASE: f32 = 23.0;
/// Unscaled row text line-height, at the tree's local text step of zero. Paired with
/// [`ROW_H_BASE`] for the same reason: both feed [`row_h`] and must move together.
pub(super) const ROW_LINE_BASE: f32 = 14.0;
/// Unscaled row vertical padding, at the tree's local text step of zero. Unlike the other two, this
/// does not grow with `step` — see [`row_h`].
pub(super) const ROW_PAD_BASE: f32 = 4.5;

/// Historical custom badge metrics used only when the local tree-text step is positive.
/// The zero-step branch uses the shared Xs tier. The Custom box stays on the legacy-tuned
/// constants — including font_size — because migrating the font alone would put text on the
/// control tier inside a box that never moved (this task's exception for that split).
pub(super) const BADGE_TINY_H: f32 = 13.0;
pub(super) const BADGE_TINY_RADIUS: f32 = 4.0;
pub(super) const BADGE_TINY_FONT: f32 = 8.5;
pub(super) const BADGE_TINY_LINE: f32 = 11.0;
pub(super) const BADGE_TINY_PAD_X: f32 = 4.0;
pub(super) const BADGE_TINY_MIN_W: f32 = 16.0;

/// Gap a heading row leaves between its disclosure marker and the control after it, in design
/// units. Read by [`disclosure_run`] as well, so the markerless rows reserve exactly what the
/// heading spends.
pub(super) const HEADING_GAP: f32 = 5.0;

/// Width a heading row reserves for its `active/total` counter, in design units.
///
/// A MINIMUM rather than a fixed box: at the shipped text step this fits seven mono glyphs, which
/// covers every count a real account produces, so the column lines up across every row at every
/// tree depth. A count wide enough to exceed it grows its own slot and truncates nothing — a
/// counter that lies is worse than a column that bulges on one row.
pub(super) const COUNTS_SLOT_W: f32 = 50.0;
/// Width a heading row reserves for the open-orders `(N)`, in design units.
///
/// ALWAYS reserved, including on a row that currently has no open orders, so a core gaining or
/// losing them never shifts the `active/total` column left of it.
pub(super) const ORDERS_SLOT_W: f32 = 34.0;
/// Gap between the two counter slots, in design units. Tighter than [`HEADING_GAP`] so the two
/// numbers read as one cluster rather than as two unrelated columns.
pub(super) const COUNTS_GAP: f32 = 4.0;

/// Row height at the tree's local text step, in `Pixels`.
///
/// The only caller of [`design::fit_h_px`] for a tree row — see [`ROW_H_BASE`] for why every row
/// kind must share this one function rather than compute its own height.
///
/// Args:
///     app: Application context used to read active theme tokens.
///     step: Local unscaled text-size step, `0.0` for no local adjustment.
///
/// Returns:
///     The scaled row height that fits the row's text line box at `step`.
pub(super) fn row_h(app: &App, step: f32) -> Pixels {
    design::fit_h_px(app, ROW_H_BASE + step, ROW_LINE_BASE + step, ROW_PAD_BASE)
}

/// Return Xs for an unadjusted tree badge, or the existing custom dimensions plus a local step.
///
/// The positive-step custom branch deliberately retains its historical metrics and text scaling.
/// Args:
///     step: Local unscaled text adjustment; nonpositive values select the shared Xs tier.
/// Returns:
///     An explicit Xs tier or the existing locally enlarged custom badge.
pub(super) fn row_badge_size(step: f32) -> MoonBadgeSize {
    if step <= 0.0 {
        return MoonBadgeSize::Tier(moon_ui::MoonSize::Xs);
    }
    MoonBadgeSize::Custom {
        height: BADGE_TINY_H + step,
        radius: BADGE_TINY_RADIUS,
        font_size: BADGE_TINY_FONT + step,
        line_height: BADGE_TINY_LINE + step,
        pad_x: BADGE_TINY_PAD_X,
        min_width: BADGE_TINY_MIN_W,
    }
}

/// Leading run a heading row spends on its disclosure marker and the gap after it.
///
/// Rows that carry no marker — strategies, and the deleted rows beside them — reserve the same run
/// so their controls land one indent step RIGHT of their own folder's, instead of five design units
/// LEFT of it. Derived from the two values `core_folder_row` actually renders with, so the columns
/// cannot drift apart when either is nudged.
///
/// Args:
///     app: Application context used to scale the design units.
///     step: Local unscaled text-size step, which the marker rides like the row's text.
///
/// Returns:
///     Scaled width of the marker box plus the heading row's control gap.
pub(super) fn disclosure_run(app: &App, step: f32) -> Pixels {
    design::ui_px(app, design::DISCLOSURE_BOX + step) + design::ui_px(app, HEADING_GAP)
}

/// Pane inset before depth zero, in design units. Shared with [`tree_row_indent`].
pub(super) const TREE_INDENT_INSET: f32 = 6.0;
/// One tree level, in design units. Kept equal to the historical `12.0 * depth` step.
pub(super) const TREE_INDENT_STEP: f32 = 12.0;

/// Leading inset of a tree row at `depth`, in design units.
///
/// Args:
///     depth: Nesting level from the tree entry. Zero is the first row in the pane.
///
/// Returns:
///     Unscaled x of the row content. [`design::ui_px`] applies UI zoom later, once.
pub(super) fn tree_row_indent(depth: f32) -> f32 {
    TREE_INDENT_INSET + TREE_INDENT_STEP * depth
}

/// Unscaled x of a heading caret, in design units from the pane's left edge.
///
/// A folder used to subtract `DISCLOSURE_BOX + step` from this whenever it also
/// drew an icon. That box is the same 12 units as [`TREE_INDENT_STEP`], so the
/// folder caret landed on its core's caret, and a larger text step moved it
/// further left. The icon no longer shares the caret slot, so neither input
/// moves the box: a child one level deeper stays exactly one step to the right
/// at every text size. UI zoom is a later [`design::ui_px`] of this value.
///
/// Args:
///     depth: Nesting level of the heading row.
///     has_icon: Whether the row draws a folder mark. Must not move the caret.
///     step: Local unscaled text-size step. Must not move the caret.
///
/// Returns:
///     Left edge of the caret box.
pub(super) fn disclosure_caret_x(depth: f32, has_icon: bool, step: f32) -> f32 {
    // Both inputs stay in the signature so the row cannot stop passing them.
    // Neither is allowed to become a shift again.
    let _ = (has_icon, step);
    tree_row_indent(depth)
}

/// Design-unit shift of the caret glyph from the left of its own depth slot.
///
/// The slot is already padded by [`tree_row_indent`], so this is the glyph's
/// `left`. It is whatever [`disclosure_caret_x`] adds on top of that indent.
///
/// Args:
///     has_icon: Whether the row draws a folder mark.
///     step: Local unscaled text-size step.
///
/// Returns:
///     `0.0` while the caret stays on its own level.
pub(super) fn disclosure_caret_shift(has_icon: bool, step: f32) -> f32 {
    disclosure_caret_x(0.0, has_icon, step) - tree_row_indent(0.0)
}
