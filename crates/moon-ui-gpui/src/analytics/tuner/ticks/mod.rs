//! The "Entry/Exit" axis of the strategy tuner: the scope's deals replayed over the trade
//! tape rather than masked by SQL.
//!
//! Left, under the strategy list: the deal table — one row per closed trade with millisecond
//! stamps, what came of it, whether the terminal holds its tape, and whether the model
//! reproduces the fact; a double-click opens the trade window on it. Every row of the scope is
//! shown; the switch under the table narrows it to the sample the variants and the search run
//! on — the rows whose tape covers the window AND which the model reproduces (`DealRow::fit`) —
//! and the status line says how many that is out of the scope; the table's head opens the model's
//! settings (`cfg.rs`), which decide its ✓ column. Under the table, behind a rail, the trade pane
//! (`trade_pane.rs`): the selected deal as its trade window draws it, with the variants' trades
//! beside the fact. Right: the shared KPI matrix over the fit rows
//! (captioned with the ✓ shares) and the two variant columns, and the parameter grid laid out as
//! the "By filter" one — the strategies' values, the variant columns, the search row with its
//! settings popover.
//!
//! The model itself is `moon_core::db::tuner::ticks`; this module only feeds it and draws
//! what it says.

use std::time::Duration;

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    MoonButton, MoonButtonVariant, MoonCheckbox, MoonPalette, MoonScrollbarVisibility,
    MoonVirtualList, h_flex, v_flex,
};
use rust_i18n::t;

use super::super::AnalyticsView;
use super::kpi::{KpiOpts, KpiRows, VarLabel, kpi_matrix_card_with};
use super::{sort_arrow_of, toggle_sort_key};
use crate::design;
use crate::design::{moon, moon_alpha};
use columns::*;
pub(in crate::analytics::tuner) use fetch::strategy_field_defaults;
pub(in crate::analytics::tuner) use load::{Fetched, PassReads, fetch_batch, replay_fetched};
// The readiness bench times the reads and the model apart and runs the auto-pick itself.
#[cfg(test)]
pub(in crate::analytics::tuner) use load::{archived_lines, calibrate_lags, replay_timing};
use state::DealRow;
pub(in crate::analytics::tuner) use summary::readiness_refusal;
#[cfg(test)]
pub(in crate::analytics::tuner) use tape::{PackedTape, PendingDeal, prepare_sample};

mod accuracy;
/// Sequential searches and checked bulk consent for the strategy list's best picks.
pub(in crate::analytics::tuner) mod best;
mod cfg;
pub(in crate::analytics::tuner) mod columns;
mod delta_summary;
pub(in crate::analytics::tuner) mod estimate;
pub(crate) mod fetch;
mod grid;
mod lags;
mod layout;
mod load;
pub(in crate::analytics) mod model_cfg;
pub(in crate::analytics::tuner) mod ranges;
pub(in crate::analytics::tuner) mod readiness;
mod refiled;
mod report;
mod risk;
pub(in crate::analytics::tuner) mod rows;
mod sample;
mod sections;
pub(in crate::analytics) mod state;
mod summary;
pub(in crate::analytics) mod tail;
mod tape;
mod trade_pane;
mod unmodelled;
mod variants;
pub(in crate::analytics::tuner) mod why;

#[cfg(test)]
mod tests;

