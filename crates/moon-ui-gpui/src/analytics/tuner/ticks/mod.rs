//! The "Entry/Exit" axis of the strategy tuner: the scope's deals replayed over the trade
//! tape rather than masked by SQL.
//!
//! Left, under the strategy list: the deal table — one row per closed trade with millisecond
//! stamps, its market at the buy, why it closed, whether the terminal holds its tape, and
//! whether the model reproduces the fact. Right: the shared "Fact vs …" matrix (the whole
//! scope, the replayable subset captioned with the ✓ shares, the variant columns), and the
//! parameter grid with the strategies' values, the two variant columns and the search row.
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
use super::shared::TunerKind;
use super::shell::CfgInput;
use super::{sort_arrow_of, toggle_sort_key};
use crate::design;
use crate::design::{moon, moon_alpha};
use columns::*;
pub(in crate::analytics::tuner) use fetch::strategy_field_defaults;
use state::SuggState;
use state::{DealRow, TapeStatus};

pub(in crate::analytics::tuner) mod columns;
pub(crate) mod fetch;
mod grid;
mod load;
pub(in crate::analytics::tuner) mod rows;
pub(in crate::analytics) mod state;
mod variants;

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
        // The rows the scope holds but the table does not show, for the caption and for the
        // empty state: a period entirely before the millisecond stamps is not an empty period,
        // and the table must say which it is rather than draw the shared "no trades".
        let left_out = self
            .ticks
            .data
            .data()
            .map(|d| (d.without_ms, d.service, d.untunable))
            .unwrap_or_default();
        let summary = self.ticks.data.view(|d| d.rows.is_empty()).map(|d| {
            (
                d.rows.len(),
                d.covered(),
                d.fetchable().count(),
                d.without_ms,
            )
        });
        let (body, total, covered, fetchable, without_ms) = match summary {
            Err(crate::load_state::Note::Empty) if left_out != (0, 0, 0) => (
                crate::load_state::muted(
                    t!(
                        "analytics.ticks.empty_left_out",
                        without = left_out.0,
                        service = left_out.1,
                        untunable = left_out.2
                    )
                    .to_string(),
                    10.0,
                    p,
                    cx,
                ),
                0usize,
                0usize,
                0usize,
                left_out.0,
            ),
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
        // The batch is the process's (`fetch::job`), not this window's: the caption reads its
        // progress, and says what it is doing right now, not only how far it is — one walk can
        // take minutes on a slow venue, and a batch asleep on a venue's backoff has nothing in
        // flight at all; a bare "N/M" reads as stuck in both cases.
        let progress = fetch::job::progress();
        let fetch_active = progress.active;
        // Rows of THIS table a running batch does not have — the autoload's, or one left by a
        // previous window: a second button adds them, while the first stays the stop.
        let addable = self.ticks_fetch_addable();
        // A batch this window did not start — the startup autoload, or one left by a previous
        // window — is listened to from the first paint that finds it running, so its answers
        // land in the table and the button keeps counting. Idempotent: one listener per view
        // while a batch runs.
        if fetch_active {
            self.attach_fetch_listener(cx);
        }
        let fetch_label = if !fetch_active && self.ticks.tape_reading {
            t!("analytics.ticks.fetch_reading").to_string()
        } else if !fetch_active {
            t!("analytics.ticks.fetch_btn").to_string()
        } else if !progress.in_flight.is_empty() {
            // Every market a request is out for, in the order they went out: the walks run in
            // parallel across venues, and one name would read as one request.
            let markets: Vec<&str> = progress
                .in_flight
                .iter()
                .map(|(_, market)| market.as_str())
                .collect();
            t!(
                "analytics.ticks.fetch_progress_at",
                done = progress.done,
                total = progress.total,
                market = markets.join(" · ")
            )
            .to_string()
        } else {
            t!(
                "analytics.ticks.fetch_waiting",
                done = progress.done,
                total = progress.total
            )
            .to_string()
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
                    // "N with tape of M · K without stamps": the honest size of the sample,
                    // with the service rows and the switch's leftovers when there are any.
                    .child(
                        div()
                            .flex_none()
                            .font_family(design::ui_font())
                            .text_size(design::t_caption(cx))
                            .text_color(moon(p.text_muted))
                            .child(coverage_caption(
                                covered, total, without_ms, left_out.1, left_out.2,
                            )),
                    )
                    .when(fetch_active && addable > 0, |el| {
                        el.child(
                            div().font_family(design::ui_font()).child(
                                MoonButton::new("an-ticks-fetch-add")
                                    .variant(MoonButtonVariant::Soft)
                                    .label(t!("analytics.ticks.fetch_add", n = addable).to_string())
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.ticks_fetch_missing(cx);
                                        cx.notify();
                                    }))
                                    .render(),
                            ),
                        )
                    })
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

    /// The right column of the axis: the matrix on top, the grid panel below it.
    pub(in crate::analytics::tuner) fn ticks_side(
        &mut self,
        p: MoonPalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let kpi = self.ticks_kpi(p, cx);
        let grid = self.ticks_grid(p, window, cx);
        v_flex()
            .w_full()
            .h_full()
            .min_h_0()
            .gap(design::ui_px(cx, 8.0))
            .child(kpi)
            .child(grid)
            .into_any_element()
    }

    /// "Fact vs …": the whole scope, the rows the tape covers (captioned with the ✓ shares of
    /// both groups — the model's own account of itself), then the variant columns, each over
    /// the replayable rows and captioned with how many.
    fn ticks_kpi(&self, p: MoonPalette, cx: &Context<Self>) -> AnyElement {
        let (covered, total, entry, exit, replayable) = self
            .ticks
            .data
            .data()
            .map(|d| {
                (
                    d.covered(),
                    d.rows.len(),
                    d.entry_share,
                    d.exit_share,
                    d.replayable().count(),
                )
            })
            .unwrap_or_default();
        let share = |(hits, n): (usize, usize)| -> String {
            if n == 0 {
                "—".to_string()
            } else {
                format!("{:.0} %", hits as f64 / n as f64 * 100.0)
            }
        };
        let mut labels = vec![VarLabel::with_sub(
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
        // The matrix reads one vector: `[fact, subset]` from the load, then the variants that
        // were scored. An untouched variant is not a column.
        let mut stats: Vec<moon_core::db::tuner::VarStats> = self
            .ticks
            .kpi
            .data()
            .map(|k| k.to_vec())
            .unwrap_or_default();
        for (i, var) in self.ticks.var_stats.iter().enumerate() {
            let Some(var) = var else {
                continue;
            };
            let mut sub = t!(
                "analytics.ticks.var_sub",
                n = var.n,
                m = self.ticks.var_n.max(replayable)
            )
            .to_string();
            if i == 0 {
                if let Some(holdout) = self
                    .ticks
                    .last_result
                    .as_ref()
                    .and_then(|r| r.holdout.as_ref())
                {
                    sub = format!(
                        "{sub} · {}",
                        t!(
                            "analytics.ticks.holdout",
                            n = holdout.n,
                            profit = super::super::summary::fmt_signed(holdout.profit)
                        )
                    );
                }
            }
            labels.push(VarLabel::with_sub(
                t!("analytics.ticks.var_n", n = i + 1).to_string(),
                sub,
            ));
            stats.push(var.clone());
        }
        let state = match &self.ticks.kpi {
            crate::load_state::LoadState::Ready(_) => {
                crate::load_state::LoadState::Ready(std::sync::Arc::new(stats))
            }
            crate::load_state::LoadState::Loading { stale } => {
                crate::load_state::LoadState::Loading {
                    stale: stale.as_ref().map(|_| std::sync::Arc::new(stats)),
                }
            }
            crate::load_state::LoadState::NotReady => crate::load_state::LoadState::NotReady,
            crate::load_state::LoadState::Failed(e) => {
                crate::load_state::LoadState::Failed(e.clone())
            }
        };
        kpi_matrix_card(
            &state,
            self.scope_label(),
            &labels,
            self.kpi_collapsed,
            p,
            cx,
        )
    }

    /// The search row of the axis: restarts, minimum trades, the train share, the status,
    /// Stop and "Search".
    pub(in crate::analytics::tuner) fn ticks_config_row(
        &mut self,
        p: MoonPalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let running = matches!(self.ticks.sugg, SuggState::Running { .. });
        let (status, status_color) = match &self.ticks.sugg {
            // Counted against the restarts the run was launched with, not the box's current
            // text.
            SuggState::Running { handle, total } => (
                t!(
                    "analytics.tuner.sugg_progress",
                    done = handle.completed(),
                    total = total
                )
                .to_string(),
                p.text_soft,
            ),
            SuggState::Idle => match &self.ticks.sugg_note {
                Some(note) => (note.clone(), p.amber),
                None => (String::new(), p.text_muted),
            },
        };
        let it_placeholder = variants::DEFAULT_RESTARTS.to_string();
        let it_input = self.shell_cfg_input(
            TunerKind::Ticks,
            CfgInput::Restarts,
            &it_placeholder,
            window,
            cx,
        );
        let mn_input =
            self.shell_cfg_input(TunerKind::Ticks, CfgInput::MinTrades, "auto", window, cx);
        let train_pct = self.ticks.train_pct;
        let tr_view = cx.entity();
        let tr_items = crate::panels::radio_items(
            super::filter::state::TRAIN_OPTIONS.map(|n| {
                (
                    n,
                    SharedString::from(format!("tun-tr-x-{n}")),
                    SharedString::from(super::shell::train_label(n)),
                )
            }),
            train_pct,
            crate::panels::RadioMark::Highlight,
            move |app, n| {
                tr_view.update(app, |this, cx| {
                    this.ticks.train_pct = n;
                    cx.notify();
                });
            },
        );
        let tr_combo = moon_ui::MoonDropdown::new(SharedString::from("tun-cfg-tr-x"))
            .label(super::shell::train_label(train_pct))
            .trigger_caret(true)
            .trigger_variant(MoonButtonVariant::Soft)
            .trigger_size(moon_ui::MoonButtonSize::density(cx))
            .menu_width_scaled(96.0)
            .items(tr_items);
        let input_box = |id: &'static str, state: &Entity<moon_ui::MoonInputState>, w: f32| {
            div()
                .w(design::font_w_px(cx, w))
                .flex_none()
                .font_family(design::mono())
                .child(
                    moon_ui::MoonInput::new(SharedString::from(id))
                        .state(state)
                        .size(design::INPUT_SIZE),
                )
        };
        h_flex()
            .w_full()
            .flex_none()
            .px(design::ui_px(cx, 12.0))
            .pb(design::ui_px(cx, 6.0))
            .items_center()
            .gap(design::ui_px(cx, 6.0))
            .text_size(design::t_caption(cx))
            .font_family(design::ui_font())
            .child(
                div()
                    .text_color(moon(p.text_muted))
                    .child(t!("analytics.tuner.iters").to_string()),
            )
            .child(input_box("tun-cfg-it-x", &it_input, 46.0))
            .child(
                div()
                    .text_color(moon(p.text_muted))
                    .child(t!("analytics.tuner.min_trades").to_string()),
            )
            .child(input_box("tun-cfg-mn-x", &mn_input, 46.0))
            .child(tr_combo)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(moon(status_color))
                    .child(status),
            )
            .when(running, |el| {
                el.child(
                    MoonButton::new("tun-suggest-stop-x")
                        .variant(MoonButtonVariant::Soft)
                        .label(t!("analytics.tuner.stop").to_string())
                        .on_click(cx.listener(|this, _, _, cx| this.ticks_stop_suggest(cx)))
                        .render(),
                )
            })
            .child(
                MoonButton::new("tun-suggest-run-x")
                    .variant(MoonButtonVariant::Blue)
                    .label(t!("analytics.tuner.suggest_run").to_string())
                    .disabled(running)
                    .on_click(cx.listener(|this, _, _, cx| this.ticks_suggest(cx)))
                    .render(),
            )
            .into_any_element()
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

/// The tape dot of a row: filled in the state's colour, hollow while the tape is missing —
/// the same distinction the column's ●/○ draws, readable at any table width.
fn tape_dot(
    tape: TapeStatus,
    tip: String,
    report_uid: i64,
    p: MoonPalette,
    scale: f32,
) -> AnyElement {
    let (color, filled) = match tape {
        TapeStatus::Covered => (p.green, true),
        TapeStatus::Fetching => (p.amber, true),
        TapeStatus::Refused(_) => (p.red, true),
        TapeStatus::NoAddress => (p.text_muted, true),
        TapeStatus::Missing => (p.text_muted, false),
    };
    let size = px(TAPE_DOT_PX * scale);
    let dot = div().size(size).rounded_full().flex_none();
    let dot = if filled {
        dot.bg(moon(color))
    } else {
        dot.border_1().border_color(moon(color))
    };
    div()
        .id(SharedString::from(format!("an-ticks-dot-{report_uid}")))
        .flex_none()
        .child(dot)
        .tooltip(move |_w, cx| cx.new(|_| MoonTooltipView::new(tip.clone())).into())
        .into_any_element()
}

/// Diameter of the row's tape dot, in base px, before the font scale.
const TAPE_DOT_PX: f32 = 7.0;

/// The header caption of the table: how many rows have their tape, out of how many, and what
/// the scope holds beyond the table — rows without millisecond stamps always, the service rows
/// and the untunable ones only when there are any.
fn coverage_caption(
    covered: usize,
    total: usize,
    without_ms: usize,
    service: usize,
    untunable: usize,
) -> String {
    let mut caption = t!(
        "analytics.ticks.coverage",
        covered = covered,
        total = total,
        without = without_ms
    )
    .to_string();
    if service > 0 {
        caption.push_str(" · ");
        caption.push_str(&t!("analytics.ticks.coverage_service", n = service));
    }
    if untunable > 0 {
        caption.push_str(" · ");
        caption.push_str(&t!("analytics.ticks.coverage_untunable", n = untunable));
    }
    caption
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
        // The tape's state as a dot at the LEFT edge, before the coin: the tape column sits
        // last and is the first thing a narrow table cuts off, and whether a row has its tape
        // is the one thing about it this axis is for. Same tooltip as the column's mark.
        .child(tape_dot(row.tape, tape_tip.clone(), d.report_uid, p, scale))
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
