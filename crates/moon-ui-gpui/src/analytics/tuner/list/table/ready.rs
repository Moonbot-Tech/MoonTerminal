//! The strategy table's "Tuning" pieces ("Entry/Exit" only): the readiness cell, the progress
//! line and the filter's empty states. The same widgets as the auto-pick's on "By coin" — the
//! progress line uses the shared bar with a wrapping readiness caption.

use gpui::*;
use moon_ui::{MoonButton, MoonButtonVariant, MoonPalette, h_flex, v_flex};
use rust_i18n::t;

use super::super::super::super::AnalyticsView;
use super::super::super::ready::{ReadyClass, ReadyFilter, cell_tip};
use super::super::super::ticks::readiness::Readiness;
use super::super::super::{READY_MIN_W, READY_W};
use super::progress_row;
use crate::design;
use crate::design::{moon, moon_alpha};
use moon_core::db::analytics::GroupStat;

impl AnalyticsView {
    /// The readiness pass's line under the list header: "K of M" with a thin bar while it
    /// runs, the tunable count when done, an amber warning when the read failed.
    ///
    /// Args:
    ///     p: Active palette.
    ///     total: The header's "of Y": rows passing every filter but this one.
    ///     cx: View context, for sizing.
    ///
    /// Returns:
    ///     The line, or `None` off "Entry/Exit" and before the first pass.
    pub(super) fn ready_progress_el(
        &self,
        p: MoonPalette,
        total: usize,
        cx: &Context<Self>,
    ) -> Option<AnyElement> {
        if !self.strat_mode.shows_ready() {
            return None;
        }
        let (text, warn) = self.ready.status(total)?;
        Some(progress_row(text, warn, self.ready.fraction(), true, p, cx))
    }

    /// The list emptied by the "Tuning" filter: what it needs and the button that shows every
    /// strategy again.
    pub(super) fn ready_empty_state(&self, p: MoonPalette, cx: &Context<Self>) -> AnyElement {
        ready_empty_el(
            cx.listener(|this, _, _, cx| {
                this.ready.chosen = Some(ReadyFilter::All);
                this.strat_visible = None;
                cx.notify();
            }),
            p,
            cx,
        )
    }
}

/// The "Tuning" cell of one strategy row, the card's sentence on hover.
///
/// Args:
///     view: Current Analytics state, holding the pass.
///     g: The row's strategy.
///     p: Active palette.
///     cx: App, for sizing.
pub(super) fn ready_cell(
    view: &AnalyticsView,
    g: &GroupStat,
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
    let ready = view.ready.counts.get(&g.key);
    ready_cell_el(
        SharedString::from(format!("an-ready-{}", g.key)),
        view.ready.class(&g.key),
        ready.map(cell_tip),
        p,
        cx,
    )
}

/// The cell's text for a class, and whether it is drawn as a chip (tunable).
///
/// Returns:
///     `None` for an unread row, which stays empty.
pub(in crate::analytics::tuner) fn ready_cell_text(class: ReadyClass) -> Option<(String, bool)> {
    Some(match class {
        ReadyClass::Can { fit } => (t!("analytics.ready.chip_can", n = fit).to_string(), true),
        ReadyClass::Few { fit } => (
            t!("analytics.ready.chip_few", n = fit, min = Readiness::min()).to_string(),
            false,
        ),
        ReadyClass::NoTape => (t!("analytics.ready.chip_no_tape").to_string(), false),
        ReadyClass::Pending => ("\u{2013}".to_string(), false),
        ReadyClass::Unread => return None,
    })
}

/// The "Tuning" cell, as a free function so a layout probe can draw every class.
///
/// Args:
///     id: Element id; the text box carries `"{id}-txt"` as its debug selector.
///     class: What the pass says for the strategy.
///     tip: Hover text, when the pass holds a count.
///     p: Active palette.
///     cx: App, for sizing.
///
/// Returns:
///     The fixed-width, right-aligned cell.
pub(in crate::analytics::tuner) fn ready_cell_el(
    id: SharedString,
    class: ReadyClass,
    tip: Option<String>,
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
    let txt_sel = format!("{id}-txt");
    let cell_sel = format!("{id}-cell");
    let mut cell = h_flex()
        .id(id)
        .debug_selector(move || cell_sel)
        .w(design::font_w_px(cx, READY_W))
        .min_w(design::font_w_px(cx, READY_MIN_W))
        .flex_shrink_1()
        .justify_end()
        .text_size(design::t_caption(cx));
    if let Some((text, chip)) = ready_cell_text(class) {
        let fg = match class {
            ReadyClass::Can { .. } => p.green_text,
            ReadyClass::Few { .. } => p.text_soft,
            _ => p.text_muted,
        };
        let mut el = div()
            .flex_none()
            .debug_selector(move || txt_sel)
            .text_color(moon(fg))
            .child(text);
        if chip {
            el = el
                .px(design::ui_px(cx, 5.0))
                .rounded(design::ui_px(cx, 3.0))
                .bg(moon_alpha(p.green, 0.16));
        }
        cell = cell.child(el);
    }
    match tip {
        Some(tip) => cell
            .tooltip(crate::panels::common::text_tooltip(tip))
            .into_any_element(),
        None => cell.into_any_element(),
    }
}

/// The "Tuning"-filtered list while the pass is still computing: one muted sentence instead
/// of the empty state, which would be a guess until the pass answers.
///
/// Returns:
///     The centred note (`an-ready-computing`).
pub(in crate::analytics::tuner) fn ready_computing_el(p: MoonPalette, cx: &App) -> AnyElement {
    div()
        .id("an-ready-computing")
        .debug_selector(|| "an-ready-computing".to_string())
        .w_full()
        .p(design::ui_px(cx, 18.0))
        .font_family(design::ui_font())
        .text_center()
        .text_color(moon(p.text_muted))
        .child(t!("analytics.ready.computing").to_string())
        .into_any_element()
}

/// The "Tuning" filter's empty state, as a free function so a layout probe can draw it.
///
/// Args:
///     on_reset: The "Show every strategy" click.
///     p: Active palette.
///     cx: App, for sizing.
///
/// Returns:
///     A centred title, one sentence and the reset button.
pub(in crate::analytics::tuner) fn ready_empty_el(
    on_reset: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
    v_flex()
        .id("an-ready-empty")
        .debug_selector(|| "an-ready-empty".to_string())
        .w_full()
        .p(design::ui_px(cx, 18.0))
        .gap(design::ui_px(cx, 8.0))
        .items_center()
        .font_family(design::ui_font())
        .text_center()
        .child(
            div()
                .debug_selector(|| "an-ready-empty-title".to_string())
                .w_full()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(moon(p.text))
                .child(t!("analytics.ready.empty_title").to_string()),
        )
        .child(
            div()
                .debug_selector(|| "an-ready-empty-body".to_string())
                .w_full()
                .text_size(design::t_caption(cx))
                .text_color(moon(p.text_muted))
                .child(t!("analytics.ready.empty_body", min = Readiness::min()).to_string()),
        )
        .child(
            div()
                .debug_selector(|| "an-ready-empty-reset".to_string())
                .child(
                    MoonButton::new("an-ready-empty-reset")
                        .variant(MoonButtonVariant::Amber)
                        .label(t!("analytics.auto.show_all").to_string())
                        .on_click(on_reset)
                        .render(),
                ),
        )
        .into_any_element()
}
