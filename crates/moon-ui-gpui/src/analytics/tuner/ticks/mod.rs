//! The "Entry/Exit" axis of the strategy tuner: the scope's deals replayed over the trade
//! tape rather than masked by SQL.
//!
//! Left, under the strategy list: the deal table — one row per closed trade with millisecond
//! stamps, its market at the buy, why it closed, whether the terminal holds its tape, and
//! whether the model reproduces the fact. Right: the shared "Fact vs …" matrix (the whole
//! scope beside the replayable subset, captioned with the ✓ shares) and the parameter grid.
//! Phase 1 stops there — the variant columns and the search land with phase 2 — and says so
//! in its captions rather than hiding the gap.
//!
//! The model itself is `moon_core::db::tuner::ticks`; this module only feeds it and draws
//! what it says.

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    MoonButton, MoonButtonVariant, MoonPalette, MoonScrollbarVisibility, MoonTooltipView,
    MoonVirtualList, h_flex, v_flex,
};
use rust_i18n::t;

use super::super::AnalyticsView;
use super::kpi::{VarLabel, kpi_matrix_card};
use super::{sort_arrow_of, toggle_sort_key};
use crate::design;
use crate::design::{moon, moon_alpha};
use columns::*;
use state::{DealRow, TapeStatus};

pub(in crate::analytics::tuner) mod columns;
mod fetch;
mod grid;
mod load;
pub(in crate::analytics::tuner) mod rows;
pub(in crate::analytics) mod state;

