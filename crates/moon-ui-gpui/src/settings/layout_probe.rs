//! Layout probe for Settings pages: draws a page builder in the geometry the real window gives
//! its body, in every language and at the wide and the minimum window size, and checks that
//! nothing escapes its container, nothing is clipped and nothing overlaps.
//!
//! Every reworked page ships one [`sweep`] over its builder with a synthetic model. A page cannot
//! be built in a test as a whole `SettingsView` (it needs a `Backend`), so the probe lays out free
//! builder functions inside a small host view.

use std::cell::RefCell;

use gpui::*;
use moon_core::config::Language;

use super::nav::{nav_hidden, nav_w};
use super::render::BODY_PAD;
use crate::design;

/// Window sizes swept: the default window, the minimum Settings window, a ~900 px window and a
/// wide one as tall as a full-height Settings window on a 1080p-class screen.
pub(super) const SIZES: [(f32, f32); 5] = [
    (860.0, 620.0),
    (760.0, 620.0),
    (620.0, 420.0),
    (900.0, 700.0),
    (1504.0, 1190.0),
];

thread_local! {
    /// Labels drawn since the last [`probe_once`] cleared it, recorded by the shared word floor
    /// (`common::word_floored`), so every page that draws its labels through it is word-checked.
    static LABELS: RefCell<Vec<FloorRecord>> = const { RefCell::new(Vec::new()) };
}

/// One label for the word check: its box, its text, whether it is a caption, and the width of
/// the box before the label (a checkbox square), in layout pixels.
struct FloorRecord {
    selector: String,
    text: String,
    caption: bool,
    lead: f32,
}

/// Note a label drawn through the shared word floor, for the word check of [`probe_once`].
///
/// Args:
///     selector: The label box's `debug_selector`.
///     text: The label.
///     caption: Whether it is drawn at the caption size.
///     lead: Layout pixels of the box before the label.
pub(super) fn record_floor(selector: String, text: &str, caption: bool, lead: f32) {
    LABELS.with(|labels| {
        labels.borrow_mut().push(FloorRecord {
            selector,
            text: text.to_string(),
            caption,
            lead,
        })
    });
}

/// Height of the page picker that replaces the sidebar on a narrow window, unscaled.
const PICKER_H: f32 = 44.0;
/// Height of the window title bar, unscaled.
const TITLE_H: f32 = super::SETTINGS_HEADER_H;

/// Which face a single-line text is drawn in.
#[derive(Clone, Copy, Debug)]
pub(super) enum TextFace {
    Ui,
    /// The page title: UI face, 17px semibold.
    Title,
}

/// What one probe run checks. Selectors are `debug_selector` names.
#[derive(Default)]
pub(super) struct ProbeSpec {
    /// The element everything below must stay inside.
    pub(super) container: &'static str,
    /// Single-line labels and values: `(selector, text, face)`; the element must be at least as
    /// wide as the text.
    pub(super) fits: Vec<(String, String, TextFace)>,
    /// Wrapping prose: `(selector, text)`; the element must be at least as tall as the text needs
    /// at the element's width, so no line is cut off.
    pub(super) wraps: Vec<(String, String)>,
    /// Elements that must lie inside the container (0.5 px tolerance).
    pub(super) inside: Vec<String>,
    /// Groups of elements no two of which may intersect.
    pub(super) no_overlap: Vec<Vec<String>>,
    /// Groups of elements of which any two on one line must share their top edge and height: a row of
    /// tiles that stretches to its tallest member.
    pub(super) same_row_same_height: Vec<Vec<String>>,
    /// The last element of a scrolling page: it must lie within the scroll extent.
    pub(super) last: Option<String>,
    /// Groups of elements that must be equally wide (0.5 px tolerance).
    pub(super) same_width: Vec<Vec<String>>,
    /// `(upper, lower, gap)`: `lower` starts exactly `gap` unscaled pixels below `upper`'s bottom
    /// (1 px tolerance) — neither a hole between them nor `lower` riding over `upper`.
    pub(super) stacked: Vec<(String, String, f32)>,
}

/// A view a sweep can draw: it lays a page builder out and exposes its scroll handle.
pub(super) trait Probed: Render + 'static {
    /// The page's scroll handle, when the page scrolls.
    fn scroll(&self) -> Option<ScrollHandle>;
}

