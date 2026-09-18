//! The trade's archived order lines: what to resolve, what to draw, what the rail says.
//!
//! A live chart draws the session's CURRENT orders; the trade window empties that source on its
//! engine because those orders describe a different moment than the one on screen. What it draws
//! instead is what the core archived when THIS trade finalized — its own buy and sell lines with
//! their repricing paths and stop markers, plus the lines it inherited through a join or a split —
//! keyed by the report row's `ReportUID` and drawn through the same geometry a live closed order
//! takes (`OrderLineStore::archived`).
//!
//! # One resolver, not one per window
//!
//! The window neither reads the archive nor asks the core itself: it hands the resolver on the
//! backend (`backend::traces`) the rows it wants — the subject, then up to [`NEIGHBOUR_CAP`]
//! neighbours nearest in time — and reads each row's state back after the resolver's wake. The
//! resolver reads the local archive first and asks the core only for what that did not hold, so a
//! trade opened yesterday costs no request today; a chart drawing the same trade in its lines
//! mode shares the very same answer.
//!
//! # Three ends, three sentences
//!
//! An EMPTY answer is final — the trade predates the archive or the core's mode does not write
//! one — and the block says so rather than spinning. A FAILED request (transport, the library's
//! own timeout, a core too old to know the command) is not evidence of an empty archive, so it
//! keeps a Retry button. A row the replica never received a uid for cannot be asked about at all,
//! and says that too.

use std::sync::Arc;

use gpui::*;
use moon_core::db::ChartTradeRecord;
use moon_core::feed::ArchivedOrderTrace;
use moon_core::session::order_lines::{ArchivedOrdersInput, OrderLineStore};
use moon_ui::{
    MoonButton, MoonButtonIconSlot, MoonButtonVariant, MoonPalette, MoonSize, h_flex, v_flex,
};
use rust_i18n::t;

use super::TradeWindowView;
use crate::backend::traces::TraceState as Resolved;
use crate::design;
use crate::design::moon;

/// How many "other trades" a window resolves, nearest the subject in time first.
///
/// The Report period can hold hundreds of one coin's trades; the ones whose lines can share the
/// subject's picture are the ones closed near it. The resolver reads them all from the archive in
/// one pass and asks the core for at most this many misses.
pub(super) const NEIGHBOUR_CAP: usize = 20;

/// Where this window stands with the trade's archived order lines.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum TraceState {
    /// The report row carries no `ReportUID`, so there is nothing to resolve it by.
    NotAskable,
    /// The archive is being read or the core has been asked.
    Pending,
    /// Lines are on the chart.
    Drawn { own: usize, inherited: usize },
    /// The core answered that it holds no archive for this trade. Final, until the user asks again.
    Missing,
    /// The request produced no answer. Retryable.
    Failed,
}

impl TraceState {
    /// Whether a Retry button can achieve anything from this state.
    ///
    /// `Missing` is retryable too, by the protocol's own advice: the core uses the same empty
    /// answer for an archive-storage failure and may backfill older trades at its startup, so a
    /// user-driven second ask is allowed while a timed one is not.
    fn retryable(&self) -> bool {
        matches!(self, Self::Failed | Self::Missing)
    }
}

impl TradeWindowView {
    /// Resolve this trade's archived lines: the archive, then the core on a miss.
    ///
    /// Called once at open. The Retry button takes [`Self::retry_traces`] instead, which skips
    /// the archive on purpose.
    ///
    /// Args:
    ///     cx: View context.
    pub(super) fn request_traces(&mut self, cx: &mut Context<Self>) {
        let Some(report_uid) = self.record.report_uid else {
            // Named, because "not askable" is a fact about the REPLICA ROW and the log is where a
            // reader checks that row: a key the database holds but the window did not get is a
            // read bug, not an old trade.
            log::info!(
                "[trade] traces not askable for {} core={} rec={}: no ReportUID on the row",
                self.market,
                self.core,
                self.record.record_id
            );
            self.traces = TraceState::NotAskable;
            cx.notify();
            return;
        };
        let core = self.core;
        self.backend.update(cx, |backend, cx| {
            backend.ensure_traces(core, vec![report_uid], 1, cx);
        });
        self.sync_traces(true, cx);
    }

    /// Ask the core again for the subject, past whatever the archive holds.
    ///
    /// Args:
    ///     cx: View context.
    pub(super) fn retry_traces(&mut self, cx: &mut Context<Self>) {
        let Some(report_uid) = self.record.report_uid else {
            return;
        };
        let core = self.core;
        self.backend.update(cx, |backend, cx| {
            backend.retry_trace(core, report_uid, cx);
        });
        self.sync_traces(true, cx);
    }