impl AnalyticsView {
    /// The deal table card — sits UNDER the strategy list, where the coin table sits in "By
    /// coin".
    pub(in crate::analytics::tuner) fn ticks_card(
        &mut self,
        p: MoonPalette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let scale = design::font_scale(cx);
        let scope = self.scope_label();
        let zone = self.query().axis.zone();
        // The order is settled before the data is viewed: both live in `ticks`, and the sort
        // cache needs the mutable half.
        let drawn = rows::order_for(&mut self.ticks).len();
        let summary = self.ticks.data.view(|d| d.rows.is_empty()).map(|d| {
            (
                d.rows.len(),
                d.covered(),
                d.fetchable().count(),
                d.without_ms,
            )
        });
        let (body, total, covered, fetchable, without_ms) = match summary {
            Err(note) => (
                super::super::note_el("an-ticks-note", note, 10.0, p, cx),
                0usize,
                0usize,
                0usize,
                0usize,
            ),
            Ok((total, covered, fetchable, without_ms)) => {
                let weak = cx.entity().downgrade();
                let row_h = deal_row_h(cx);
                let list =
                    MoonVirtualList::new("an-ticks-rows", drawn, row_h, move |ix, _w, app| {
                        weak.upgrade()
                            .and_then(|e| {
                                let view = e.read(app);
                                // Order and rows are read from the view in ONE go, so an
                                // index is never applied to a row set it was not built
                                // against.
                                let order = view.ticks.order.as_ref()?;
                                let row =
                                    view.ticks.data.data()?.rows.get(*order.order.get(ix)?)?;
                                Some(deal_row(row, p, scale, row_h, zone, app))
                            })
                            .unwrap_or_else(|| div().into_any_element())
                    })
                    .surface(false)
                    .border(false)
                    .radius(0.0)
                    .scrollbar_visibility(MoonScrollbarVisibility::Hover)
                    .into_any_element();
                (list, total, covered, fetchable, without_ms)
            }
        };
        let fetch_active = self.ticks.fetch.is_active();
        let fetch_label = if fetch_active {
            t!(
                "analytics.ticks.fetch_progress",
                done = self.ticks.fetch.done,
                total = self.ticks.fetch.total
            )
            .to_string()
        } else {
            t!("analytics.ticks.fetch_btn").to_string()
        };
        v_flex()
            .w_full()
            .flex_1()
            .min_h_0()
            .rounded(design::ui_px(cx, 8.0))
            .bg(moon(p.panel))
            .border_1()
            .border_color(moon(p.border))
            .overflow_hidden()
            .child(
                h_flex()
                    .w_full()
                    .flex_none()
                    .h(design::fit_h_px(cx, 34.0, 14.0, 8.0))
                    .px(design::ui_px(cx, 12.0))
                    .items_center()
                    .gap(design::ui_px(cx, 8.0))
                    .child(
                        div()
                            .flex_none()
                            .font_family(design::ui_font())
                            .text_size(design::t_title(cx))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(t!("analytics.ticks.title").to_string()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(design::t_caption(cx))
                            .text_color(moon(p.text_muted))
                            .child(scope),
                    )
                    // "N with tape of M · K without stamps": the honest size of the sample.
                    .child(
                        div()
                            .flex_none()
                            .font_family(design::ui_font())
                            .text_size(design::t_caption(cx))
                            .text_color(moon(p.text_muted))
                            .child(
                                t!(
                                    "analytics.ticks.coverage",
                                    covered = covered,
                                    total = total,
                                    without = without_ms
                                )
                                .to_string(),
                            ),
                    )
                    .when(fetchable > 0 || fetch_active, |el| {
                        el.child(
                            div().font_family(design::ui_font()).child(
                                MoonButton::new("an-ticks-fetch")
                                    .variant(if fetch_active {
                                        MoonButtonVariant::Amber
                                    } else {
                                        MoonButtonVariant::Soft
                                    })
                                    .label(fetch_label)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        if fetch_active {
                                            this.ticks_fetch_stop(cx);
                                        } else {
                                            this.ticks_fetch_missing(cx);
                                        }
                                        cx.notify();
                                    }))
                                    .render(),
                            ),
                        )
                    }),
            )
            .child(self.deal_header(p, cx))
            // The virtual list owns its own scrolling.
            .child(div().w_full().flex_1().min_h_0().child(body))
            .into_any_element()
    }

    /// The table's heading row: every column sortable, the arrow on the active one.
    fn deal_header(&self, p: MoonPalette, cx: &Context<Self>) -> impl IntoElement + use<> {
        let scale = design::font_scale(cx);
        let sortable =
            |id: SharedString, title: String, key: &'static str, col: Option<&DealCol>| {
                let arrow = sort_arrow_of(&self.ticks.sort, key);
                let d = div()
                    .id(id)
                    .flex_none()
                    .truncate()
                    .cursor_pointer()
                    .text_color(if arrow.is_empty() {
                        moon(p.text_soft)
                    } else {
                        moon(p.amber)
                    })
                    .child(format!("{title}{arrow}"))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        toggle_sort_key(&mut this.ticks.sort, key);
                        this.ticks.order = None;
                        cx.notify();
                    }));
                match col {
                    Some(col) => {
                        let d = d
                            .w(px(col.w * scale))
                            .min_w(px(col.min_w * scale))
                            .flex_shrink_1();
                        match col.align {
                            Align::Right => d.text_right(),
                            Align::Center => d.text_center(),
                            Align::Left => d,
                        }
                    }
                    None => d.flex_1().min_w(px(DEAL_COIN_MIN_W * scale)),
                }
            };
        h_flex()
            .w_full()
            .flex_none()
            .h(design::fit_h_px(cx, 22.0, 12.0, 5.0))
            .px(design::ui_px(cx, DEAL_ROW_PAD_X))
            .gap(design::ui_px(cx, DEAL_ROW_GAP))
            .items_center()
            .text_size(design::t_caption(cx))
            .text_color(moon(p.text_soft))
            .bg(moon(p.table_head))
            .child(sortable(
                "an-ticks-hdr-coin".into(),
                t!("analytics.col.coin").to_string(),
                COL_COIN,
                None,
            ))
            .children(DEAL_COLS.iter().map(|c| {
                sortable(
                    SharedString::from(format!("an-ticks-hdr-{}", c.key)),
                    t!(c.label).to_string(),
                    c.key,
                    Some(c),
                )
            }))
    }

    /// The right column of the axis: the matrix on top, the grid below it, scrolling as one.
    pub(in crate::analytics::tuner) fn ticks_side(
        &self,
        p: MoonPalette,
        cx: &Context<Self>,
    ) -> AnyElement {
        v_flex()
            .w_full()
            .h_full()
            .min_h_0()
            .gap(design::ui_px(cx, 8.0))
            .child(self.ticks_kpi(p, cx))
            .child(
                div()
                    .id("an-ticks-grid-scroll")
                    .w_full()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(self.ticks_grid(p, cx)),
            )
            .into_any_element()
    }

    /// "Fact vs …": the whole scope beside the rows the tape covers, the second captioned
    /// with the ✓ shares of both groups — the model's own account of itself.
    fn ticks_kpi(&self, p: MoonPalette, cx: &Context<Self>) -> AnyElement {
        let (covered, total, entry, exit) = self
            .ticks
            .data
            .data()
            .map(|d| (d.covered(), d.rows.len(), d.entry_share, d.exit_share))
            .unwrap_or_default();
        let share = |(hits, n): (usize, usize)| -> String {
            if n == 0 {
                "—".to_string()
            } else {
                format!("{:.0} %", hits as f64 / n as f64 * 100.0)
            }
        };
        let labels = vec![VarLabel::with_sub(
            t!("analytics.ticks.subset").to_string(),
            t!(
                "analytics.ticks.subset_sub",
                n = covered,
                m = total,
                entry = share(entry),
                exit = share(exit)
            )
            .to_string(),
        )];
        // `TicksState::kpi` is `TicksData::kpi` — `[fact, subset]` — under the matrix's shape.
        kpi_matrix_card(
            &self.ticks.kpi,
            self.scope_label(),
            &labels,
            self.kpi_collapsed,
            p,
            cx,
        )
    }
}

