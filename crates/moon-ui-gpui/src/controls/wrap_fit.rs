//! Shrink-before-wrap for a wrapping chrome row.
//!
//! A `flex_wrap` row gives a narrowing host exactly one answer: move a section to a second line.
//! The controls themselves never yield first, so a row loses a whole line while its selectors are
//! still carrying words the icon beside them already says. The toolbar solves the same problem by
//! BUDGET (`controls::toolbar::row_fit`): it knows every width on the row and sheds labels by an
//! explicit ladder. A panel row cannot — it is composed of localized dropdowns, inputs and pickers
//! whose widths differ per host, per locale and per font step, and it lives in a dock whose width
//! is not the window's.
//!
//! So this resolves the same question by MEASUREMENT instead, which is also the only honest way to
//! ask it here: the row reports what it actually got, and a pure decision function answers whether
//! its shrinkable controls should render compact. Two facts come back from one painted frame —
//! the row's own size, and the height of one section, which is what a single LINE of that row
//! measures. A row taller than one line has wrapped.
//!
//! The loop that this could become is closed by construction, and that is the whole design:
//! compacting shrinks the row, which can un-wrap it, which would re-expand it, which wraps it
//! again. [`WrapFit`] therefore remembers the width at which the FULL row overflowed and refuses
//! to expand again until the row is wider than that by at least what compacting saves. Every retry
//! raises that threshold, so the sequence terminates instead of oscillating; see the sibling tests.
//!
//! [`compact_trigger_width`] and [`signature`] live here rather than in the panel because they are
//! the parts a SECOND host needs verbatim: the compact geometry every selector on a row shares, and
//! the theme half of the composition digest.

use std::cell::Cell;
use std::hash::{Hash, Hasher};
use std::rc::Rc;

use gpui::{App, Div, Entity, ParentElement, Render, Styled, canvas};
use moon_ui::{MoonButtonSize, MoonDropdown};

use crate::design;

#[cfg(test)]
mod tests;

/// Row height, in multiples of one section's height, above which the row has WRAPPED.
///
/// One line measures the TALLEST section plus the row's own vertical padding; a second line adds a
/// whole section plus the wrap gap, so two lines always exceed twice the measured section while one
/// line exceeds it only by however much its tallest member and its padding outgrow the section the
/// height is read from. The ratio therefore sits just under two: a sibling section would have to
/// stand nearly twice the height of a dropdown before one line read as two, and the padding — a
/// fixed gpui rem that does not follow the theme's scales — cannot close that gap even at the
/// smallest font step.
const WRAP_RATIO: f32 = 1.9;

/// Smallest extra width a re-expansion waits for, when compacting saves almost nothing.
///
/// Design-reference pixels are deliberately NOT used: this is compared against measured widths,
/// and it exists only so a caller that reports a near-zero saving still gets a threshold that a
/// one-pixel resize cannot cross.
const MIN_REEXPAND_MARGIN: f32 = 24.0;

/// Design-reference width bounds shared by EVERY compact trigger on a row.
///
/// The floor is what the short "all" word plus the glyph, the caret and the button's own insets
/// occupy, so a count never renders narrower than the word it replaced — a selector holds one width
/// while the selection changes under it, exactly as its full form does. The ceiling leaves room for
/// a longer localization of that word and ellipsizes anything longer (a pinned core name) instead
/// of letting it reclaim the row.
///
/// One pair for every selector, because they stand SIDE BY SIDE: two compact triggers that resolved
/// their own bounds ended up a few pixels apart for the same shape, which reads as a mistake rather
/// than as a design. MoonUI scales both by the trigger's own font step.
pub(crate) const COMPACT_MIN_W: f32 = 72.0;
/// Ceiling of a compact trigger; see [`COMPACT_MIN_W`].
pub(crate) const COMPACT_MAX_W: f32 = 104.0;

/// A fitting bound high enough not to clip anything — the label's NATURAL width is wanted here, and
/// [`compact_trigger_width`] applies the shared bounds itself, after the glyph is accounted for.
const UNBOUNDED_FIT_W: f32 = 4000.0;