    /// Resolve the lines of the neighbours the "other trades" tick shows.
    ///
    /// Nothing while the tick is off. Bounded to [`NEIGHBOUR_CAP`] trades nearest the subject in
    /// time — the Report period can hold hundreds of a coin's trades, and the ones that matter are
    /// the ones whose lines can share the picture. The resolver skips what it already holds, so a
    /// re-click that keeps the same neighbours costs nothing; a neighbour without a `ReportUID`
    /// cannot be resolved at all.
    ///
    /// Args:
    ///     cx: View context.
    pub(super) fn request_neighbour_traces(&mut self, cx: &mut Context<Self>) {
        if !self.show_other_trades {
            return;
        }
        let focus_close = self.record.close_date;
        let mut candidates: Vec<(i64, i64)> = self
            .history
            .iter()
            .filter(|r| r.record_id != self.record.record_id)
            .filter_map(|r| {
                r.report_uid
                    .map(|uid| (uid, (r.close_date - focus_close).abs()))
            })
            .collect();
        candidates.sort_by_key(|(_, distance)| *distance);
        let uids: Vec<i64> = candidates
            .into_iter()
            .take(NEIGHBOUR_CAP)
            .map(|(uid, _)| uid)
            .collect();
        if uids.is_empty() {
            return;
        }
        let core = self.core;
        self.backend.update(cx, |backend, cx| {
            backend.ensure_traces(core, uids, NEIGHBOUR_CAP, cx);
        });
    }

    /// Read the resolver's answers and rebuild the archived store when a line of this window's
    /// changed. Called from the resolver's wake, and by hand after anything that changes which
    /// rows the window draws.
    ///
    /// Args:
    ///     force: Rebuild even when the signature did not move — the drawn set changed on this
    ///         side (a tick, a new period) rather than on the resolver's.
    ///     cx: View context.
    pub(super) fn sync_traces(&mut self, force: bool, cx: &mut Context<Self>) {
        let subject_uid = self.record.report_uid.unwrap_or_default();
        let (subject, neighbours, sig) = {
            let backend = self.backend.read(cx);
            let (subject, subject_stamp) = backend.trace_state_stamped(self.core, subject_uid);
            let mut sig = subject_stamp;
            let mut neighbours = Vec::new();
            if self.show_other_trades {
                for record in self.history.iter() {
                    let Some(uid) = record
                        .report_uid
                        .filter(|_| record.record_id != self.record.record_id)
                    else {
                        continue;
                    };
                    let (state, stamp) = backend.trace_state_stamped(self.core, uid);
                    sig = sig.wrapping_mul(31).wrapping_add(stamp);
                    if let Some(lines) = state.lines().filter(|lines| !lines.is_empty()) {
                        neighbours.push((record.clone(), lines.clone()));
                    }
                }
            }
            (subject, neighbours, sig)
        };
        if !force && sig == self.traces_sig {
            return;
        }
        self.traces_sig = sig;
        if self.traces != TraceState::NotAskable {
            let next = match &subject {
                Resolved::Unknown | Resolved::Loading | Resolved::Unasked | Resolved::Pending => {
                    TraceState::Pending
                }
                Resolved::Lines(lines) => {
                    let own = lines.iter().filter(|line| line.own).count();
                    TraceState::Drawn {
                        own,
                        inherited: lines.len() - own,
                    }
                }
                Resolved::Empty => TraceState::Missing,
                Resolved::Failed => TraceState::Failed,
            };
            if next != self.traces {
                // The one line a live check reads back: what was resolved and what reached the
                // chart.
                log::info!(
                    "[trade] traces for {} uid={subject_uid}: {next:?}",
                    self.market
                );
                self.traces = next;
            }
        }
        self.rebuild_frozen_orders(subject.lines().cloned(), &neighbours, cx);
        cx.notify();
    }

