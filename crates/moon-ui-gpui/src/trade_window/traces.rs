//! The trade's archived order lines: asking the core, folding its answer, stating the result.
//!
//! A live chart draws the session's CURRENT orders; the trade window empties that source on its
//! engine because those orders describe a different moment than the one on screen. What it draws
//! instead is what the core archived when THIS trade finalized — its own buy and sell lines with
//! their repricing paths and stop markers, plus the lines it inherited through a join or a split —
//! asked for by the report row's `ReportUID` and drawn through the same geometry a live closed
//! order takes (`OrderLineStore::archived`).
//!
//! # The answer is filed, not delivered
//!
//! The feed files the core's answer in the session store under the row's uid, and this window
//! reads it from the backend observer it already runs — one revision compare per notification,
//! nothing while there is no request in flight. An answer filed BEFORE this window asked is never
//! adopted, however complete it looks: `ReportUID` is unique within ONE report database, and a
//! core whose database was recreated can reissue a uid an earlier window already asked about —
//! the filed lines would then be another trade's. Every open asks the core afresh (MoonProto
//! shares one network request between concurrent asks for the same row, so a second window on
//! the same trade costs nothing), and the entry's revision stamp is what tells the fresh answer
//! from the stale one.
//!
//! # Three ends, three sentences
//!
//! An EMPTY answer is final — the trade predates the archive or the core's mode does not write
//! one — and the block says so rather than spinning. A FAILED request (transport, the library's
//! own timeout, a core too old to know the command) is not evidence of an empty archive, so it
//! keeps a Retry button. A row the replica never received a uid for cannot be asked about at all,
//! and says that too.

use gpui::*;
use moon_core::feed::ReportTracesOutcome;
use moon_core::session::order_lines::{ArchivedOrdersInput, OrderLineStore};
use moon_ui::{
    MoonButton, MoonButtonIconSlot, MoonButtonVariant, MoonPalette, MoonSize, h_flex, v_flex,
};
use rust_i18n::t;

use super::TradeWindowView;
use crate::design;
use crate::design::moon;

/// How many "other trades" a window asks the core about, nearest the subject in time first.
///
/// The Report period can hold hundreds of one coin's trades; the ones whose lines can share the
/// subject's picture are the ones closed near it. MoonProto shares one network request between
/// windows asking about the same row, so this bounds the terminal's own ask, not the wire's cost.
pub(super) const NEIGHBOUR_CAP: usize = 20;

