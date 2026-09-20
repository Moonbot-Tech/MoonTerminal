//! "Fetch the tape" — the rows whose window the terminal does not hold, asked from the venue
//! one at a time through the same replay request a trade window makes.
//!
//! One at a time because the worker is one thread and the venues are rate-limited: a burst of
//! a hundred requests would queue a hundred tick walks behind every chart window's own. The
//! answers are read for their STATUS only (`NoRoute`, `OutOfRetention`, …) and shown in the
//! row's tooltip, so the button does not look broken on a venue without a public route; the
//! prints themselves land in the worker's tiles and on disk, and the row is then replayed
//! through the same held-data query the load uses.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc;

use gpui::*;

use super::super::super::AnalyticsView;
use super::load::{ArchivedLines, replay_row};
use super::state::TapeStatus;
use crate::analytics::bg::ReadLane;
use moon_core::db::order_traces::read_many;
use moon_core::db::tuner::ticks::Deal;
use moon_core::market::trade_replay::worker::{self, TradeReplayRequest};
use moon_core::market::trade_replay::{
    TickStatus, TradeReplayOutcome, margin_ms, replay_window_ms,
};

impl AnalyticsView {
    /// Queue every fetchable row and start on the first.
    pub(in crate::analytics::tuner) fn ticks_fetch_missing(&mut self, cx: &mut Context<Self>) {
        if self.ticks.fetch.is_active() {
            return;
        }
        let Some(data) = self.ticks.data.data() else {
            return;
        };
        let mut pending: Vec<i64> = data.fetchable().map(|r| r.deal.report_uid).collect();
        if pending.is_empty() {
            return;
        }
        // Oldest first, so the ones nearest the venues' retention edge go before it moves.
        pending.reverse();
        self.ticks.fetch.total = pending.len();
        self.ticks.fetch.done = 0;
        self.ticks.fetch.pending = pending;
        self.ticks_fetch_next(cx);
    }

    /// Abandon the queue; the request in flight finishes on its own and is dropped on arrival.
    pub(in crate::analytics::tuner) fn ticks_fetch_stop(&mut self, cx: &mut Context<Self>) {
        let in_flight = self.ticks.fetch.in_flight;
        self.ticks.fetch.clear();
        if let Some(uid) = in_flight {
            self.ticks.update_row(uid, |row| {
                if row.tape == TapeStatus::Fetching {
                    row.tape = TapeStatus::Missing;
                }
            });
        }
        cx.notify();
    }

    /// Ask for the next queued row, if any.
    fn ticks_fetch_next(&mut self, cx: &mut Context<Self>) {
        let Some(uid) = self.ticks.fetch.pending.pop() else {
            self.ticks.fetch.in_flight = None;
            cx.notify();
            return;
        };
        let Some((deal, address)) = self.ticks.data.data().and_then(|d| {
            d.rows
                .iter()
                .find(|r| r.deal.report_uid == uid)
                .and_then(|r| Some((r.deal.clone(), r.address.clone()?)))
        }) else {
            self.ticks_fetch_next(cx);
            return;
        };
        let Some(window) = replay_window_ms(deal.buy_ms, deal.close_ms, margin_ms()) else {
            self.ticks_fetch_next(cx);
            return;
        };
        // The same addressing a trade window resolves: the live source for the exchange
        // identity, the core's contract terms for how the prints are valued.
        let resolved = {
            let backend = self.backend.read(cx);
            let source = backend.session.market_source();
            source
                .replay_address(address.core_uid)
                .ok()
                .map(|replay_address| {
                    let terms = source.market_contract_terms(address.core_uid, &address.market);
                    let tick_value = moon_core::market::trade_replay::venue_caps::tick_value(
                        replay_address.venue,
                        terms.as_ref().map(|(quote, size)| (quote.as_str(), *size)),
                    );
                    (replay_address, tick_value)
                })
        };
        let Some((replay_address, tick_value)) = resolved else {
            // The core went away since the load resolved the row: not a fetch failure, an
            // address the row no longer has.
            self.ticks
                .update_row(uid, |row| row.tape = TapeStatus::NoAddress);
            self.ticks_fetch_next(cx);
            return;
        };
        self.ticks.fetch.in_flight = Some(uid);
        let seq = self.ticks.fetch.seq;
        self.ticks
            .update_row(uid, |row| row.tape = TapeStatus::Fetching);
        let (reply, rx) = mpsc::channel();
        worker::request(TradeReplayRequest {
            address: replay_address,
            market: address.market.clone(),
            window,
            identity: fetch_identity(uid),
            tick_value,
            ticks: true,
            cancel: Arc::new(AtomicBool::new(false)),
            reply,
        });
        let defaults = self.filter_defaults(cx);
        self.spawn_latest_db(
            &[ReadLane::TicksFetch],
            false,
            cx,
            move || {
                // The worker streams the candle stage, then the tick stage, then drops the
                // sender: the last outcome before the drop is the tick stage's word.
                let mut status = TickStatus::Failed;
                while let Ok(outcome) = rx.recv() {
                    status = match outcome {
                        TradeReplayOutcome::Ready(series) => series.tick_status,
                        TradeReplayOutcome::Empty(_) | TradeReplayOutcome::Failed(_) => {
                            TickStatus::Failed
                        }
                    };
                }
                let lines = archived_lines_of(&deal);
                let mut row = super::state::DealRow {
                    deal,
                    tape: TapeStatus::Missing,
                    verdict: None,
                    address: Some(address),
                    ticks: None,
                    entry_start: None,
                };
                replay_row(&mut row, &defaults, lines);
                if row.tape == TapeStatus::Missing {
                    row.tape = match status {
                        TickStatus::Served | TickStatus::Pending | TickStatus::Streaming => {
                            TapeStatus::Missing
                        }
                        refused => TapeStatus::Refused(refused),
                    };
                }
                row
            },
            move |this, row, cx| {
                if this.ticks.fetch.seq != seq {
                    return;
                }
                let uid = row.deal.report_uid;
                this.ticks.update_row(uid, |slot| {
                    slot.tape = row.tape;
                    slot.verdict = row.verdict;
                    slot.deal.tick = row.deal.tick;
                    slot.ticks = row.ticks;
                    slot.entry_start = row.entry_start;
                });
                this.ticks.fetch.done += 1;
                // A row joined the replayable set: the variant columns are due a rescore.
                this.arm_ticks_variants(cx);
                this.ticks_fetch_next(cx);
            },
        );
    }
}

/// A replay identity for a fetch, distinct from every chart window's: the row's own id, which
/// no window uses as a series discriminator.
fn fetch_identity(report_uid: i64) -> u64 {
    // FNV-1a over the id, salted so a window's `identity` (an entity number) cannot collide.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325 ^ 0x7469_636b_7300_0000;
    for byte in report_uid.to_le_bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash | 1
}

/// The archived lines of one deal.
fn archived_lines_of(deal: &Deal) -> ArchivedLines {
    read_many(deal.core_uid, &[deal.report_uid])
        .ok()
        .and_then(|entries| entries.get(&deal.report_uid).map(ArchivedLines::of))
        .unwrap_or_default()
}