/// The page body's content box for a window of `size`, as `render.rs` lays it out: the sidebar
/// (or the picker below the breakpoint), the title bar and the body padding taken off.
///
/// Args:
///     size: Window width and height in logical pixels.
///     cx: Application context for the zoom-scaled insets.
///
/// Returns:
///     Content width and the viewport height the body scrolls in.
pub(super) fn body_box(size: (f32, f32), cx: &App) -> (f32, f32) {
    let (w, h) = size;
    let pad = 2.0 * design::ui_value(cx, BODY_PAD);
    let title = design::ui_value(cx, TITLE_H);
    if !nav_hidden(w, cx) {
        ((w - nav_w(cx) - pad).max(0.0), h - title)
    } else {
        (
            (w - pad).max(0.0),
            h - title - design::ui_value(cx, PICKER_H),
        )
    }
}

/// `debug_bounds` takes a `'static` selector; the probe's selectors are built per run.
pub(super) fn leak(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

/// Bounds of `sel`, or the reason it is missing.
fn bounds(visual: &mut VisualTestContext, sel: &str) -> Result<Bounds<Pixels>, String> {
    visual
        .debug_bounds(leak(sel.to_string()))
        .ok_or_else(|| format!("{sel} was not laid out"))
}

/// Run every check of `spec` against the window drawn in `visual`.
///
/// Args:
///     visual: The drawn window.
///     scroll: The page's scroll handle, for the reachability check.
///     spec: What to check.
///
/// Returns:
///     `Err` naming the first broken check.
pub(super) fn check(
    visual: &mut VisualTestContext,
    scroll: Option<&ScrollHandle>,
    spec: &ProbeSpec,
) -> Result<(), String> {
    let container = bounds(visual, spec.container)?;
    let tol = px(0.5);
    for sel in &spec.inside {
        let b = bounds(visual, sel)?;
        if b.origin.x + tol < container.origin.x
            || b.right() > container.right() + tol
            || b.origin.y + tol < container.origin.y
        {
            return Err(format!("{sel} escapes {}", spec.container));
        }
    }
    for (sel, text, face) in &spec.fits {
        let b = bounds(visual, sel)?;
        let need = visual.update(|_, cx| match face {
            TextFace::Ui => design::ui_body_text_width(cx, text, 400.0),
            TextFace::Title => design::ui_text_width_zoomed(cx, text, 17.0, 600.0, false),
        });
        if f32::from(b.size.width) + 0.5 < need {
            return Err(format!(
                "{sel} is narrower than {text:?} ({} < {need})",
                f32::from(b.size.width)
            ));
        }
    }
    for (sel, text) in &spec.wraps {
        let b = bounds(visual, sel)?;
        // A hint row's long text (`hint_row`, the `-more-text` selector) is drawn as a caption.
        let caption = sel.ends_with("-more-text");
        let (width, line) = visual.update(|_, cx| {
            if caption {
                (
                    design::ui_caption_text_width(cx, text, 400.0),
                    f32::from(design::line_px(cx, 12.0)),
                )
            } else {
                (
                    design::ui_body_text_width(cx, text, 400.0),
                    f32::from(design::line_px(cx, 14.0)),
                )
            }
        });
        let lines = (width / f32::from(b.size.width).max(1.0)).ceil().max(1.0);
        if f32::from(b.size.height) + 0.5 < lines * line {
            return Err(format!(
                "{sel} cuts {text:?} off vertically ({} x {} for {lines} lines)",
                f32::from(b.size.width),
                f32::from(b.size.height)
            ));
        }
    }
    for group in &spec.no_overlap {
        let boxes = group
            .iter()
            .map(|sel| bounds(visual, sel).map(|b| (sel, b)))
            .collect::<Result<Vec<_>, _>>()?;
        for (i, (a, ab)) in boxes.iter().enumerate() {
            for (b, bb) in &boxes[i + 1..] {
                if ab.intersects(bb) && ab.intersect(bb).size.width > tol {
                    return Err(format!("{a} overlaps {b}"));
                }
            }
        }
    }
    for group in &spec.same_row_same_height {
        let boxes = group
            .iter()
            .map(|sel| bounds(visual, sel).map(|b| (sel, b)))
            .collect::<Result<Vec<_>, _>>()?;
        for (i, (a, ab)) in boxes.iter().enumerate() {
            for (b, bb) in &boxes[i + 1..] {
                // Vertically overlapping boxes share a line of the wrapping row.
                let same_row = ab.origin.y < bb.bottom_left().y && bb.origin.y < ab.bottom_left().y;
                let dy = (f32::from(ab.origin.y) - f32::from(bb.origin.y)).abs();
                let dh = (f32::from(ab.size.height) - f32::from(bb.size.height)).abs();
                if same_row && (dy > f32::from(tol) || dh > f32::from(tol)) {
                    return Err(format!(
                        "{a} and {b} share a row but are offset by {dy} and differ in height by {dh}"
                    ));
                }
            }
        }
    }
    for group in &spec.same_width {
        let boxes = group
            .iter()
            .map(|sel| bounds(visual, sel).map(|b| (sel, b)))
            .collect::<Result<Vec<_>, _>>()?;
        for (a, ab) in &boxes {
            let (b, bb) = &boxes[0];
            let dw = (f32::from(ab.size.width) - f32::from(bb.size.width)).abs();
            if dw > f32::from(tol) {
                return Err(format!("{a} and {b} differ in width by {dw}"));
            }
        }
    }
    for (upper, lower, gap_px) in &spec.stacked {
        let ub = bounds(visual, upper)?;
        let lb = bounds(visual, lower)?;
        let gap = f32::from(lb.origin.y) - f32::from(ub.bottom_left().y);
        let want = visual.update(|_, cx| design::ui_value(cx, *gap_px));
        if (gap - want).abs() > 1.0 {
            return Err(format!(
                "{lower} sits {gap} px under {upper} ({want} expected)"
            ));
        }
    }
    if let (Some(last), Some(scroll)) = (&spec.last, scroll) {
        let tail = bounds(visual, last)?;
        let reachable = scroll.bounds().bottom_right().y + scroll.max_offset().y;
        if reachable + tol < tail.bottom_right().y {
            return Err(format!(
                "{last} lies outside the scroll extent ({} > {reachable})",
                f32::from(tail.bottom_right().y)
            ));
        }
    }
    Ok(())
}

/// Open `make`'s view at `size`, draw it three times (layout settles over frames) and check it.
///
/// Args:
///     cx: Test context.
///     size: Window size the body box is derived from.
///     make: Builds the probe view for a body box `(width, viewport height)`.
///     spec: What to check.
///
/// Returns:
///     The first broken check, if any.
pub(super) fn probe_once<V: Probed>(
    cx: &mut TestAppContext,
    size: (f32, f32),
    make: &impl Fn(&mut Window, &mut Context<V>, (f32, f32)) -> V,
    spec: &ProbeSpec,
) -> Result<(), String> {
    LABELS.with(|labels| labels.borrow_mut().clear());
    let window = cx.open_window(gpui::size(px(size.0), px(size.1)), |window, cx| {
        let body = body_box(size, cx);
        make(window, cx, body)
    });
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    for _ in 0..3 {
        visual.update(|window, cx| {
            let _ = window.draw(cx);
        });
    }
    let scroll = window
        .update(&mut visual, |view, _, _| view.scroll())
        .expect("probe window open");
    check(&mut visual, scroll.as_ref(), spec)?;
    check_words(&mut visual)
}

/// No row label drawn in this run is narrower than its longest word: a label column squeezed
/// below that breaks a word across lines, one glyph per line at worst.
///
/// Args:
///     visual: The drawn window.
///
/// Returns:
///     `Err` naming the first squeezed label.
fn check_words(visual: &mut VisualTestContext) -> Result<(), String> {
    let labels = LABELS.with(|labels| std::mem::take(&mut *labels.borrow_mut()));
    for FloorRecord {
        selector: sel,
        text,
        caption,
        lead,
    } in labels
    {
        // A label recorded but not laid out (a collapsed section) has nothing to squeeze.
        let Some(b) = visual.debug_bounds(leak(sel.clone())) else {
            continue;
        };
        // Measured here, not through `common::longest_word_w`, so the check does not trust the
        // floor it checks.
        let need = lead
            + visual.update(|_, cx| {
                text.split_whitespace()
                    .map(|word| {
                        if caption {
                            design::ui_caption_text_width(cx, word, 400.0)
                        } else {
                            design::ui_body_text_width(cx, word, 400.0)
                        }
                    })
                    .fold(0.0_f32, f32::max)
            });
        if f32::from(b.size.width) + 0.5 < need {
            return Err(format!(
                "{sel} is narrower than the longest word of {text:?} ({} < {need})",
                f32::from(b.size.width)
            ));
        }
    }
    Ok(())
}

/// Sweep a page builder over every language and both [`SIZES`]; panics on the first broken check.
///
/// Args:
///     cx: Test context.
///     make: Builds the probe view for a body box `(width, viewport height)`.
///     spec: What to check, per language (texts differ by language).
pub(super) fn sweep<V: Probed>(
    cx: &mut TestAppContext,
    make: impl Fn(&mut Window, &mut Context<V>, (f32, f32)) -> V,
    spec: impl Fn(Language) -> ProbeSpec,
) {
    cx.update(moon_ui::init);
    for language in Language::ALL {
        let _locale = crate::test_locale::force(language.code());
        let spec = spec(language);
        for size in SIZES {
            if let Err(broken) = probe_once(cx, size, &make, &spec) {
                panic!("{broken} ({} at {}x{})", language.code(), size.0, size.1);
            }
        }
    }
}

#[cfg(test)]
mod tests;