/// Whether a row's shrinkable controls render compact, and what it takes to undo that.
///
/// Held by the panel that owns the row and updated only from [`Self::resolve`], so the value the
/// frame was rendered with is always the value the decision was made against. The compact state is
/// the remembered width itself: a row is compact exactly while it owes a re-expansion.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct WrapFit {
    /// Measured row width at which the FULL row last overflowed onto a second line.
    ///
    /// `None` while the row is full: nothing has overflowed, so nothing is owed a re-expansion.
    overflow_w: Option<f32>,
    /// Row composition the remembered width was resolved for; see [`RowFit::signature`].
    signature: u64,
}

/// The two facts about the row that only its caller can supply.
#[derive(Clone, Copy, Debug)]
pub(crate) struct RowFit {
    /// Width the row gives up by rendering compact, as the caller measures it.
    ///
    /// It is the margin a re-expansion waits for, so over-stating it costs a few pixels of delay
    /// while under-stating it lets the row expand into a width it then overflows again.
    pub(crate) saving: f32,
    /// Fingerprint of what the caller knows changes the row's natural width without changing its
    /// own measured size — which sections it composes, the locale, the font step, the width of a
    /// selector whose trigger fits itself to a name. Build it with [`signature`].
    ///
    /// It cannot be complete, and does not claim to be: a label that grows by a few pixels goes
    /// unnoticed until the next resize. What it must cover is every input that moves the row by
    /// MORE than `saving`, since that is the width the fit is held against.
    pub(crate) signature: u64,
}

/// One painted frame's measurement of the row, joined with what the caller knows.
#[derive(Clone, Copy)]
struct RowMetrics {
    /// Measured width of the whole row.
    row_w: f32,
    /// Measured height of the whole row, including its own vertical padding.
    row_h: f32,
    /// Measured height of one section on that row — the height of a single LINE.
    section_h: f32,
    /// The caller's half of the decision.
    fit: RowFit,
}

impl WrapFit {
    /// Whether the row's shrinkable controls should render compact this frame.
    pub(crate) fn compact(self) -> bool {
        self.overflow_w.is_some()
    }

    /// Resolve the next fit from one frame's measurement.
    ///
    /// Args:
    ///     m: The row's measurement for the frame that was rendered with `self`.
    ///
    /// Returns:
    ///     The changed fit, or `None` when this frame changes nothing — which is every frame of a
    ///     steady row, so the caller repaints exactly zero times for it.
    fn resolve(self, m: RowMetrics) -> Option<Self> {
        // A changed composition retires the remembered width: it described a row that no longer
        // exists. KEEPING the compact state and re-anchoring it was the tempting alternative — it
        // avoids a one-frame flash while the Settings font slider is dragged — but it latches a row
        // compact for good when the change made the row NARROWER (a smaller font, a shorter
        // locale): the row would then fit full at its present width with nothing left to ask again.
        //
        // The frame that reported the change is NOT discarded with it. This frame was drawn with
        // the old fit, so its measurement is exactly the evidence the new one needs, and the very
        // first frame of every panel takes this path — signature 0 against a real digest.
        let base = if self.signature == m.fit.signature {
            self
        } else {
            Self {
                overflow_w: None,
                signature: m.fit.signature,
            }
        };
        // A row that has not been laid out yet reports zeroes, and a zero section height would read
        // as "wrapped" against any row height at all. Non-finite pixels are refused for a sharper
        // reason: an infinite width would satisfy every re-expansion test and a NaN would satisfy
        // none, and either one, once stored, decides every frame that follows it.
        if !(m.row_w.is_finite() && m.row_h.is_finite() && m.section_h.is_finite())
            || m.row_w <= 0.0
            || m.section_h <= 0.0
        {
            return (base != self).then_some(base);
        }
        let wrapped = m.row_h > m.section_h * WRAP_RATIO;
        let Some(overflow_w) = base.overflow_w else {
            // A full row yields its words the moment it takes a second line, and remembers the
            // width it did so at.
            let next = Self {
                overflow_w: wrapped.then_some(m.row_w),
                signature: m.fit.signature,
            };
            return (next != self).then_some(next);
        };
        // Tested BEFORE the width: a row that wraps even in its compact form is already giving
        // everything it has, and the second line is the right answer. Re-expanding it because the
        // host grew past the remembered width would put the full row back onto two lines and bounce
        // straight back — one flash per step of a widening drag.
        if wrapped {
            return (base != self).then_some(base);
        }
        // `f32::max` answers with the finite operand for a NaN saving, so a caller that measured
        // nothing still gets the fixed margin rather than a threshold no width can cross.
        let expand_at = overflow_w + m.fit.saving.max(MIN_REEXPAND_MARGIN);
        let next = Self {
            overflow_w: (m.row_w < expand_at).then_some(overflow_w),
            signature: m.fit.signature,
        };
        (next != self).then_some(next)
    }
}

