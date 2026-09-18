//! An [`OrderLineStore`] built from a closed trade's ARCHIVED traces rather than from live rows.
//!
//! The trade window draws a market that stopped moving hours ago, so the live order store — which
//! holds what the user has open RIGHT NOW — is emptied on its engine. What it draws instead is the
//! core's archive of that trade's own order lines and of the lines it inherited through a join or
//! a split. Building those into the same store type is what lets the chart draw them through the
//! exact code a live closed order takes: the archive's point layout is `LineTrace::server_points`'
//! layout, its stop marker is `server_stop_price`, and every style rule — long/short colours,
//! the repricing path, the "hide move history" toggle — applies unchanged.
//!
//! A child module rather than a method beside `update`: the store's fields are private to keep
//! live bookkeeping (closure grace, the closed ring, generations) out of reach, and this is the one
//! other legitimate writer.

use super::{LineKind, LineTrace, OrderCloseReason, OrderLineStore, RetainedOrder};
use crate::feed::{ArchivedLineKind, ArchivedOrderTrace};

/// What the archive does not carry and the trade's own report row does.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ArchivedOrdersInput<'a> {
    /// Exchange-native market the lines belong to; the store is per market on the draw side.
    pub market: &'a str,
    /// Whether the trade was a short, which picks the `buy_short`/`sell_short` styles.
    pub is_short: bool,
    /// Filled base quantity. Mirrored onto the order's `size` for completeness — a closed order
    /// draws no label today, so nothing prints it; a consumer asking the store still gets the
    /// row's own number rather than a zero.
    pub quantity: f32,
    /// Entry fill instant in Unix UTC ms, where the subject's entry line ends. `None` draws it to
    /// the trade's close, as a live order the core never dated a fill for is drawn.
    pub entry_fill_ms: Option<f64>,
    /// Exit instant in Unix UTC ms: where the subject order — and every inherited line, whose
    /// own end the archive does not state — stops being drawn.
    pub close_ms: f64,
}

/// Synthetic uid of the subject order. Inherited lines count up from it; nothing here can collide
/// with a live uid because this store never meets a live one.
const SUBJECT_UID: u64 = 1;

impl OrderLineStore {
    /// Build a store holding one closed trade's archived lines, and nothing else.
    ///
    /// The first own entry line and the first own exit line become ONE order — the subject, drawn
    /// at full opacity. Every other line, own or inherited, becomes an order of its own: the
    /// archive lists lines, not orders, and two own lines of one kind are two orders' worth of
    /// geometry that happen to share a trade. Inherited lines are pale (`subject == false`).
    ///
    /// Args:
    ///     input: The row's facts the archive lacks.
    ///     traces: The archived lines, own first as the core writes them.
    ///
    /// Returns:
    ///     A store whose `rev` is nonzero, so a chart that compares revisions sees it arrive.
    pub fn archived(input: ArchivedOrdersInput<'_>, traces: &[ArchivedOrderTrace]) -> Self {
        let mut store = Self::default();
        let mut next_uid = SUBJECT_UID + 1;
        let mut subject: Option<RetainedOrder> = None;
        for (seq, trace) in traces.iter().enumerate() {
            let Some(&(first_ms, _)) = trace.points.first() else {
                continue;
            };
            let kind = match trace.kind {
                ArchivedLineKind::Entry => LineKind::Buy,
                ArchivedLineKind::Exit => LineKind::Sell,
            };
            let line = archived_line(trace);
            // The subject absorbs the first own line of each kind; a second own line of a kind it
            // already holds is a second order, as are all inherited lines.
            let joins_subject = trace.own
                && subject
                    .as_ref()
                    .is_none_or(|order| order.lines[kind as usize].steps.is_empty());
            if joins_subject {
                let order = subject.get_or_insert_with(|| {
                    archived_order(&input, SUBJECT_UID, seq as u64, first_ms, true)
                });
                order.create_ms = order.create_ms.min(first_ms);
                order.lines[kind as usize] = line;
                continue;
            }
            let uid = next_uid;
            next_uid += 1;
            let mut order = archived_order(&input, uid, seq as u64, first_ms, false);
            order.lines[kind as usize] = line;
            store.insert_closed(order);
        }
        if let Some(order) = subject {
            store.insert_closed(order);
        }
        store.rev = 1;
        store
    }