/// Height of one deal row, in base px — the single pitch the list and the row share.
fn deal_row_h(cx: &App) -> f32 {
    design::fit_h_value(cx, 24.0, 14.0, 5.0)
}

/// Wall-clock time of a millisecond stamp in the selected zone.
fn hms(unix_ms: i64, zone: chrono_tz::Tz) -> String {
    moon_core::util::display_time::at_millis(unix_ms, zone)
        .map(|value| value.format("%H:%M:%S").to_string())
        .unwrap_or_default()
}

/// A duration in the shortest unit that keeps it readable.
fn duration_text(ms: i64) -> String {
    let s = ms.max(0) as f64 / 1000.0;
    if s < 60.0 {
        format!("{s:.0}s")
    } else if s < 3600.0 {
        format!("{:.1}m", s / 60.0)
    } else {
        format!("{:.1}h", s / 3600.0)
    }
}

/// A delta cell: signed, one decimal, dimmed at zero.
fn delta_text(v: f64) -> String {
    if v == 0.0 {
        "—".to_string()
    } else {
        format!("{v:+.1}")
    }
}

/// The mark of the tape column and its tooltip.
fn tape_mark(tape: TapeStatus) -> (&'static str, String) {
    match tape {
        TapeStatus::Covered => ("●", t!("analytics.ticks.tape_covered").to_string()),
        TapeStatus::Missing => ("○", t!("analytics.ticks.tape_missing").to_string()),
        TapeStatus::Fetching => ("…", t!("analytics.ticks.tape_fetching").to_string()),
        TapeStatus::NoAddress => ("·", t!("analytics.ticks.tape_no_address").to_string()),
        TapeStatus::Refused(status) => (
            "✕",
            t!(
                "analytics.ticks.tape_refused",
                status = format!("{status:?}")
            )
            .to_string(),
        ),
    }
}

/// The mark of the model column — one glyph per group — and its tooltip with the deviations.
fn model_mark(row: &DealRow) -> (String, String) {
    let Some(v) = row.verdict else {
        return (String::new(), String::new());
    };
    let glyph = |x: Option<bool>| match x {
        Some(true) => "✓",
        Some(false) => "✗",
        None => "·",
    };
    let dev = |d: Option<f64>| match d {
        Some(d) => format!("{d:+.3} %"),
        None => "—".to_string(),
    };
    (
        format!("{}{}", glyph(v.entry), glyph(v.exit)),
        t!(
            "analytics.ticks.model_tip",
            entry = dev(v.entry_dev_pct),
            exit = dev(v.exit_dev_pct)
        )
        .to_string(),
    )
}

