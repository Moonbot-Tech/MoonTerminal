//! What every tuning axis shares: the axis tag the common shell dispatches on, the write targets
//! and save-dialog model, and the card, glyph-button, and collapse-caret UI helpers.
//!
//! Anything used by exactly ONE axis belongs to that axis' folder — this file is the contract
//! between them, and a helper that drifts in here stops being reviewable as shared.

use gpui::*;
use moon_ui::{
    MoonDisclosure, MoonDisclosureDirection, MoonPalette, MoonTooltipView, h_flex, v_flex,
};

use super::super::AnalyticsView;
use crate::design;
use crate::design::moon;

/// Variants besides "Fact" (V3 holds 8 — we start with two).
pub(super) const N_VAR: usize = 2;

/// Which tuning axis draws the shared shell (toolbar + suggestion row). The shell
/// is one for every mode; the actions (Search/Save/Copy) are dispatched by kind.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum TunerKind {
    /// "By filter" — thresholds of the market fields (the full set of actions).
    Filter,
    /// "By time" — the weekly schedule (actions arrive in phase 2b).
    Time,
    /// "By coin" — the `CoinsBlackList` field the picker builds.
    Coins,
}

/// A write target: a strategy ON A SPECIFIC core. `core` is the list row's `core_uid`,
/// which equals the live store's `CoreId` (identity verified: the report ingest writes
/// `core_uid: server.uid`, and strategies.sqlite `core_uid` is already used as a CoreId).
/// `None` is a legacy key without a core: write to every core of the strategy (back-compat).
#[derive(Clone)]
pub(super) struct SaveTarget {
    pub(super) sid: i64,
    pub(super) core: Option<u64>,
    pub(super) name: String,
}

/// A prepared save: target(s) + the list of changes. Single selection = 1 target;
/// Ctrl- or Shift-built multi-selection = N targets (the same changes for each). Copy = always 1.
pub(super) struct SaveDialog {
    pub(super) targets: Vec<SaveTarget>,
    pub(super) changes: Vec<(String, String)>,
    /// Current parameter values of the FIRST (anchor) target, indexed like `changes`
    /// (the dialog shows "now → next"); in multi-select the preview reflects the anchor
    /// and the rest are written blind. None — the parameter is absent from the strategy.
    pub(super) olds: Vec<Option<String>>,
    /// true — the "Make a copy" dialog: confirming creates a NEW strategy with these
    /// fields (name from the input) rather than editing the source.
    pub(super) copy: bool,
    /// Warnings (overwriting a foreign slot type, fields that did not fit) — shown in
    /// the confirmation dialog, not only in the log.
    pub(super) warns: Vec<String>,
    /// One value set PER TARGET, indexed like `targets`. `None` — every target gets
    /// `changes`, which is what a threshold write means.
    ///
    /// The coin axis needs it because its value is a SET the strategies do not share: the
    /// edit is applied to each strategy's OWN list, so the string sent to each differs. With
    /// one shared value, ticking a single coin on a multi-select copied the whole working
    /// list — itself the union of the selected strategies' lists — onto every one of them.
    pub(super) per_target: Option<Vec<Vec<(String, String)>>>,
    /// What actually changed, in words, indexed like `changes`. `None` — nothing to add
    /// beyond the value itself.
    ///
    /// Exists because "now → next" cannot be read for a SET. A coin list runs to hundreds of
    /// entries, and two walls of them differing by one coin are indistinguishable by eye —
    /// the dialog was showing the change without ever showing WHAT the change was. The axis
    /// that understands the field computes the sentence; the dialog only prints it.
    pub(super) notes: Vec<Option<String>>,
}

