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
    /// The exit as the REPORT states it, for the line the archive may not hold. The core archives
    /// a line only once its chart gave it a point, so a trade that closed within a second, or one
    /// whose exit never moved, answers with no exit line — and no archive is not no line: the
    /// order was placed at this price at this instant and filled at the close. Both builders draw
    /// that straight line themselves when the traces carry no own exit line. An own exit line in
    /// the traces wins with its repricing path, but it is the sell ORDER's movement, and the
    /// position closed at this price: the line stands here, and a path that ends elsewhere — a
    /// stop or a panic sale by market, which leaves no price on the order — is carried to it at
    /// the close ([`own_exit_line`]). `None` skips both (a row with no exit price to place).
    pub exit: Option<ReportExit>,
    /// The entry as the REPORT states it, for the line the archive may not hold: the core files an
    /// entry line only once the order moved, so an order that stood at its price from creation to
    /// fill answers with none — and the report's `BuySetDateMs` still says where it began. Both
    /// builders draw that straight line from the placement to the fill when the traces carry no
    /// own entry line; an own entry line in the traces wins. `None` skips the fallback (a row
    /// without a placement, one placed at its fill).
    pub entry: Option<ReportEntry>,
    /// Whether [`OrderLineStore::append_archived`] draws these lines at the ACTIVE opacity rather
    /// than the closed one. The trade window's neighbours are context and stay pale; a live
    /// chart in the "Moonbot lines" style draws its closed trades the way Moonbot does, in full
    /// colour. [`OrderLineStore::archived`] ignores it: there the subject is bright and the rest
    /// pale by construction.
    pub bright: bool,
}

/// The exit order as the report row records it: where a trade's exit line is placed when the
/// core archived none for it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ReportExit {
    /// Exit price the row settled at.
    pub price: f32,
    /// When the exit order was created (`SellSetDate`), in Unix UTC ms. The exit line runs from
    /// here to the trade's close; a placement dated after the close — a seconds-only stamp raised
    /// past a millisecond close — collapses to a point at the close rather than running backwards.
    pub set_ms: f64,
}

impl ReportExit {
    /// The exit of one report row, lifted onto true UTC through the report axis — the same lift
    /// its arrows and its close take, so the line starts where the exit arrow would have stood.
    ///
    /// Args:
    ///     record: The closed trade's row.
    ///     axis: The report axis that lifts the core's clock onto UTC.
    ///
    /// Returns:
    ///     The exit to fall back on; `None` for a row that settled at no price.
    pub fn of_record(
        record: &crate::db::ChartTradeRecord,
        axis: &crate::db::ReportAxis,
    ) -> Option<Self> {
        (record.sell_price > 0.0).then(|| Self {
            price: record.sell_price as f32,
            set_ms: axis.stamp_to_utc_ms(record.sell_set_stamp(), record.core_uid) as f64,
        })
    }
}

/// The entry order as the report row records it: where a trade's entry line is placed when the
/// core archived none for it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ReportEntry {
    /// Entry price the row filled at — the level an order that never moved stood at.
    pub price: f32,
    /// When the entry order was created (`BuySetDateMs`), in Unix UTC ms. The line runs from here
    /// to the order's entry fill ([`ArchivedOrdersInput::entry_fill_ms`]).
    pub set_ms: f64,
}