/// One deal row.
fn deal_row(
    row: &DealRow,
    p: MoonPalette,
    scale: f32,
    row_h: f32,
    zone: chrono_tz::Tz,
    cx: &App,
) -> AnyElement {
    let d = &row.deal;
    let result = rows::result_pct(row);
    let cell = |col: &DealCol, text: String, color: u32, tip: Option<String>| {
        let mut el = div()
            .w(px(col.w * scale))
            .min_w(px(col.min_w * scale))
            .flex_shrink_1()
            .flex_none()
            .truncate()
            .text_color(moon(color))
            .child(text);
        el = match col.align {
            Align::Right => el.text_right(),
            Align::Center => el.text_center(),
            Align::Left => el,
        };
        match tip {
            Some(tip) if !tip.is_empty() => el
                .id(SharedString::from(format!(
                    "an-ticks-{}-{}",
                    col.key, d.report_uid
                )))
                .tooltip(move |_w, cx| cx.new(|_| MoonTooltipView::new(tip.clone())).into())
                .into_any_element(),
            _ => el.into_any_element(),
        }
    };
    let (tape_glyph, tape_tip) = tape_mark(row.tape);
    let (model_glyph, model_tip) = model_mark(row);
    let text = p.text;
    let mut el = h_flex()
        .id(SharedString::from(format!("an-tickrow-{}", d.report_uid)))
        .w_full()
        .h(px(row_h))
        .px(design::ui_px(cx, DEAL_ROW_PAD_X))
        .gap(design::ui_px(cx, DEAL_ROW_GAP))
        .items_center()
        .bg(moon(p.table_body))
        .border_t_1()
        .border_color(moon_alpha(p.border, 0.5))
        .child(
            div()
                .flex_1()
                .min_w(design::font_w_px(cx, DEAL_COIN_MIN_W))
                .truncate()
                .child(d.coin.clone()),
        );
    for col in DEAL_COLS {
        let (value, color, tip) = match col.key {
            COL_TIME => (hms(d.buy_ms, zone), text, None),
            COL_BUY => (moon_core::util::fmt::adaptive(d.buy_price), text, None),
            COL_SELL => (moon_core::util::fmt::adaptive(d.sell_price), text, None),
            COL_RESULT => (
                format!("{result:+.2}"),
                if result > 0.0 {
                    p.green
                } else if result < 0.0 {
                    p.red
                } else {
                    p.text_muted
                },
                None,
            ),
            COL_DURATION => (duration_text(d.close_ms - d.buy_ms), p.text_muted, None),
            COL_D5S => (delta_text(d.deltas.d5s), p.text_muted, None),
            COL_D1M => (delta_text(d.deltas.d1m), p.text_muted, None),
            COL_D1H => (delta_text(d.deltas.d1h), p.text_muted, None),
            COL_DMARK => (delta_text(d.deltas.dmark), p.text_muted, None),
            COL_PRICEBUG => (delta_text(d.deltas.pricebug), p.text_muted, None),
            COL_REASON => (
                d.sell_reason.clone(),
                p.text_muted,
                Some(d.sell_reason.clone()),
            ),
            COL_TAPE => (
                tape_glyph.to_string(),
                match row.tape {
                    TapeStatus::Covered => p.green,
                    TapeStatus::Fetching => p.amber,
                    _ => p.text_muted,
                },
                Some(tape_tip.clone()),
            ),
            COL_MODEL => (
                model_glyph.clone(),
                match row.verdict.and_then(|v| v.entry.or(v.exit)) {
                    Some(true) => p.green,
                    Some(false) => p.red,
                    None => p.text_muted,
                },
                Some(model_tip.clone()),
            ),
            _ => (String::new(), text, None),
        };
        el = el.child(cell(col, value, color, tip));
    }
    el = el.hover(move |s| s.bg(moon_alpha(p.panel_high, 0.9)));
    el.into_any_element()
}