    /// Add another closed trade's archived lines to this store, every one of them pale.
    ///
    /// For the trade window's "other trades": the neighbours of the subject drawn beside it, each
    /// asked from the core by its own `ReportUID`. None of them is the subject, so none takes the
    /// active opacity — own and inherited alike, they are context. Uids continue past whatever
    /// the store already holds, so two neighbours can never collide.
    ///
    /// The caller stamps `rev` afterwards: a rebuilt store must differ from the one it replaces
    /// for the chart's revision gate, and only the caller knows how many rebuilds it has made.
    ///
    /// Args:
    ///     input: The neighbour row's facts the archive lacks.
    ///     traces: Its archived lines.
    pub fn append_archived(
        &mut self,
        input: ArchivedOrdersInput<'_>,
        traces: &[ArchivedOrderTrace],
    ) {
        let mut next_uid = self.orders.keys().copied().max().unwrap_or(SUBJECT_UID) + 1;
        let seq_base = self.orders.len() as u64;
        // The neighbour's row states ONE fill, and it belongs to its first own entry line — the
        // same rule `archived` applies to the subject; a second own entry line of the same trade
        // runs undated to the close, like an inherited one.
        let mut fill_taken = false;
        for (seq, trace) in traces.iter().enumerate() {
            let Some(&(first_ms, _)) = trace.points.first() else {
                continue;
            };
            let kind = match trace.kind {
                ArchivedLineKind::Entry => LineKind::Buy,
                ArchivedLineKind::Exit => LineKind::Sell,
            };
            let mut order =
                archived_order(&input, next_uid, seq_base + seq as u64, first_ms, false);
            if trace.own && kind == LineKind::Buy && !fill_taken {
                fill_taken = true;
                order.entry_fill_ms = input.entry_fill_ms;
            }
            order.lines[kind as usize] = archived_line(trace);
            self.insert_closed(order);
            next_uid += 1;
        }
    }

    /// File one closed order where `market_draw_orders` will find it: the map AND the closed ring.
    fn insert_closed(&mut self, order: RetainedOrder) {
        self.closed_ring.push_back(order.uid);
        self.orders.insert(order.uid, order);
    }
}

/// One archived line as the chart reads it.
///
/// `steps` holds the line's LAST price from its first instant: the chart draws the straight
/// primary line at that price over the order's life and the repricing path beside it from
/// `server_points`, exactly as it does for a live order whose core sent a trace.
fn archived_line(trace: &ArchivedOrderTrace) -> LineTrace {
    let (first_ms, _) = trace.points[0];
    let (_, last_price) = trace.points[trace.points.len() - 1];
    LineTrace {
        steps: vec![(first_ms, last_price)],
        server_points: trace.points.clone(),
        tmp_point: None,
        server_stop_price: trace.stop_price,
        server_stop_time_ms: trace.stop_time_ms,
        off_ms: None,
    }
}

/// A closed, filled order with no lines yet; the caller hangs the archived lines on it.
fn archived_order(
    input: &ArchivedOrdersInput<'_>,
    uid: u64,
    seq: u64,
    create_ms: f64,
    subject: bool,
) -> RetainedOrder {
    RetainedOrder {
        uid,
        market: input.market.to_owned(),
        strat_id: 0,
        strat_name: String::new(),
        is_short: input.is_short,
        emulator: false,
        size: input.quantity,
        remaining_size: 0.0,
        // Filled, so the entry line takes the primary colour rather than the pending one.
        fill_pct: 100.0,
        chart_num: 0,
        pending: false,
        panic_sell: false,
        is_moon_shot: false,
        corridor_price_down: 0.0,
        corridor_price_up: 0.0,
        create_ms,
        // Only the subject's entry ended at the trade's fill; an inherited entry belongs to an
        // ancestor whose fill the archive does not state, so its line runs to the close like a
        // live order the core never dated a fill for.
        entry_fill_ms: if subject { input.entry_fill_ms } else { None },
        closed_ms: Some(input.close_ms),
        closed_reason: Some(OrderCloseReason::Filled),
        closed_store_ms: Some(input.close_ms),
        closed_rev: Some(1),
        last_seen_ms: input.close_ms,
        seen_generation: 0,
        correction_gen: 0,
        seq,
        lines: Default::default(),
        liq: None,
        subject,
    }
}

#[cfg(test)]
mod tests;
