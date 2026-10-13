//! Reported-build mark: a mono number and a reserved letter badge.
//!
//! The Flat cell and the layout probe both paint this. An absent letter still occupies
//! [`LETTER_SLOT`], so the digits on the next row do not slide.

use gpui::prelude::FluentBuilder;
use gpui::{App, Div, ParentElement, Styled, div, px};
use moon_ui::{
    MoonBadge, MoonBadgeSize, MoonBadgeVariant, MoonPalette, MoonSize, MoonTheme, MoonTone,
};

use crate::design;

use super::presentation::BuildParts;

/// Two glyphs, the length of a product letter such as `R3`.
///
/// An empty letter slot paints a badge of this string invisibly. [`MoonBadge`] sizes itself
/// from its label, so this is the width input — not a raw pixel width on the widget — and a
/// release keeps the same footprint as a named build.
const LETTER_SLOT: &str = "R3";

/// Gap between the number and the letter badge, in pixels.
///
/// Explicit pixels, not a rem gap: the theme font slider must not move the pair apart.
pub(super) const CELL_GAP_W: f32 = 4.0;

/// The number and the letter slot, in that order.
///
/// The number is the value, so it is `design::mono` at the body tier. The letter is a
/// `MoonBadge` at the dense-strip tier (`MoonSize::Xs`). The badge carries no raw width; an
/// absent letter still occupies [`LETTER_SLOT`] so the digits on the next row do not slide.
///
/// Args:
///     parts: Number text and optional letter from `build_parts` or `group_build_parts`.
///     p: Active Moon palette. The badge reads it directly so a behind-coloured parent cannot
///         repaint the tag.
///     app: Application context for the number's tier size and the badge's density tokens.
///     end: The Flat cell: the mark sizes to its content and the centred cell places it.
///         Without it the mark fills the free width its host gives it.
///
/// Returns:
///     A row of the number and the letter slot.
pub(super) fn version_mark(parts: BuildParts, p: MoonPalette, app: &App, end: bool) -> Div {
    let shown = parts.tag.is_some();
    let label = parts.tag.as_deref().unwrap_or(LETTER_SLOT);
    // The Flat cell sizes to its content, so a growing mark there collapses to zero and squeezes
    // the number into an ellipsis; only a fixed-width host may let the mark grow. The number
    // never truncates: a clipped build reads as a different build.
    div()
        .when(end, |row| row.flex_none().justify_end())
        .when(!end, |row| row.flex_1().min_w_0())
        .flex()
        .items_center()
        .gap(px(CELL_GAP_W))
        .child(
            div()
                .flex_none()
                .whitespace_nowrap()
                .font_family(design::mono())
                .text_size(design::t_body(app))
                .child(parts.number),
        )
        .child(
            div()
                .flex_none()
                .when(!shown, |slot| slot.invisible())
                .child(
                    MoonBadge::new(label)
                        .variant(MoonBadgeVariant::Soft)
                        .tone(MoonTone::Info)
                        .size(MoonBadgeSize::Tier(MoonSize::Xs))
                        .mono(true)
                        .render_with_theme(p, MoonTheme::active_tokens(app)),
                ),
        )
}

#[cfg(test)]
mod tests;