/// Digest the theme, the locale and the caller's own composition bits into a row signature.
///
/// The theme half is the same for every host — a font step or a locale changes what every label on
/// every row measures — so a host supplies only what is particular to its own row.
///
/// Args:
///     cx: Application context supplying the resolved typography, UI scale and locale.
///     composition: Whatever the host knows about its own row: which optional sections it drew, the
///         rendered width of a section that sizes itself to its content.
///
/// Returns:
///     The digest to put in [`RowFit::signature`].
pub(crate) fn signature(cx: &App, composition: impl Hash) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    composition.hash(&mut h);
    // The RESOLVED font, not the requested size: a fallback or an availability change moves every
    // measured width without moving the slider.
    design::text_metrics_key(cx, design::ACTION_LABEL_BASE, 400.0, true).hash(&mut h);
    // Sampled at 100 so a UI-scale step survives the rounding.
    design::ui_value(cx, 100.0).to_bits().hash(&mut h);
    rust_i18n::locale().hash(&mut h);
    h.finish()
}

/// Rendered width of a compact trigger: a leading glyph, a short label, and its caret.
///
/// Reproduces exactly what `MoonDropdown` resolves for a trigger carrying a leading icon — natural
/// text width plus the button's insets, the glyph and its gap, clamped to the shared bounds — so a
/// selector that draws its own trigger content lands on the same width as one that lets the
/// component fit it. That equality is the point: the two sit beside each other on the row.
///
/// Never narrower than the "all" word: a selector holds ONE width while the selection changes under
/// it, so a row that fitted with "All" cannot start wrapping because a count replaced it.
///
/// Args:
///     cx: Application context supplying the active font and UI scales.
///     label: The compact text actually rendered.
///     all_word: The short "all" word, which is this trigger's floor.
///
/// Returns:
///     The trigger width in logical pixels.
pub(crate) fn compact_trigger_width(cx: &App, label: &str, all_word: &str) -> f32 {
    // Unbounded on purpose: the component's own clamp is applied below, AFTER the glyph, which is
    // where MoonUI applies it too — clamping the text first and adding the glyph after would land
    // one reservation away from what the component draws.
    let natural = |text: &str| {
        MoonDropdown::fitted_trigger_label(cx, text, MoonButtonSize::Action, 0.0, UNBOUNDED_FIT_W).1
    };
    let text_w = if label == all_word {
        natural(label)
    } else {
        natural(label).max(natural(all_word))
    };
    let content = text_w + design::action_icon_reservation(cx);
    content.clamp(compact_floor_width(cx), action_width(cx, COMPACT_MAX_W))
}

/// Rendered width a compact trigger never goes below — [`COMPACT_MIN_W`] at the active font step.
///
/// Also what a row budgets its compaction saving with: the floor UNDER-states how narrow a trigger
/// gets, which over-states the saving, and over-stating is the safe direction for the margin a
/// re-expansion waits on — see [`RowFit::saving`].
pub(crate) fn compact_floor_width(cx: &App) -> f32 {
    action_width(cx, COMPACT_MIN_W)
}

/// The same floor as [`compact_trigger_width`] resolves, in the DESIGN units a component takes.
///
/// A selector that lets `MoonDropdown` fit its own trigger cannot be handed a rendered width — a
/// pinned core name has to stay ellipsizable — so it is handed this as the fit's lower bound
/// instead. Without it the component would floor at the bare [`COMPACT_MIN_W`] and a locale whose
/// "all" word is wider than that (Spanish "Todos") would leave the two selectors a few pixels
/// apart, and the fitted one would change width as its selection moved between the word and a
/// count. Both are exactly what the shared floor exists to prevent.
///
/// Args:
///     cx: Application context supplying the active font and UI scales.
///     all_word: The short "all" word this row's selectors are floored on.
///
/// Returns:
///     The lower bound to pass to `MoonDropdown::fit_trigger_width`.
pub(crate) fn compact_design_floor(cx: &App, all_word: &str) -> f32 {
    compact_trigger_width(cx, all_word, all_word) / action_scale(cx)
}