/// A card with a title and a subtitle (the shared look of the Analytics cards). `accessory`
/// is an optional element pinned to the RIGHT of the title bar (e.g. the KPI collapse caret);
/// `None` leaves the header exactly as the plain cards have it.
///
/// The text clips inside one shrinking box so a narrow card never pushes the accessory beyond its
/// `overflow_hidden` root.
pub(super) fn card(
    title: String,
    sub: String,
    body: AnyElement,
    accessory: Option<AnyElement>,
    p: MoonPalette,
    cx: &Context<AnalyticsView>,
) -> AnyElement {
    // One clipping box keeps the accessory outside the width that may yield.
    let mut text = h_flex()
        .flex_1()
        .min_w_0()
        .overflow_hidden()
        .whitespace_nowrap()
        .items_center()
        .gap(design::ui_px(cx, 8.0))
        .child(
            div()
                .flex_none()
                .text_size(design::t_title(cx))
                .font_weight(FontWeight::SEMIBOLD)
                .child(title),
        );
    if !sub.is_empty() {
        text = text.child(
            div()
                .flex_none()
                .text_size(design::t_caption(cx))
                .text_color(moon(p.text_muted))
                .child(sub),
        );
    }
    let mut head = h_flex()
        .w_full()
        .px(design::ui_px(cx, 12.0))
        .py(design::ui_px(cx, 8.0))
        .items_center()
        .gap(design::ui_px(cx, 8.0))
        .child(text);
    if let Some(acc) = accessory {
        head = head.child(div().flex_none().child(acc));
    }
    v_flex()
        .w_full()
        // Inside a scrolling column the cards must not shrink to the viewport height.
        .flex_none()
        .rounded(design::ui_px(cx, 8.0))
        .bg(moon(p.panel))
        .border_1()
        .border_color(moon(p.border))
        .overflow_hidden()
        .child(head)
        .child(body)
        .into_any_element()
}

/// Whether any staged "ignore" differs from the strategy's current flags.
/// A tuner-grid glyph button (→ / ← / ✕) with a hover tooltip `tip`; `hover` is the hover
/// color. SHARED by both axes ("By filter" and "By time") — the caller adds its own `.on_click`.
/// Returns a stateful div.
pub(super) fn glyph_btn(
    id: impl Into<ElementId>,
    glyph: &'static str,
    tip: String,
    hover: u32,
    p: MoonPalette,
    cx: &Context<AnalyticsView>,
) -> Stateful<Div> {
    div()
        .id(id)
        .w(design::ui_px(cx, 12.0))
        .flex_none()
        .cursor_pointer()
        .text_size(design::t_caption(cx))
        .text_color(moon(p.text_muted))
        .hover(move |s| s.text_color(moon(hover)))
        .tooltip(move |_w, cx| cx.new(|_| MoonTooltipView::new(tip.clone())).into())
        .child(glyph)
}

/// A card's collapse caret, for the title-bar `accessory` slot.
///
/// Draws MoonUI's shared [`MoonDisclosure`] in the explicit `DownUp` pose: down while collapsed and
/// up while expanded. The helper selects the pose and tooltip together, and callers build it before
/// matching their data state so the caret remains visible during loading and read failures.
///
/// `MoonDisclosure` implements `RenderOnce`, so the click handler is supplied before rendering.
/// Pass `cx.listener(..)`; its output type matches `on_toggle` and keeps this helper independent of
/// a concrete view type.
///
/// Args:
///     id: Stable element id of this card's caret.
///     collapsed: Whether the card is folded right now.
///     collapse_tip: Tooltip shown while expanded (what the click will do).
///     expand_tip: Tooltip shown while collapsed.
///     p: Active palette used for the hover color.
///     on_toggle: Receives the next expanded state on click; build it with `cx.listener(..)`.
///
/// Returns:
///     An interactive caret ready for the accessory slot.
pub(super) fn collapse_caret(
    id: impl Into<ElementId>,
    collapsed: bool,
    collapse_tip: String,
    expand_tip: String,
    p: MoonPalette,
    on_toggle: impl Fn(&bool, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let tip = if collapsed { expand_tip } else { collapse_tip };
    MoonDisclosure::button(id, !collapsed)
        .direction(MoonDisclosureDirection::DownUp)
        .size(design::DISCLOSURE_GLYPH)
        .box_size(design::DISCLOSURE_BOX)
        .hover_color(p.text)
        .tooltip(tip)
        .on_toggle(on_toggle)
        .into_any_element()
}