    /// Build the archived store from everything resolved so far and hand it to the chart.
    ///
    /// The subject's lines first, at full opacity; then, while the "other trades" tick is on,
    /// every neighbour that answered with lines, pale. The store is rebuilt whole on each change
    /// — at most a couple of dozen orders — and stamped with a fresh revision so the chart's
    /// revision gate sees it arrive.
    ///
    /// Row stamps are lifted onto true UTC the way `fetch` lifts them for the REST window, so a
    /// subject's entry line ends at the same instant its entry arrow is drawn at.
    ///
    /// Args:
    ///     subject: The subject's lines, when resolved.
    ///     neighbours: Each drawn neighbour's row and lines.
    ///     cx: View context.
    fn rebuild_frozen_orders(
        &mut self,
        subject: Option<Arc<[ArchivedOrderTrace]>>,
        neighbours: &[(ChartTradeRecord, Arc<[ArchivedOrderTrace]>)],
        cx: &mut Context<Self>,
    ) {
        let axis = self
            .backend
            .read(cx)
            .report_axis(crate::chartdx::axes::display_zone());
        let input = |record: &ChartTradeRecord| {
            let (buy_utc_ms, close_utc_ms) = super::utc_stamps_ms(record, &axis);
            ArchivedOrdersInput {
                market: &self.market,
                is_short: record.is_short,
                quantity: record.quantity as f32,
                entry_fill_ms: Some(buy_utc_ms as f64),
                close_ms: close_utc_ms as f64,
                bright: false,
            }
        };
        let mut store =
            OrderLineStore::archived(input(&self.record), subject.as_deref().unwrap_or(&[]));
        for (record, lines) in neighbours {
            store.append_archived(input(record), lines);
        }
        self.neighbours_drawn = neighbours.len();
        self.frozen_rev = self.frozen_rev.wrapping_add(1).max(1);
        store.rev = self.frozen_rev;
        self.panel.update(cx, |panel, pcx| {
            panel.attach_frozen_orders(Some(std::rc::Rc::new(store)), pcx);
        });
    }
}

/// The sentence the rail prints for one state.
///
/// A free function so the wording per state is pinnable without a window.
///
/// Args:
///     state: Where the window stands with the subject's lines.
///     neighbours: How many other trades' lines are drawn beside them; zero adds nothing.
///
/// Returns:
///     The localized sentence.
pub(super) fn caption(state: &TraceState, neighbours: usize) -> String {
    let subject = match state {
        TraceState::NotAskable => t!("trade_window.traces.not_askable").to_string(),
        TraceState::Pending => t!("trade_window.traces.pending").to_string(),
        TraceState::Drawn { own, inherited: 0 } => {
            t!("trade_window.traces.drawn_own", own = own).to_string()
        }
        TraceState::Drawn { own, inherited } => t!(
            "trade_window.traces.drawn",
            own = own,
            inherited = inherited
        )
        .to_string(),
        TraceState::Missing => t!("trade_window.traces.none").to_string(),
        TraceState::Failed => t!("trade_window.traces.failed").to_string(),
    };
    match neighbours {
        0 => subject,
        n => format!(
            "{subject} · {}",
            t!("trade_window.traces.neighbours", n = n)
        ),
    }
}

/// Render the rail's "Orders" block: the caption for the state, and Retry where it can help.
///
/// Args:
///     state: Where the window stands with the subject's lines.
///     neighbours: How many other trades' lines are on the chart beside them.
///     view: The window, for the Retry click.
///     p: Active palette.
///     cx: Render context, for scaled type.
///
/// Returns:
///     One label-over-content block shaped like the rail's other cells.
pub(super) fn render_block(
    state: &TraceState,
    neighbours: usize,
    view: &Entity<TradeWindowView>,
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
    let tone = match state {
        TraceState::Drawn { .. } => moon(p.text),
        TraceState::Failed => moon(p.red_text),
        _ => moon(p.text_soft),
    };
    let text = caption(state, neighbours);
    let retry = state.retryable().then(|| {
        let view = view.clone();
        MoonButton::new("tw-traces-retry")
            .width(design::micro_control_h_value(cx))
            .variant(MoonButtonVariant::Soft)
            .size(MoonSize::Xs)
            .leading_icon(MoonButtonIconSlot::new("icons/refresh-cw.svg").color(p.text_soft))
            .tooltip(t!("trade_window.traces.retry_tip").to_string())
            .on_click(move |_, _window, app| {
                app.stop_propagation();
                view.update(app, |this, cx| this.retry_traces(cx));
            })
            .render()
    });
    // No `w_full()`, for the reason the strategy block gives: in the narrow, wrapping strip a
    // full-width child would take a line of its own.
    v_flex()
        .id("tw-traces")
        .min_w_0()
        .gap(design::ui_px(cx, 1.0))
        .child(
            div()
                .text_size(design::t_caption(cx))
                .text_color(moon(p.text_muted))
                .child(t!("trade_window.figure.orders").to_string()),
        )
        .child(
            h_flex()
                .w_full()
                .min_w_0()
                .items_center()
                .gap(design::ui_px(cx, design::CHROME_GAP))
                .child(
                    div()
                        .id("tw-traces-caption")
                        .flex_1()
                        .min_w_0()
                        .text_size(design::t_caption(cx))
                        .text_color(tone)
                        .tooltip(crate::panels::common::text_tooltip(text.clone()))
                        .child(text),
                )
                .children(retry),
        )
        .into_any_element()
}