impl ReportEntry {
    /// The entry of one report row, lifted onto true UTC through the report axis — the same lift
    /// its arrows take, so the line ends where the entry arrow would have stood.
    ///
    /// Args:
    ///     record: The closed trade's row.
    ///     axis: The report axis that lifts the core's clock onto UTC.
    ///
    /// Returns:
    ///     The entry to fall back on; `None` for a row without a placement stamp, one whose
    ///     placement is not strictly before its fill (a market entry, or a stamp no order can
    ///     have), or one with no entry price.
    pub fn of_record(
        record: &crate::db::ChartTradeRecord,
        axis: &crate::db::ReportAxis,
    ) -> Option<Self> {
        let set = record.buy_set_stamp()?;
        let set_ms = axis.stamp_to_utc_ms(set, record.core_uid);
        let fill_ms = axis.stamp_to_utc_ms(record.buy_stamp(), record.core_uid);
        (record.buy_price > 0.0 && set_ms < fill_ms).then_some(Self {
            price: record.buy_price as f32,
            set_ms: set_ms as f64,
        })
    }
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
                order.lines[kind as usize] = match kind {
                    LineKind::Sell => own_exit_line(line, input.exit, input.close_ms),
                    _ => line,
                };
                continue;
            }
            let uid = next_uid;
            next_uid += 1;
            let mut order = archived_order(&input, uid, seq as u64, first_ms, false);
            order.lines[kind as usize] = line;
            store.insert_closed(order);
        }
        // No own exit line in the archive: the report's straight exit line joins the subject —
        // beside its archived entry line when there is one, alone when the archive was empty.
        if let Some(exit) = input.exit.filter(|_| !has_own_exit(traces)) {
            let (start_ms, line) = report_exit_line(exit, input.close_ms);
            let order = subject.get_or_insert_with(|| {
                archived_order(&input, SUBJECT_UID, traces.len() as u64, start_ms, true)
            });
            order.create_ms = order.create_ms.min(start_ms);
            order.lines[LineKind::Sell as usize] = line;
        }
        // No own entry line in the archive: the report's straight entry line joins the subject,
        // from the placement to the fill the subject is dated with.
        if let Some(entry) = input.entry.filter(|_| !has_own_entry(traces)) {
            let order = subject.get_or_insert_with(|| {
                archived_order(&input, SUBJECT_UID, traces.len() as u64, entry.set_ms, true)
            });
            order.create_ms = order.create_ms.min(entry.set_ms);
            order.lines[LineKind::Buy as usize] = report_entry_line(entry);
        }
        if let Some(order) = subject {
            store.insert_closed(order);
        }
        store.rev = 1;
        store
    }

    /// Add another closed trade's archived lines to this store, pale unless `input.bright`.
    ///
    /// For the trade window's "other trades": the neighbours of the subject drawn beside it, each
    /// asked from the core by its own `ReportUID`. None of them is the subject, so none takes the
    /// active opacity — own and inherited alike, they are context. The live chart's lines style
    /// takes the same builder with `bright` set: there the closed trades ARE the picture. Uids
    /// continue past whatever the store already holds, so two trades can never collide.
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
        // Its ONE sale belongs to its first own exit line, likewise.
        let mut sale_taken = false;
        for (seq, trace) in traces.iter().enumerate() {
            let Some(&(first_ms, _)) = trace.points.first() else {
                continue;
            };
            let kind = match trace.kind {
                ArchivedLineKind::Entry => LineKind::Buy,
                ArchivedLineKind::Exit => LineKind::Sell,
            };
            // Built as a non-subject — that flag also dates the entry fill, which only the first
            // own entry line below may carry — and then lit up when the caller asked for it.
            let mut order =
                archived_order(&input, next_uid, seq_base + seq as u64, first_ms, false);
            order.subject = input.bright;
            if trace.own && kind == LineKind::Buy && !fill_taken {
                fill_taken = true;
                order.entry_fill_ms = input.entry_fill_ms;
            }
            let mut line = archived_line(trace);
            if trace.own && kind == LineKind::Sell && !sale_taken {
                sale_taken = true;
                line = own_exit_line(line, input.exit, input.close_ms);
            }
            order.lines[kind as usize] = line;
            self.insert_closed(order);
            next_uid += 1;
        }
        // Same fallback as `archived`, as an order of its own like every other line here.
        if let Some(exit) = input.exit.filter(|_| !has_own_exit(traces)) {
            let (start_ms, line) = report_exit_line(exit, input.close_ms);
            let mut order = archived_order(
                &input,
                next_uid,
                seq_base + traces.len() as u64,
                start_ms,
                false,
            );
            order.subject = input.bright;
            order.lines[LineKind::Sell as usize] = line;
            self.insert_closed(order);
            next_uid += 1;
        }
        // And the entry's, dated with the row's one fill — which no own entry line took, since
        // the fallback applies only when there is none.
        if let Some(entry) = input.entry.filter(|_| !has_own_entry(traces)) {
            let mut order = archived_order(
                &input,
                next_uid,
                seq_base + traces.len() as u64 + 1,
                entry.set_ms,
                false,
            );
            order.subject = input.bright;
            order.entry_fill_ms = input.entry_fill_ms;
            order.lines[LineKind::Buy as usize] = report_entry_line(entry);
            self.insert_closed(order);
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
/// `steps` holds the line's LAST price from its first instant (the trade's own exit then moves to
/// the sale's, [`own_exit_line`]): the chart draws the straight
/// primary line at that price over the order's life and the repricing path beside it from
/// `server_points`, exactly as it does for a live order whose core sent a trace.
fn archived_line(trace: &ArchivedOrderTrace) -> LineTrace {
    // The one place the archive's `f64` prices narrow to the chart's `f32`: the store is drawn,
    // never persisted, so nothing downstream loses what the local archive keeps.
    let server_points: Vec<(f64, f32)> = trace
        .points
        .iter()
        .map(|&(time_ms, price)| (time_ms, price as f32))
        .collect();
    let (first_ms, _) = server_points[0];
    let (_, last_price) = server_points[server_points.len() - 1];
    LineTrace {
        steps: vec![(first_ms, last_price)],
        server_points,
        tmp_point: None,
        server_stop_price: trace.stop_price.map(|price| price as f32),
        server_stop_time_ms: trace.stop_time_ms,
        off_ms: None,
    }
}

/// The trade's own archived exit line ended where the trade sold. The trace is the sell order's
/// movement; the report's sell price is where the position closed. The primary line stands at the
/// sell price, and a trace whose last point is elsewhere — the take a stop sold past by market —
/// is carried to it at the close, so its path runs from the last placement to the sale.
///
/// The trace is Moonbot's `SetPointTrade` layout — an anchor, then one group of three points per
/// move (`order_geometry` draws it so) — and the sale goes in as one more move: held at the last
/// price to the close, then down to the sale there. A trace not in that layout is left as it is,
/// since a point appended to it would be read off its grid; its primary line still moves.
///
/// Args:
///     line: The line as [`archived_line`] built it.
///     exit: The report's exit; `None` leaves the line as the archive has it.
///     close_ms: The trade's close, where the sale is drawn.
fn own_exit_line(mut line: LineTrace, exit: Option<ReportExit>, close_ms: f64) -> LineTrace {
    let Some(exit) = exit else {
        return line;
    };
    let Some(&(last_ms, last_price)) = line.server_points.last() else {
        return line;
    };
    // The two prices come down different pipes — the report's settled sell against the wire's
    // order price — so "the same" is the store's own float-jitter bound, not bit equality.
    let sold_elsewhere = (last_price - exit.price).abs() > super::price_eps(exit.price);
    if sold_elsewhere && line.server_points.len() % 3 == 1 {
        let at_ms = close_ms.max(last_ms);
        line.server_points.extend([
            (at_ms, last_price),
            (at_ms, exit.price),
            (at_ms, exit.price),
        ]);
    }
    for step in &mut line.steps {
        step.1 = exit.price;
    }
    line
}

/// Whether the traces carry the trade's OWN exit line with at least one point — the line that
/// makes the report's fallback unnecessary. An inherited exit belongs to an ancestor.
fn has_own_exit(traces: &[ArchivedOrderTrace]) -> bool {
    traces
        .iter()
        .any(|trace| trace.own && trace.kind == ArchivedLineKind::Exit && !trace.points.is_empty())
}

/// Whether the traces carry the trade's OWN entry line with at least one point — the line that
/// makes the report's entry fallback unnecessary.
fn has_own_entry(traces: &[ArchivedOrderTrace]) -> bool {
    traces
        .iter()
        .any(|trace| trace.own && trace.kind == ArchivedLineKind::Entry && !trace.points.is_empty())
}

/// The entry line the report states: straight at the entry price from the order's placement, with
/// no repricing path — the order never moved, or the core would have archived its line. Its end is
/// the order's `entry_fill_ms`, which the builders date it with.
fn report_entry_line(entry: ReportEntry) -> LineTrace {
    LineTrace {
        steps: vec![(entry.set_ms, entry.price)],
        server_points: Vec::new(),
        tmp_point: None,
        server_stop_price: None,
        server_stop_time_ms: None,
        off_ms: None,
    }
}

/// The exit line the report states: straight at the exit price from the exit order's placement
/// to the close, with no repricing path — nothing moved, or the core kept nothing of it.
///
/// Returns:
///     The line's start (the order's `create_ms` when it stands alone) and the line itself.
fn report_exit_line(exit: ReportExit, close_ms: f64) -> (f64, LineTrace) {
    let start_ms = exit.set_ms.min(close_ms);
    let line = LineTrace {
        steps: vec![(start_ms, exit.price)],
        server_points: Vec::new(),
        tmp_point: None,
        server_stop_price: None,
        server_stop_time_ms: None,
        off_ms: None,
    };
    (start_ms, line)
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