/// The rendered width a design-reference width becomes on an Action-sized trigger.
///
/// MIRRORS MoonUI: `fit_dropdown_trigger_label` scales its bounds by `font(font_size)/font_size` at
/// the trigger's own size, which for Action is [`design::ACTION_LABEL_BASE`] — NOT the mono body
/// scale `design::font_w` applies, from which it diverges as soon as the Font slider leaves zero.
/// A row whose selectors mixed the two would size them differently for the same shape.
pub(crate) fn action_width(cx: &App, design_w: f32) -> f32 {
    design_w * action_scale(cx)
}

/// The factor MoonUI applies to an Action-sized trigger's design-reference widths.
fn action_scale(cx: &App) -> f32 {
    design::font_value(cx, design::ACTION_LABEL_BASE) / design::ACTION_LABEL_BASE
}

/// The line height one frame measured, shared between the two probes below.
///
/// Created per render and read inside the same paint pass, so it carries no state between frames:
/// the section probe writes it and the row probe, painted after it, reads it back.
pub(crate) type LineHeight = Rc<Cell<f32>>;

/// Report one section's height, which is the height of a single line of the row.
///
/// The probe is an absolutely positioned overlay inside the section, so it measures the section
/// without taking part in its layout and without a hitbox of its own — `Canvas` reports no element
/// id, so nothing beneath it loses a click or a tooltip.
///
/// Args:
///     reference: A section that is ALWAYS on the row and stands at its ordinary control height.
///         An optional section would report nothing on the frames it is absent and stall the
///         decision; an unusually tall one would make every single line read as two.
///     line: Channel the row probe reads back in the same paint pass.
///
/// Returns:
///     The section with the probe layered over it.
pub(crate) fn measured_section(reference: Div, line: &LineHeight) -> Div {
    let line = line.clone();
    reference.child(
        canvas(
            move |bounds, _, _| bounds,
            move |bounds, _, _window, _cx| line.set(f32::from(bounds.size.height)),
        )
        .absolute()
        .inset_0(),
    )
}

/// Measure the row, resolve its fit, and ask for one repaint when what is on screen is now wrong.
///
/// The probe is the row's LAST child so it paints after every section, including the one carrying
/// [`measured_section`] — the two facts the decision needs therefore come from the same frame. It
/// is absolutely positioned and takes no part in the wrapping it observes.
///
/// The repaint is deferred rather than raised here for the reason `chart_tabs::stack`'s size probe
/// states: a notify inside a draw phase is dropped by the fork, leaving the window clean and no
/// frame scheduled. It is also raised only where the frame on screen no longer matches the fit — a
/// fit that merely adopted a new signature is stored and picked up by the next render, which is
/// what keeps a dragged font slider from costing two panel renders per step.
///
/// Args:
///     row: The wrapping row, already carrying its sections.
///     line: Channel written by [`measured_section`] earlier in the same paint pass.
///     entity: The panel owning the fit, repainted when it changes.
///     now: The fit this frame was rendered with.
///     fit: The caller's half of the decision — see [`RowFit`].
///     apply: Writes the resolved fit back into the panel.
///
/// Returns:
///     The row with the probe layered over it.
pub(crate) fn measured_row<S: Render + 'static>(
    row: Div,
    line: &LineHeight,
    entity: Entity<S>,
    now: WrapFit,
    fit: RowFit,
    apply: impl Fn(&mut S, WrapFit) + 'static,
) -> Div {
    let line = line.clone();
    row.child(
        canvas(
            move |bounds, _, _| bounds,
            move |bounds, _, _window, cx: &mut App| {
                let Some(next) = now.resolve(RowMetrics {
                    row_w: f32::from(bounds.size.width),
                    row_h: f32::from(bounds.size.height),
                    section_h: line.get(),
                    fit,
                }) else {
                    return;
                };
                let repaint = next.compact() != now.compact();
                cx.defer(move |app| {
                    entity.update(app, |panel, cx| {
                        apply(panel, next);
                        if repaint {
                            cx.notify();
                        }
                    });
                });
            },
        )
        .absolute()
        .inset_0(),
    )
}
