//! What every tuning axis shares: the axis tag the common shell dispatches on, the write targets
//! and save-dialog model, and the card, glyph-button, and collapse-caret UI helpers.
//!
//! Anything used by exactly ONE axis belongs to that axis' folder — this file is the contract
//! between them, and a helper that drifts in here stops being reviewable as shared.

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{MoonDisclosure, MoonDisclosureDirection, MoonPalette, h_flex, v_flex};
use rust_i18n::t;

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
    /// "Entry/Exit" — the tape replay; named by how it evaluates, not by what it shows.
    Ticks,
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

/// Immutable authority carried from Save/Copy preparation through final command dispatch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct SaveAuthority {
    /// Selection that produced this confirmation.
    pub(super) producer: super::save::SaveProducer,
    /// Dialog/draft sequence captured before any asynchronous preview or live-core read.
    pub(super) dialog_seq: u64,
    /// Workspace generation for Auto, or `None` for intentionally unscoped Classic behavior.
    pub(super) workspace_generation: Option<u64>,
    /// Complete concrete Auto scope captured with the producer, normalized for comparison.
    pub(super) workspace_cores: Option<Vec<u64>>,
    /// Ordered strategy identities; order is significant for per-target edit payloads.
    pub(super) targets: Vec<(i64, Option<u64>)>,
}

/// An inline confirmation's immutable producer authority and exact write payload.
pub(in crate::analytics::tuner) struct InlineWrite {
    pub(in crate::analytics::tuner) authority: SaveAuthority,
    pub(in crate::analytics::tuner) targets: Vec<SaveTarget>,
    pub(in crate::analytics::tuner) changes: Vec<(String, String)>,
}

/// Wrap a control in a content-sized, non-shrinking box carrying a layout-probe selector.
pub(in crate::analytics::tuner) fn tagged(sel: impl Into<String>, el: impl IntoElement) -> Div {
    let sel = sel.into();
    div().flex_none().debug_selector(move || sel).child(el)
}

/// A prepared save: target(s) + the list of changes. Single selection = 1 target;
/// Ctrl- or Shift-built multi-selection = N targets with shared or individual edits. Copy = 1.
pub(super) struct SaveDialog {
    /// Producer identity that must still match before any target is dispatched.
    pub(super) authority: SaveAuthority,
    pub(super) targets: Vec<SaveTarget>,
    pub(super) changes: Vec<(String, String)>,
    /// Current parameter values of the FIRST (anchor) target, indexed like `changes`
    /// (the dialog shows "now → next"). Shared bulk edits hide the anchor's old value;
    /// individual edits use `per_target_olds`. None means the parameter is absent or unknown.
    pub(super) olds: Vec<Option<String>>,
    /// true — the "Make a copy" dialog: confirming creates a NEW strategy with these
    /// fields (name from the input) rather than editing the source.
    pub(super) copy: bool,
    /// Warnings (overwriting a foreign slot type, fields that did not fit) — shown in
    /// the confirmation dialog, not only in the log.
    pub(super) warns: Vec<String>,
    /// One value set PER TARGET, indexed like `targets`. `None` — every target gets `changes`.
    ///
    /// Mixed-kind threshold saves and the coin axis need distinct payloads. A coin value is a
    /// SET the strategies do not share: edits apply to each strategy's OWN list. With
    /// one shared value, ticking a single coin on a multi-select copied the whole working
    /// list — itself the union of the selected strategies' lists — onto every one of them.
    pub(super) per_target: Option<Vec<Vec<(String, String)>>>,
    /// Current values aligned with each per-target edit list; missing values stay unknown.
    pub(super) per_target_olds: Vec<Vec<Option<String>>>,
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
    cx: &App,
) -> AnyElement {
    card_header_layout(title, sub, body, accessory, false, p, cx)
}

/// Let prose captions wrap within the card while legacy data captions retain their layout.
pub(super) fn card_with_wrapping_caption(
    title: String,
    sub: String,
    body: AnyElement,
    accessory: Option<AnyElement>,
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
    card_header_layout(title, sub, body, accessory, true, p, cx)
}