/// Where this window stands with the trade's archived order lines.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum TraceState {
    /// The report row carries no `ReportUID`, so there is nothing to ask the core with.
    NotAskable,
    /// A request is in flight; the observer is watching for its answer.
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
    /// Ask the core for this trade's archived lines.
    ///
    /// Called once at open and again from the Retry button. Whatever the store already holds for
    /// this uid is superseded, not reused — see the module docs for why a filed answer cannot be
    /// trusted across the core's own database lifetime — so the observer waits for an entry whose
    /// revision is past the one seen here.
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
        (self.traces_seen_rev, self.traces_epoch) = self.report_traces_marks(cx);
        self.traces = match self.send_trace_request(report_uid, cx) {
            true => TraceState::Pending,
            false => TraceState::Failed,
        };
        cx.notify();
    }

    /// Ask the core for the lines of the neighbours the "other trades" tick shows.
    ///
    /// Nothing while the tick is off. Bounded to [`NEIGHBOUR_CAP`] trades nearest the subject in
    /// time — the Report period can hold hundreds of a coin's trades, and the ones that matter are
    /// the ones whose lines can share the picture. A neighbour already asked, answered or refused
    /// is not asked twice; a neighbour without a `ReportUID` cannot be asked at all.
    ///
    /// Args:
    ///     cx: View context.
    pub(super) fn request_neighbour_traces(&mut self, cx: &mut Context<Self>) {
        if !self.show_other_trades {
            return;
        }
        // The cap counts neighbours of the CURRENT snapshot only: a re-click can replace the
        // history with a different period, and slots burnt on trades no longer shown would leave
        // the window unable to fill up with the ones that are.
        let focus_close = self.record.close_date;
        let mut known = 0usize;
        let mut candidates: Vec<(i64, i64)> = Vec::new();
        for r in self.history.iter() {
            if r.record_id == self.record.record_id {
                continue;
            }
            let Some(uid) = r.report_uid else {
                continue;
            };
            if self.neighbour_pending.contains_key(&uid) || self.neighbour_lines.contains_key(&uid)
            {
                known += 1;
            } else {
                candidates.push((uid, (r.close_date - focus_close).abs()));
            }
        }
        if known >= NEIGHBOUR_CAP {
            return;
        }
        candidates.sort_by_key(|(_, distance)| *distance);
        let (seen_rev, epoch) = self.report_traces_marks(cx);
        self.traces_epoch = epoch;
        for (uid, _) in candidates.into_iter().take(NEIGHBOUR_CAP - known) {
            if self.send_trace_request(uid, cx) {
                self.neighbour_pending.insert(uid, seen_rev);
            }
        }
    }

    /// The core's current `(report_traces_rev, report_traces_epoch)`, or zeros when the core is
    /// not in the store.
    fn report_traces_marks(&self, cx: &App) -> (u64, u64) {
        self.backend
            .read(cx)
            .session
            .store()
            .core(self.core)
            .map_or((0, 0), |core| {
                (core.report_traces_rev, core.report_traces_epoch)
            })
    }

    /// Send one request; `false` when it could not even be queued.
    fn send_trace_request(&self, report_uid: i64, cx: &App) -> bool {
        match self
            .backend
            .read(cx)
            .session
            .request_report_traces(self.core, report_uid)
        {
            Ok(()) => {
                log::info!(
                    "[trade] traces requested for {} uid={report_uid}",
                    self.market
                );
                true
            }
            Err(error) => {
                log::warn!(
                    "[x] trade traces request for {} uid={report_uid} not sent: {error}",
                    self.market
                );
                false
            }
        }
    }

    /// Read the core's answers once they are filed. Called from the backend observer.
    ///
    /// Cheap while nothing is pending: two emptiness checks. While requests are in flight it is
    /// one revision compare per notification, and the map is consulted only when that moved.
    ///
    /// Args:
    ///     cx: View context.
    pub(super) fn poll_traces(&mut self, cx: &mut Context<Self>) {
        let subject_pending = self.traces == TraceState::Pending;
        if !subject_pending && self.neighbour_pending.is_empty() {
            return;
        }
        let mut changed = false;
        let (rev, epoch) = {
            let backend = self.backend.read(cx);
            let Some(core) = backend.session.store().core(self.core) else {
                // The core was REMOVED from the session (not merely disconnected — a reconnect
                // keeps its data) while requests were out. No answer can land now, and a block
                // left reading "asking…" with nothing to press would be a spinner in words.
                if subject_pending {
                    self.traces = TraceState::Failed;
                }
                self.neighbour_pending.clear();
                cx.notify();
                return;
            };
            (core.report_traces_rev, core.report_traces_epoch)
        };
        if rev == self.neighbour_poll_rev {
            return;
        }
        self.neighbour_poll_rev = rev;
        if epoch != self.traces_epoch {
            // A DIFFERENT MoonBot process answers on this core now and the store dropped every
            // filed answer with the old one. Whatever was asked can no longer land: the subject
            // fails (retryable), the neighbours are forgotten so a re-tick asks again.
            self.traces_epoch = epoch;
            if subject_pending {
                log::warn!(
                    "[x] trade traces for {}: core process replaced while the request was out",
                    self.market
                );
                self.traces = TraceState::Failed;
            }
            self.neighbour_pending.clear();
            cx.notify();
            return;
        }
        if subject_pending {
            let report_uid = self.record.report_uid.unwrap_or_default();
            // Only an entry filed AFTER the request went out is its answer; an older one is what
            // `request_traces` deliberately chose to supersede.
            if let Some(outcome) = self.filed_after(report_uid, self.traces_seen_rev, cx) {
                match outcome {
                    ReportTracesOutcome::Ready(lines) if lines.is_empty() => {
                        log::info!(
                            "[trade] traces for {} uid={report_uid}: the core holds no archive",
                            self.market
                        );
                        self.traces = TraceState::Missing;
                        self.subject_lines = None;
                    }
                    ReportTracesOutcome::Ready(lines) => {
                        let own = lines.iter().filter(|line| line.own).count();
                        let inherited = lines.len() - own;
                        // The one line a live check reads back: what the core answered and what
                        // reached the chart.
                        log::info!(
                            "[trade] traces for {} uid={report_uid}: {own} own, {inherited} inherited drawn",
                            self.market
                        );
                        self.traces = TraceState::Drawn { own, inherited };
                        self.subject_lines = Some(lines);
                    }
                    ReportTracesOutcome::Failed(error) => {
                        // The diagnostic is an English transport fragment and belongs in the log,
                        // never in the user's sentence — the block says only that the core did
                        // not answer.
                        log::warn!(
                            "[x] trade traces for {} uid={report_uid} failed: {error}",
                            self.market
                        );
                        self.traces = TraceState::Failed;
                    }
                }
                changed = true;
            }
        }
        let pending: Vec<(i64, u64)> = self
            .neighbour_pending
            .iter()
            .map(|(u, r)| (*u, *r))
            .collect();
        for (uid, seen) in pending {
            let Some(outcome) = self.filed_after(uid, seen, cx) else {
                continue;
            };
            self.neighbour_pending.remove(&uid);
            match outcome {
                // An empty answer is filed too, so the neighbour is not asked again.
                ReportTracesOutcome::Ready(lines) => {
                    self.neighbour_lines.insert(uid, lines);
                }
                ReportTracesOutcome::Failed(error) => {
                    log::warn!(
                        "[x] neighbour traces for {} uid={uid} failed: {error}",
                        self.market
                    );
                }
            }
            changed = true;
        }
        if changed {
            self.rebuild_frozen_orders(cx);
        }
        cx.notify();
    }

    /// The outcome filed for `uid` after revision `seen`, if any.
    fn filed_after(&self, uid: i64, seen: u64, cx: &App) -> Option<ReportTracesOutcome> {
        let backend = self.backend.read(cx);
        let core = backend.session.store().core(self.core)?;
        let entry = core.report_traces.get(&uid)?;
        (entry.rev > seen).then(|| entry.outcome.clone())
    }

    /// Build the archived store from everything answered so far and hand it to the chart.
    ///
    /// The subject's lines first, at full opacity; then, while the "other trades" tick is on,
    /// every neighbour that answered with lines, pale. The store is rebuilt whole on each change
    /// — at most a couple of dozen orders — and stamped with a fresh revision so the chart's
    /// revision gate sees it arrive. Called on the tick too, so untick takes the lines away with
    /// the arrows and re-tick brings them back without asking the core again.
    ///
    /// Row stamps are lifted onto true UTC the way `fetch` lifts them for the REST window, so a
    /// subject's entry line ends at the same instant its entry arrow is drawn at.
    ///
    /// Args:
    ///     cx: View context.
    pub(super) fn rebuild_frozen_orders(&mut self, cx: &mut Context<Self>) {
        let axis = self
            .backend
            .read(cx)
            .report_axis(crate::chartdx::axes::display_zone());
        let input = |record: &moon_core::db::ChartTradeRecord| {
            let (buy_utc_ms, close_utc_ms) = super::utc_stamps_ms(record, &axis);
            (
                record.is_short,
                record.quantity as f32,
                buy_utc_ms as f64,
                close_utc_ms as f64,
            )
        };
        let (is_short, quantity, entry_fill_ms, close_ms) = input(&self.record);
        let subject_input = ArchivedOrdersInput {
            market: &self.market,
            is_short,
            quantity,
            entry_fill_ms: Some(entry_fill_ms),
            close_ms,
        };
        let mut store =
            OrderLineStore::archived(subject_input, self.subject_lines.as_deref().unwrap_or(&[]));
        let mut neighbours_drawn = 0;
        if self.show_other_trades {
            for record in self.history.iter() {
                let Some(lines) = record
                    .report_uid
                    .filter(|_| record.record_id != self.record.record_id)
                    .and_then(|uid| self.neighbour_lines.get(&uid))
                else {
                    continue;
                };
                if lines.is_empty() {
                    continue;
                }
                let (is_short, quantity, entry_fill_ms, close_ms) = input(record);
                store.append_archived(
                    ArchivedOrdersInput {
                        market: &self.market,
                        is_short,
                        quantity,
                        entry_fill_ms: Some(entry_fill_ms),
                        close_ms,
                    },
                    lines,
                );
                neighbours_drawn += 1;
            }
        }
        self.neighbours_drawn = neighbours_drawn;
        self.frozen_rev = self.frozen_rev.wrapping_add(1).max(1);
        store.rev = self.frozen_rev;
        self.panel.update(cx, |panel, pcx| {
            panel.attach_frozen_orders(Some(std::rc::Rc::new(store)), pcx);
        });
        cx.notify();
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
                view.update(app, |this, cx| this.request_traces(cx));
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