impl AnalyticsView {
    /// The deal table card — sits UNDER the strategy list, where the coin table sits in "By
    /// coin".
    pub(in crate::analytics::tuner) fn ticks_card(
        &mut self,
        p: MoonPalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.refresh_station_tape(cx);
        let scale = design::font_scale(cx);
        let scope = self.scope_label();
        // The order is settled before the data is viewed: both live in `ticks`, and the sort
        // cache needs the mutable half. It is the SHOWN rows: with the switch on, only the
        // sample the variants run on.
        let now_ms = chrono::Utc::now().timestamp_millis();
        let min_tail_s = tail::effective_s();
        let admission = tail::Admission::at(min_tail_s, now_ms);
        let drawn = rows::order_for(&mut self.ticks, min_tail_s, now_ms).len();
        let counts = rows::counts_at(&mut self.ticks, min_tail_s, now_ms);
        let only_fit = self.ticks.only_fit;
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
                if fetch::station_available() {
                    d.rows
                        .iter()
                        .filter(|row| fetch::station_load_candidate(row, &admission))
                        .count()
                } else {
                    d.fetchable_at(now_ms, min_tail_s).count()
                },
            )
        });
        let (body, fetchable) = match summary {
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
            ),
            Err(note) => (
                super::super::note_el("an-ticks-note", note, 10.0, p, cx),
                0usize,
            ),
            // Rows loaded, none shown: the switch hid every one. Said in words, with the
            // count, rather than drawn as a blank list — while the tape stage still reads,
            // no row is covered yet and the note says that instead.
            Ok((total, fetchable)) if drawn == 0 => (
                if self.ticks.chip.is_some() {
                    filtered_empty_el(chip_action(cx.entity().downgrade()), p, cx)
                } else {
                    crate::load_state::muted(
                        if self.ticks.tape_reading {
                            t!("analytics.ticks.fetch_reading").to_string()
                        } else {
                            t!("analytics.ticks.none_fit", hidden = total).to_string()
                        },
                        10.0,
                        p,
                        cx,
                    )
                },
                fetchable,
            ),
            Ok((_, fetchable)) => {
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
                                let selected = view.ticks.trade.open
                                    && view.ticks.trade.uid == Some(row.deal.report_uid);
                                let plan = PlanCell::of(&view.ticks, row.deal.report_uid);
                                Some(deal_row(
                                    row,
                                    selected,
                                    plan,
                                    deal_action(weak.clone()),
                                    p,
                                    scale,
                                    row_h,
                                    (min_tail_s, now_ms),
                                    app,
                                ))
                            })
                            .unwrap_or_else(|| div().into_any_element())
                    })
                    .surface(false)
                    .border(false)
                    .radius(0.0)
                    .scrollbar_visibility(MoonScrollbarVisibility::Hover)
                    .into_any_element();
                (list, fetchable)
            }
        };
        // The batch is the process's (`fetch::job`), not this window's: the caption reads its
        // progress, and says what it is doing right now, not only how far it is — one walk can
        // take minutes on a slow venue, and a batch asleep on a venue's backoff has nothing in
        // flight at all. That pause names how long is left until the soonest host may be asked
        // again, read here so the next repaint shows less of it. No timer: this window's render
        // root schedules none (`theme_contract`).
        let progress = fetch::job::progress();
        let fetch_active = progress.active || fetch::station_active();
        // A batch this window did not start — the startup autoload, or one left by a previous
        // window — is listened to from the first paint that finds it running, so its answers
        // land in the table and the button keeps counting. Idempotent: one listener per view
        // while a batch runs.
        if fetch_active {
            self.attach_fetch_listener(cx);
        }
        // The button is only the switch — "fetch" or "stop"; what the batch is doing goes into
        // the caption beside it, and ONLY that: the sample's coverage sits in the status line
        // under the table, so the head says nothing when nothing runs.
        let caption = if fetch_active && !progress.in_flight.is_empty() {
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
        } else if fetch_active {
            match progress.venue_wait {
                // The venue asked for this pause. The figure is the same deadline the next
                // try will honour, the soonest host when several are waiting.
                Some(wait) => t!(
                    "analytics.ticks.fetch_venue_wait",
                    done = progress.done,
                    total = progress.total,
                    left = fetch_wait_text(wait)
                )
                .to_string(),
                None => t!(
                    "analytics.ticks.fetch_waiting",
                    done = progress.done,
                    total = progress.total
                )
                .to_string(),
            }
        } else if self.ticks.tape_reading {
            t!("analytics.ticks.fetch_reading").to_string()
        } else {
            String::new()
        };
        // The status line: the honest size of the sample — how many rows have their tape,
        // out of how many — with what the scope holds beyond the table, and the switch that
        // hides the rest. How well the MODEL does on that sample is the KPI caption's ✓
        // shares (`ticks_kpi`), not a size.
        let chips = chips_el(
            counts,
            self.ticks.chip,
            left_out,
            chip_action(cx.entity().downgrade()),
            p,
            cx,
        );
        // How many rows read live deltas, and — in the tooltip — how well each one's history
        // reproduces the core (`delta_summary`).
        let deltas = self
            .ticks
            .data
            .data()
            .and_then(|d| delta_summary::delta_summary(&d.rows))
            .map(|(caption, tip)| {
                div()
                    .id("an-ticks-deltas")
                    .flex_none()
                    .tooltip(crate::panels::common::text_tooltip(tip))
                    .child(caption)
            });
        let only_tip = t!("analytics.ticks.only_fit_tip").to_string();
        let only_switch = div()
            .id("an-ticks-only-tape-box")
            .flex_none()
            .tooltip(crate::panels::common::text_tooltip(only_tip))
            .child(
                MoonCheckbox::new("an-ticks-only-tape")
                    .label(t!("analytics.ticks.only_fit").to_string())
                    .checked(only_fit)
                    .on_change({
                        let view = cx.entity();
                        move |on: &bool, _w, app| {
                            let on = *on;
                            view.update(app, |this, cx| {
                                this.ticks.only_fit = on;
                                // The cached order is a permutation of the SHOWN rows.
                                this.ticks.order = None;
                                cx.notify();
                            });
                        }
                    }),
            );
        let fetch_label = if fetch_active {
            t!("analytics.ticks.fetch_stop").to_string()
        } else {
            t!("analytics.ticks.fetch_btn").to_string()
        };
        let model_settings = self.ticks_model_settings(p, window, cx);
        let report = self.ticks_report_button(cx);
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
                    // While a batch runs: how far it is and which markets it is on; while the
                    // tape stage reads: that. Nothing otherwise — the status line has the rest.
                    .when(!caption.is_empty(), |el| {
                        el.child(
                            div()
                                .flex_none()
                                .font_family(design::ui_font())
                                .text_size(design::t_caption(cx))
                                .text_color(moon(p.text_muted))
                                .child(caption),
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
                                    // While the tape stage reads, a press would queue rows the
                                    // tiles already hold (`ticks_fetch_missing` waits for the
                                    // fold), so the switch waits too.
                                    .disabled(!fetch_active && self.ticks.tape_reading)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        if fetch_active {
                                            this.ticks_fetch_stop(cx);
                                        } else {
                                            this.ticks_fetch_missing(
                                                fetch::job::RowOrigin::User,
                                                cx,
                                            );
                                        }
                                        cx.notify();
                                    }))
                                    .render(),
                            ),
                        )
                    })
                    .when_some(report, |el, report| {
                        el.child(
                            div()
                                .flex_none()
                                .font_family(design::ui_font())
                                .child(report),
                        )
                    })
                    .child(
                        div()
                            .flex_none()
                            .font_family(design::ui_font())
                            .child(model_settings),
                    ),
            )
            .child(legend_el(p, cx))
            .child(self.deal_header(p, cx))
            // The virtual list owns its own scrolling.
            .child(div().w_full().flex_1().min_h_0().child(body))
            .child(
                h_flex()
                    .w_full()
                    .flex_none()
                    .flex_wrap()
                    .py(design::ui_px(cx, 5.0))
                    .px(design::ui_px(cx, DEAL_ROW_PAD_X))
                    .items_center()
                    .gap(design::ui_px(cx, 8.0))
                    .border_t_1()
                    .border_color(moon(p.border))
                    .bg(moon(p.table_head))
                    .font_family(design::ui_font())
                    .text_size(design::t_caption(cx))
                    .text_color(moon(p.text_muted))
                    .child(chips)
                    .children(deltas)
                    .child(only_switch),
            )
            .into_any_element()
    }

    /// Open the trade window on one deal of the table — the same opener a Report row uses, so
    /// the two lists cannot disagree about what a trade is.
    ///
    /// Silent when the row has no address (its core is offline, or the coin resolves to no
    /// market): the row's tape mark already says so, and a double-click has nowhere to put a
    /// reason.
    ///
    /// Args:
    ///     report_uid: The row's `ReportUID` — the core's own key for the trade, not the replica's row id.
    ///     cx: View context.
    fn open_deal_window(&mut self, report_uid: i64, cx: &mut Context<Self>) {
        let Some((target, axis)) = self.deal_target(report_uid) else {
            return;
        };
        crate::trade_window::open_record::open_trade_record(&self.backend, axis, target, cx);
    }

    /// The replica row one deal of the table names, with the axis its captions render on — for
    /// the trade window and for the trade pane alike. `None` when the row is not in the table or
    /// has no address.
    fn deal_target(
        &self,
        report_uid: i64,
    ) -> Option<(
        crate::trade_window::open_record::RecordTarget,
        moon_core::db::ReportAxis,
    )> {
        let row = self
            .ticks
            .data
            .data()
            .and_then(|d| d.rows.iter().find(|r| r.deal.report_uid == report_uid))?;
        let address = row.address.as_ref()?;
        let q = self.query();
        // The window's neighbours are the core's other trades of this coin over the axis's
        // period — the Query's bounds are true UTC and `to` is exclusive, as the filter's
        // `date_to` is inclusive; one second either way on the neighbours is not a trade lost.
        let filter = moon_core::db::ReportFilter {
            core_uids: vec![row.deal.core_uid],
            date_from: (q.from >= 0).then_some(q.from),
            date_to: Some(q.to),
            axis: q.axis.clone(),
            ..moon_core::db::ReportFilter::default()
        };
        // By the core's ReportUID, not the replica's row id: the two are different counters, and
        // `reportuid` is the one the deal table is keyed by.
        let target = crate::trade_window::open_record::RecordTarget {
            core: row.deal.core_uid,
            coin: row.deal.coin.clone(),
            record: crate::trade_window::open_record::RecordKey::ReportUid(report_uid),
            market: address.market.clone(),
            filter,
        };
        Some((target, q.axis))
    }

    /// The table's heading row: every column sortable, the arrow on the active one. Each
    /// heading sits in the same box as the column's cells ([`deal_cell`], [`coin_cell`]), so
    /// the two rows share every width, minimum and shrink rule and cannot drift apart.
    fn deal_header(&self, p: MoonPalette, cx: &Context<Self>) -> impl IntoElement + use<> {
        let view = cx.entity().downgrade();
        deal_header_el(
            &self.ticks.sort,
            self.ticks.var_stats.is_some(),
            std::rc::Rc::new(move |key, _, app| {
                let _ = view.update(app, |this, cx| {
                    toggle_sort_key(&mut this.ticks.sort, key);
                    cx.notify();
                });
            }),
            p,
            cx,
        )
    }

    /// The right column of the axis: the matrix on top, the grid panel below it.
    pub(in crate::analytics::tuner) fn ticks_side(
        &mut self,
        p: MoonPalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let host_w = design::font_w_px(cx, 470.0);
        let summary = self.ticks_summary_card(host_w, p, cx);
        let kpi = self.ticks_kpi(p, cx);
        let grid = self.ticks_grid(host_w, p, window, cx);
        v_flex()
            .id("an-ticks-side")
            .w_full()
            .h_full()
            .min_h_0()
            .overflow_y_scroll()
            .gap(design::ui_px(cx, 8.0))
            .child(summary)
            .child(kpi)
            .child(grid)
            .into_any_element()
    }

    /// Compare the displayed V1 with the fit-trade fact, keeping open estimates outside profit.
    fn ticks_kpi(&self, p: MoonPalette, cx: &Context<Self>) -> AnyElement {
        let (fit, covered, entry, exit, replayable, entry_on) = self
            .ticks
            .data
            .data()
            .map(|d| {
                (
                    d.fit(),
                    d.covered(),
                    d.entry_share,
                    d.exit_share,
                    d.replayable().count(),
                    d.entry_modelled(),
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
        // Each replay uses its deal's own held trail, as the columns and search do.
        let mut subset_sub = t!(
            "analytics.ticks.subset_sub",
            n = fit,
            m = covered,
            // A kind without an entry model reproduces no entry: its share would read 0 %.
            entry = if entry_on {
                share(entry)
            } else {
                "—".to_string()
            },
            exit = share(exit)
        )
        .to_string();
        if replayable > 0 {
            subset_sub.push_str(&t!("analytics.ticks.horizon"));
        }
        // One short title. The share line is a hover: inside a 92px column it wrapped and
        // truncated ("2 of 2 with tape…").
        // Once a variant is scored the "Fact" is over its very deals (`variants.rs`); the counts
        // of what sits out of every column are on screen, not in a tooltip.
        let set_n = match self.ticks.set_fact {
            Some(_) => self.ticks.var_n,
            None => replayable,
        };
        let no_tape = fit.saturating_sub(replayable);
        // Until the cut has run nobody knows how many the base leaves open: no "0" is claimed.
        let set_counts = match self.ticks.set_fact {
            Some(_) => t!(
                "analytics.ticks.set_counts",
                n = set_n,
                base_open = self.ticks.base_open,
                no_tape = no_tape
            ),
            None => t!(
                "analytics.ticks.set_counts_pending",
                n = set_n,
                no_tape = no_tape
            ),
        }
        .to_string();
        // A ~92px column: the visible line carries a short token, the tooltip the whole text.
        let base = VarLabel::new(t!("analytics.tuner.fact").to_string())
            .with_tip(format!("{subset_sub}\n{set_counts}"))
            .sub_line(t!("analytics.ticks.kpi.fact_sub").to_string(), false);
        let baseline: Option<moon_core::db::tuner::VarStats> = self
            .ticks
            .set_fact
            .clone()
            .or_else(|| self.ticks.kpi.data().and_then(|k| k.first().cloned()));
        let mut stats: Vec<moon_core::db::tuner::VarStats> = baseline.iter().cloned().collect();
        let title = t!("analytics.ticks.sum.col_v1").to_string();
        let label = match &self.ticks.var_stats {
            None => VarLabel::with_sub(title, t!("analytics.ticks.kpi.v1_none").to_string())
                .with_tip(t!("analytics.ticks.var_untouched").to_string())
                .placeholder(),
            Some(var) => {
                let sub =
                    t!("analytics.ticks.var_sub", n = var.n, m = self.ticks.var_n).to_string();
                stats.push(var.clone());
                let label = VarLabel::with_sub(title, t!("analytics.ticks.kpi.v1_sub").to_string())
                    .with_tip(sub);
                if self.ticks.variant_too_few().is_some() {
                    label.muted()
                } else {
                    label
                }
            }
        };
        let labels = [label];
        let state = match &self.ticks.kpi {
            crate::load_state::LoadState::NotReady => crate::load_state::LoadState::NotReady,
            crate::load_state::LoadState::Failed(e) => {
                crate::load_state::LoadState::Failed(e.clone())
            }
            // The matrix reads column 0 in every row: without a baseline there is nothing to
            // draw yet, whatever else the load state says. Nor while the first tape stage of a
            // table judges it and no row is fit yet: the baseline would read as a real
            // "0 trades". A reload carries the judged rows (`load.rs`, stage B), so the numbers
            // stay on screen while it reads the rest.
            _ if stats.is_empty() || (self.ticks.tape_reading && fit == 0) => {
                crate::load_state::LoadState::Loading { stale: None }
            }
            crate::load_state::LoadState::Ready(_) => {
                crate::load_state::LoadState::Ready(std::sync::Arc::new(stats))
            }
            crate::load_state::LoadState::Loading { stale } => {
                crate::load_state::LoadState::Loading {
                    stale: stale.as_ref().map(|_| std::sync::Arc::new(stats)),
                }
            }
        };
        kpi_matrix_card_with(
            &state,
            self.scope_label(),
            &base,
            &labels,
            self.kpi_collapsed,
            p,
            cx,
            KpiOpts {
                title: Some((
                    t!("analytics.ticks.kpi.title").to_string(),
                    sample::caption(set_n, self.ticks.data.data().map_or(0, |d| d.all.n)),
                )),
                sample_note: self.ticks.data.data().and_then(|d| {
                    baseline
                        .as_ref()
                        .and_then(|fit| sample::bias_note(fit, &d.all))
                }),
                rows: KpiRows::Plain,
                unclosed: (self.ticks.var_open > 0)
                    .then_some((self.ticks.var_open, self.ticks.var_open_profit)),
            },
        )
    }
}

/// The heading of one column. The two money columns name their unit — the cells are bare
/// numbers: `Deal::profit` is USDT whatever the scope's own quote or metric (the ticker is
/// language-neutral, see locales/README.md); the plan follows the active metric — per cent in
/// percent mode, else the sample's money under its ticker (`PlanCell::render` prints the same
/// choice).
fn column_title(col: &DealCol) -> String {
    let title = t!(col.label).to_string();
    match col.key {
        COL_PROFIT => format!("{title}, USDT"),
        COL_PLAN => match crate::analytics::pnl_unit_label() {
            "" => title,
            unit => format!("{title}, {unit}"),
        },
        _ => title,
    }
}

/// What the plan column shows for one deal: the variant's `(money, per cent)` where it was
/// scored, `None` in the outer slot while the variant is not scored at all.
#[derive(Clone, Copy)]
struct PlanCell(Option<Option<(f64, f64)>>);

impl PlanCell {
    /// The deal's plan under the variant, from the state the column was scored into.
    fn of(state: &state::TicksState, uid: i64) -> Self {
        Self(
            state
                .var_stats
                .as_ref()
                .map(|_| state.plan.get(&uid).copied()),
        )
    }

    /// The cell's text, colour and tooltip: В1's result — per cent in percent mode, the
    /// sample's money otherwise, as the heading says — a dash where В1 makes no trade of the
    /// deal, nothing while В1 is untouched.
    fn render(self, p: MoonPalette) -> (String, u32, Option<String>) {
        let pct = crate::analytics::pnl_is_pct();
        let pick = |(money, percent): (f64, f64)| if pct { percent } else { money };
        let money = |value: Option<(f64, f64)>| match value {
            Some(v) => super::super::summary::fmt_signed(pick(v)),
            None => "—".to_string(),
        };
        let tip = || {
            self.0.map(|v| {
                format!(
                    "{} {} · {}",
                    t!("analytics.ticks.var_n", n = 1),
                    money(v),
                    t!("analytics.ticks.plan_tip")
                )
            })
        };
        match self.0 {
            None => (String::new(), p.text_muted, tip()),
            Some(None) => ("—".to_string(), p.text_muted, tip()),
            Some(Some(v)) => (
                money(Some(v)),
                super::super::summary::sign_color(p, pick(v)),
                tip(),
            ),
        }
    }
}

/// Height of one deal row, in base px — the single pitch the list and the row share.
fn deal_row_h(cx: &App) -> f32 {
    design::fit_h_value(cx, 24.0, 14.0, 5.0)
}

/// The remaining venue pause as the fetch caption prints it.
///
/// Short waits stay in seconds through 90s, so a 30s backoff still counts in seconds. Past
/// that the figure is the search estimate's ([`estimate::duration_text`]): minutes, then
/// hours and minutes. A fraction of a second rounds up, so the caption does not claim the
/// try is due before the deadline the dispatcher will honour.
///
/// Args:
///     wait: Time until the soonest waiting host's deadline.
///
/// Returns:
///     Localized figure. Zero is "0 s", not the estimate helper's one-second floor.
fn fetch_wait_text(wait: Duration) -> String {
    let secs = wait.as_secs() + u64::from(wait.subsec_nanos() != 0);
    if secs <= 90 {
        t!("analytics.ticks.dur_s", s = secs).to_string()
    } else {
        estimate::duration_text(Duration::from_secs(secs))
    }
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

/// How long the entry order stood in the book before it filled: from its placement
/// (`Deal::buy_set_ms`, filed by newer cores only) to the entry fill. `None` where the row
/// carries no placement or the two stamps do not give a positive wait.
fn order_wait_ms(buy_ms: i64, buy_set_ms: Option<i64>) -> Option<i64> {
    buy_set_ms
        .filter(|set| *set > 0)
        .map(|set| buy_ms - set)
        .filter(|wait| *wait > 0)
}

/// The duration cell's tooltip: what the column measures, and the order's wait before the
/// fill where the core filed its placement.
fn duration_tip(buy_ms: i64, buy_set_ms: Option<i64>) -> String {
    let what = t!("analytics.ticks.duration_tip").to_string();
    match order_wait_ms(buy_ms, buy_set_ms) {
        Some(wait) => format!(
            "{what} · {}",
            t!("analytics.ticks.order_wait_tip", wait = duration_text(wait))
        ),
        None => what,
    }
}

/// The tape the terminal holds around a trade, as "lead/trail" — what lies before the entry
/// and past the exit; a dash when nothing is held.
fn held_text(held: Option<(i64, i64)>) -> String {
    match held {
        Some((lead, trail)) => format!("{}/{}", duration_text(lead), duration_text(trail)),
        None => "—".to_string(),
    }
}

/// The box of one column's cell — the heading's and the row's alike: the descriptor's width
/// and minimum, shrinking with the table past its preferred width, the text cut rather than
/// wrapped, aligned as the column says. One builder for both rows is what keeps the headings
/// over their cells; the two used to set the shrink rule differently and drifted apart on a
/// narrow table.
fn deal_cell(col: &DealCol, scale: f32) -> Div {
    // One fill column, and only where every other column already has an explicit width.
    // A fill also overrides a persisted user width; this table does not persist widths.
    let el = if col.fill {
        div().flex_1().min_w(px(col.min_w * scale))
    } else {
        div()
            .flex_none()
            .flex_shrink_1()
            .w(px(col.w * scale))
            .min_w(px(col.min_w * scale))
    }
    .truncate();
    match col.align {
        Align::Right => el.text_right(),
        Align::Center => el.text_center(),
        Align::Left => el,
    }
}

/// The coin column: an explicit width, so the core name can take the free space.
fn coin_cell(scale: f32) -> Div {
    div()
        .flex_none()
        .flex_shrink_1()
        .w(px(DEAL_COIN_W * scale))
        .min_w(px(DEAL_COIN_MIN_W * scale))
        .truncate()
}

/// Columns drawn for this table. The plan column is В1's per-deal result; while the variant
/// is untouched every cell is empty, and the column is hidden so the core and the close
/// reason can use the width.
fn shown_deal_cols(show_plan: bool) -> impl Iterator<Item = &'static DealCol> {
    DEAL_COLS
        .iter()
        .filter(move |col| show_plan || col.key != COL_PLAN)
}

/// Every heading explains its quantity; symbols are composed here rather than in dictionaries.
type SortAction = std::rc::Rc<dyn Fn(&'static str, &mut Window, &mut App)>;

/// Share the production header with layout probes, including all hover trigger identities.
fn deal_header_el(
    sort: &Option<(String, bool)>,
    show_plan: bool,
    pick: SortAction,
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
    let scale = design::font_scale(cx);
    let sortable = |title: String, key: &'static str, col: Option<&DealCol>| {
        let arrow = sort_arrow_of(sort, key);
        let pick = pick.clone();
        let el = match col {
            Some(col) => deal_cell(col, scale),
            None => coin_cell(scale),
        };
        el.id(SharedString::from(format!("an-ticks-hdr-{key}")))
            .debug_selector(move || format!("an-ticks-hdr-{key}"))
            .tooltip(crate::panels::common::text_tooltip(column_tip(key)))
            .cursor_pointer()
            .text_color(moon(if arrow.is_empty() {
                p.text_soft
            } else {
                p.amber
            }))
            .child(
                div()
                    .truncate()
                    .debug_selector(move || format!("an-ticks-hdr-text-{key}"))
                    .child(format!("{title}{arrow}")),
            )
            .on_click(move |_, w, app| pick(key, w, app))
    };
    h_flex()
        .debug_selector(|| "an-ticks-header".to_string())
        .w_full()
        .flex_none()
        .h(design::fit_h_px(cx, 22.0, 12.0, 5.0))
        .px(design::ui_px(cx, DEAL_ROW_PAD_X))
        .gap(design::ui_px(cx, DEAL_ROW_GAP))
        .items_center()
        .text_size(design::t_caption(cx))
        .bg(moon(p.table_head))
        .child(sortable(
            t!("analytics.col.coin").to_string(),
            COL_COIN,
            None,
        ))
        .children(shown_deal_cols(show_plan).map(|c| sortable(column_title(c), c.key, Some(c))))
        .into_any_element()
}

/// Explain the quantity in every column, preserving glyphs in code.
fn column_tip(key: &str) -> String {
    let locale = format!("analytics.ticks.col_tip.{key}");
    let text = t!(locale.as_str()).to_string();
    match key {
        COL_MODEL => format!("{text}\n✓ · ✗ · ·"),
        COL_TAPE => format!("{text}\n● · ◐ · ○ · … · ✕ · ·"),
        _ => text,
    }
}

/// A wrapping symbol key keeps the model and tape marks understandable in a narrow dock.
fn legend_el(p: MoonPalette, cx: &App) -> impl IntoElement + use<> {
    h_flex()
        .debug_selector(|| "an-ticks-legend".to_string())
        .w_full()
        .flex_none()
        .flex_wrap()
        .px(design::ui_px(cx, DEAL_ROW_PAD_X))
        .py(design::ui_px(cx, 3.0))
        .gap(design::ui_px(cx, 8.0))
        .font_family(design::ui_font())
        .text_size(design::t_caption(cx))
        .text_color(moon(p.text_muted))
        .children(
            [
                ("hit", "✓", p.green),
                ("miss", "✗", p.red),
                ("unjudged", "·", p.text_muted),
                ("hole", "◐", p.amber),
            ]
            .into_iter()
            .map(|(key, glyph, color)| {
                let locale = format!("analytics.ticks.legend.{key}");
                div()
                    .flex_none()
                    .text_color(moon(color))
                    .debug_selector(move || format!("an-ticks-legend-{key}"))
                    .child(format!("{glyph} {}", t!(locale.as_str())))
            }),
        )
}

/// One callback serves chips, reset and the empty state without introducing repaint tasks.
type ChipAction = std::rc::Rc<dyn Fn(Option<why::DealChip>, &mut Window, &mut App)>;

/// Apply one filter edit and request exactly one repaint.
fn chip_action(view: WeakEntity<AnalyticsView>) -> ChipAction {
    std::rc::Rc::new(move |chip, _, app| {
        let _ = view.update(app, |this, cx| {
            this.ticks.chip = chip;
            cx.notify();
        });
    })
}

/// Whole-population counters wrap as intact controls, with static exclusions kept noninteractive.
fn chips_el(
    counts: rows::ChipCounts,
    active: Option<why::DealChip>,
    left_out: (usize, usize, usize),
    pick: ChipAction,
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
    let mut el = h_flex()
        .debug_selector(|| "an-ticks-chips".to_string())
        .w_full()
        .min_w_0()
        .flex_none()
        .flex_wrap()
        .gap(design::ui_px(cx, 6.0))
        .items_center()
        .font_family(design::ui_font())
        .text_size(design::t_caption(cx));
    for chip in why::DealChip::ALL {
        let key = chip.key();
        let label = format!("{}: {}", chip.label(counts.min_tail_s), counts.get(chip));
        let tip_key = format!("analytics.ticks.chip.{key}_tip");
        let pick = pick.clone();
        el = el.child(
            div()
                .id(SharedString::from(format!("an-ticks-chip-tip-{key}")))
                .debug_selector(move || format!("an-ticks-chip-{key}"))
                .flex_none()
                .tooltip(crate::panels::common::text_tooltip(
                    t!(tip_key.as_str(), min = counts.min_tail_s).to_string(),
                ))
                .child(
                    MoonButton::new(SharedString::from(format!("an-ticks-chip-{key}")))
                        .variant(if active == Some(chip) {
                            MoonButtonVariant::Amber
                        } else {
                            MoonButtonVariant::Soft
                        })
                        .label(label)
                        .on_click(move |_, w, app| {
                            pick(
                                if active == Some(chip) {
                                    None
                                } else {
                                    Some(chip)
                                },
                                w,
                                app,
                            )
                        })
                        .render(),
                ),
        );
    }
    for (key, n) in [
        ("without_ms", left_out.0),
        ("service", left_out.1),
        ("untunable", left_out.2),
    ] {
        let label_key = format!("analytics.ticks.chip.{key}");
        let tip_key = format!("analytics.ticks.chip.{key}_tip");
        el = el.child(
            div()
                .id(SharedString::from(format!("an-ticks-chip-tip-{key}")))
                .debug_selector(move || format!("an-ticks-chip-{key}"))
                .flex_none()
                .border_1()
                .border_dashed()
                .border_color(moon(p.border))
                .rounded(design::ui_px(cx, 3.0))
                .px(design::ui_px(cx, 7.0))
                .py(design::ui_px(cx, 2.0))
                .text_color(moon(p.text_muted))
                .tooltip(crate::panels::common::text_tooltip(
                    t!(tip_key.as_str()).to_string(),
                ))
                .child(format!("{}: {n}", t!(label_key.as_str()))),
        );
    }
    if let Some(chip) = active {
        el = el.child(
            h_flex()
                .debug_selector(|| "an-ticks-chip-reset-box".to_string())
                .w_full()
                .min_w_0()
                .items_center()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .debug_selector(|| "an-ticks-chip-filter-name".to_string())
                        .child(
                            t!(
                                "analytics.ticks.chip.filter",
                                name = chip.label(counts.min_tail_s)
                            )
                            .to_string(),
                        ),
                )
                .child(
                    div()
                        .flex_none()
                        .debug_selector(|| "an-ticks-chip-reset-link".to_string())
                        .child(
                            MoonButton::new("an-ticks-chip-reset")
                                .variant(MoonButtonVariant::Ghost)
                                .label(t!("analytics.ticks.chip.reset").to_string())
                                .on_click(move |_, w, app| pick(None, w, app))
                                .render(),
                        ),
                ),
        );
    }
    el.into_any_element()
}

/// A filter that hides the population has an immediate escape in the table body.
fn filtered_empty_el(pick: ChipAction, p: MoonPalette, cx: &App) -> AnyElement {
    v_flex()
        .debug_selector(|| "an-ticks-filter-empty".to_string())
        .w_full()
        .flex_1()
        .min_w_0()
        .items_center()
        .justify_center()
        .gap(design::ui_px(cx, 6.0))
        .text_color(moon(p.text_muted))
        .text_size(design::t_caption(cx))
        .font_family(design::ui_font())
        .child(
            div()
                .debug_selector(|| "an-ticks-filter-empty-text".to_string())
                .child(t!("analytics.ticks.chip.empty").to_string()),
        )
        .child(
            MoonButton::new("an-ticks-empty-reset")
                .variant(MoonButtonVariant::Ghost)
                .label(t!("analytics.ticks.chip.reset_filter").to_string())
                .on_click(move |_, w, app| pick(None, w, app))
                .render(),
        )
        .into_any_element()
}

/// One glyph per model group without allocating hover text on every repaint.
fn model_glyph(verdict: Option<moon_core::db::tuner::ticks::Verdict>) -> String {
    let Some(v) = verdict else {
        return String::new();
    };
    let glyph = |x: Option<bool>| match x {
        Some(true) => "✓",
        Some(false) => "✗",
        None => "·",
    };
    format!("{}{}", glyph(v.entry), glyph(v.exit))
}

/// One deal row. A click shows it in the trade pane, while the pane is open; a double-click
/// opens the trade window on it, as a Report row does.
type DealAction = std::rc::Rc<dyn Fn(i64, &ClickEvent, &mut Window, &mut App)>;

/// Keep row interactions outside the layout builder so probes exercise the same cell tree.
fn deal_action(view: WeakEntity<AnalyticsView>) -> DealAction {
    std::rc::Rc::new(move |uid, ev, _, app| {
        let _ = view.update(app, |this, cx| match ev.click_count() {
            1 => this.ticks_select_deal(uid, cx),
            _ => this.open_deal_window(uid, cx),
        });
    })
}

/// Draw only a visible row, with lazy hover explanations and shared classification inputs.
#[allow(clippy::too_many_arguments)]
fn deal_row(
    row: &DealRow,
    selected: bool,
    plan: PlanCell,
    action: DealAction,
    p: MoonPalette,
    scale: f32,
    row_h: f32,
    sample: (u32, i64),
    cx: &App,
) -> AnyElement {
    let d = &row.deal;
    let (min_tail_s, now_ms) = sample;
    let why = why::DealWhy::of(row, min_tail_s, now_ms);
    let tape_why = if row.tape == state::TapeStatus::Covered {
        why::DealWhy::tape_of(row, min_tail_s, now_ms)
    } else {
        why
    };
    let verdict = row.verdict;
    let result = rows::result_pct(row);
    let cell = |col: &DealCol, text: String, color: u32, tip: Option<String>| {
        let el = deal_cell(col, scale).text_color(moon(color));
        let el = if matches!(col.key, COL_RESULT | COL_PROFIT | COL_PLAN) {
            el.font_family(design::mono())
        } else {
            el.font_family(design::ui_font())
        };
        let key = col.key;
        let uid = d.report_uid;
        let el = el
            .debug_selector(move || format!("an-ticks-cell-{key}-{uid}"))
            .child(
                div()
                    .truncate()
                    .debug_selector(move || format!("an-ticks-text-{key}-{uid}"))
                    .child(text),
            );
        if col.key == COL_TAPE || col.key == COL_WHY || (col.key == COL_MODEL && verdict.is_some())
        {
            let tape = col.key == COL_TAPE;
            let reason = col.key == COL_WHY;
            let covered = row.tape == state::TapeStatus::Covered;
            return el
                .id(SharedString::from(format!(
                    "an-ticks-{}-{}",
                    col.key, d.report_uid
                )))
                .tooltip(move |window, cx| {
                    let text = if tape {
                        tape_why.tape_tip().text()
                    } else if reason && covered {
                        format!(
                            "{}\n\n{}",
                            tape_why.tape_tip().text(),
                            why.model_tip(verdict.as_ref()).text()
                        )
                    } else if reason {
                        tape_why.tape_tip().text()
                    } else {
                        why.model_tip(verdict.as_ref()).text()
                    };
                    crate::panels::common::text_tooltip(text)(window, cx)
                })
                .into_any_element();
        }
        match tip {
            Some(tip) if !tip.is_empty() => el
                .id(SharedString::from(format!(
                    "an-ticks-{}-{}",
                    col.key, d.report_uid
                )))
                .tooltip(crate::panels::common::text_tooltip(tip))
                .into_any_element(),
            _ => el.into_any_element(),
        }
    };
    let (tape_glyph, tape_tone) = tape_why.tape_glyph();
    let model_glyph = model_glyph(verdict);
    let text = p.text;
    // The outer slot is `Some` only once В1 has been scored. Until then the plan column is
    // empty on every row and is not drawn.
    let show_plan = plan.0.is_some();
    let mut el = h_flex()
        .id(SharedString::from(format!("an-tickrow-{}", d.report_uid)))
        .debug_selector({
            let uid = d.report_uid;
            move || format!("an-tickrow-{uid}")
        })
        .w_full()
        .h(px(row_h))
        .px(design::ui_px(cx, DEAL_ROW_PAD_X))
        .gap(design::ui_px(cx, DEAL_ROW_GAP))
        .items_center()
        .bg(moon(if selected { p.panel_high } else { p.table_body }))
        .border_t_1()
        .border_color(moon_alpha(p.border, 0.5))
        .child(coin_cell(scale).child(d.coin.clone()));
    for col in shown_deal_cols(show_plan) {
        let (value, color, tip) = match col.key {
            COL_KIND => (d.kind.clone(), p.text_muted, Some(d.kind.clone())),
            COL_PLAN => plan.render(p),
            // The core by the name the report carries; a row that carries none names it by
            // its uid, which is still an address. The full name is the tooltip: the cell
            // still truncates when the pane is narrow.
            COL_CORE => {
                let name = if d.core_name.trim().is_empty() {
                    format!("#{}", d.core_uid)
                } else {
                    d.core_name.clone()
                };
                (name.clone(), p.text_muted, Some(name))
            }
            COL_RESULT => {
                let (text, sign) = rows::paint_fixed(result, 2);
                (text, sign.pick(p.green, p.orange, p.text_muted), None)
            }
            // A dash where the scope's money cannot be valued in USDT (`Deal::profit`).
            // Colour follows the same rounded sign as the text, and the same orange a loss
            // uses in Summary — not a separate red.
            COL_PROFIT => match d.profit {
                Some(profit) => (
                    super::super::summary::fmt_signed_plain(profit),
                    super::super::summary::sign_color(p, profit),
                    None,
                ),
                None => ("—".to_string(), p.text_muted, None),
            },
            COL_DURATION => (
                duration_text(d.close_ms - d.buy_ms),
                p.text_muted,
                Some(duration_tip(d.buy_ms, d.buy_set_ms)),
            ),
            COL_HELD => (
                held_text(row.held),
                p.text_muted,
                row.held.map(|(lead, trail)| {
                    t!(
                        "analytics.ticks.held_tip",
                        lead = duration_text(lead),
                        trail = duration_text(trail)
                    )
                    .to_string()
                }),
            ),
            COL_REASON => (
                d.sell_reason.clone(),
                p.text_muted,
                Some(d.sell_reason.clone()),
            ),
            COL_TAPE => (
                tape_glyph.to_string(),
                match tape_tone {
                    why::Tone::Good => p.green,
                    why::Tone::Warning => p.amber,
                    why::Tone::Muted => p.text_muted,
                },
                None,
            ),
            COL_MODEL => (
                model_glyph.clone(),
                match row.verdict.and_then(|v| v.entry.or(v.exit)) {
                    Some(true) => p.green,
                    Some(false) => p.red,
                    None => p.text_muted,
                },
                None,
            ),
            COL_WHY => (
                why.short(),
                if why == why::DealWhy::Fit {
                    p.green
                } else {
                    p.text_muted
                },
                None,
            ),
            _ => (String::new(), text, None),
        };
        el = el.child(cell(col, value, color, tip));
    }
    let uid = d.report_uid;
    el = el
        .hover(move |s| s.bg(moon_alpha(p.panel_high, 0.9)))
        .on_click(move |ev, window, app| action(uid, ev, window, app));
    el.into_any_element()
}