/// Share the card frame and accessory positioning across data and wrapping prose headers.
fn card_header_layout(
    title: String,
    sub: String,
    body: AnyElement,
    accessory: Option<AnyElement>,
    wrap: bool,
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
    // Data headers keep their inherited mono face and clipping contract. Prose-only headers
    // wrap their captions while the accessory remains outside the yielding text box.
    let mut text = h_flex()
        .flex_1()
        .min_w_0()
        .when(!wrap, |text| text.overflow_hidden().whitespace_nowrap())
        .when(wrap, |text| text.flex_wrap().font_family(design::ui_font()))
        .items_center()
        .gap(design::ui_px(cx, 8.0))
        .child(
            div()
                .flex_none()
                .font_family(design::ui_font())
                .text_size(design::t_title(cx))
                .font_weight(FontWeight::SEMIBOLD)
                .debug_selector(|| "an-tuner-card-title".to_string())
                .child(title),
        );
    if !sub.is_empty() {
        text = text.child(
            div()
                .min_w_0()
                .font_family(design::mono())
                .when(!wrap, |caption| caption.truncate())
                .when(wrap, |caption| caption.min_w_0().max_w_full())
                .text_size(design::t_caption(cx))
                .text_color(moon(p.text_muted))
                .debug_selector(|| "an-tuner-card-caption".to_string())
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
        // Auxiliary and KPI cards keep their content height above flexible Parameters.
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
        .tooltip(crate::panels::common::text_tooltip(tip))
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

impl AnalyticsView {
    /// Read the display-only detail fold for an axis without changing any tuning input.
    pub(super) fn bottom_collapsed(&self, mode: super::StratMode) -> bool {
        match mode {
            super::StratMode::Filters => self.hist_collapsed,
            super::StratMode::Time => self.tuner_bottom_collapsed[0],
            super::StratMode::Coins => self.tuner_bottom_collapsed[1],
            super::StratMode::Ticks => self.tuner_bottom_collapsed[2],
        }
    }

    /// Build the common detail-card caret; folding affects presentation only.
    pub(super) fn bottom_caret(
        &self,
        mode: super::StratMode,
        p: MoonPalette,
        cx: &Context<Self>,
    ) -> AnyElement {
        collapse_caret(
            "tuner-detail-collapse",
            self.bottom_collapsed(mode),
            t!("analytics.tuner.hist_collapse").to_string(),
            t!("analytics.tuner.hist_expand").to_string(),
            p,
            cx.listener(move |this, _, _, cx| {
                match mode {
                    super::StratMode::Filters => this.toggle_hist_collapsed(cx),
                    super::StratMode::Time => this.tuner_bottom_collapsed[0] ^= true,
                    super::StratMode::Coins => this.tuner_bottom_collapsed[1] ^= true,
                    super::StratMode::Ticks => this.tuner_bottom_collapsed[2] ^= true,
                }
                cx.notify();
            }),
        )
    }

    /// Retain the detail heading, strategy subtitle and caret while its body is folded.
    pub(super) fn folded_bottom(
        &self,
        mode: super::StratMode,
        title: String,
        p: MoonPalette,
        cx: &Context<Self>,
    ) -> AnyElement {
        card(
            title,
            self.scope_label(),
            div().into_any_element(),
            Some(self.bottom_caret(mode, p, cx)),
            p,
            cx,
        )
    }

    /// The narrow full-height rail between a tuning axis' left area and its right-hand column,
    /// carrying the one caret that folds that column away.
    ///
    /// Built in BOTH states, and that is the whole point: once the column is gone this rail is
    /// the only control left that can bring it back. Folding it into the expanded branch — while
    /// "simplifying" a render guard, or because it reads as part of the column — makes the
    /// collapse a one-way door whose state survives a restart, so the user's only recovery is
    /// hand-editing `layout.toml`.
    ///
    /// A METHOD rather than a free function taking its tooltips, unlike [`collapse_caret`]: that
    /// helper serves carets whose wording differs per card, while every rail in the tab is the
    /// same control over the same flag, so all three axes would otherwise re-derive the same two
    /// keys and the same listener.
    ///
    /// The caret is that shared [`collapse_caret`], so a horizontal fold borrows the page's
    /// vertical up/down pose: `MoonDisclosureDirection` offers no horizontal pair, and agreeing
    /// with the two carets already on this page beats axis-literalism. The tooltips carry the
    /// meaning.
    ///
    /// Args:
    ///     p: Active palette used for the caret's hover color.
    ///     cx: GPUI context, for the UI-scaled rail width and the toggle listener.
    ///
    /// Returns:
    ///     The rail, ready to sit between the left area and the column.
    pub(super) fn side_rail(&self, p: MoonPalette, cx: &Context<Self>) -> AnyElement {
        v_flex()
            .flex_none()
            // DISCLOSURE_BOX plus breathing room, through `ui_px` so the rail tracks the UI
            // slider exactly as the caret's own `box_size` does.
            .w(design::ui_px(cx, 16.0))
            .h_full()
            .items_center()
            .justify_center()
            .child(collapse_caret(
                "an-tuner-side-collapse",
                self.side_collapsed,
                t!("analytics.tuner.side_collapse").to_string(),
                t!("analytics.tuner.side_expand").to_string(),
                p,
                cx.listener(|this, _, _, cx| this.toggle_side_collapsed(cx)),
            ))
            .into_any_element()
    }
}
