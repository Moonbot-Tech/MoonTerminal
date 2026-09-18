//! Archived order traces of a closed report trade, as the terminal keeps them.
//!
//! The core writes a trade's buy/sell line geometry into an archive when the trade finalizes and
//! answers `request_traces(report_uid)` from it. MoonProto parses the answer into its own types;
//! this module is the moonproto-free projection the session store retains and the trade window
//! draws, in the same spirit as [`super::OrderTrace`] for a live order.
//!
//! The points are the core's CHART geometry, not a stream of live trace messages: one anchor
//! followed by groups of three points per repricing (`docs/reports.md`, "Archived Order Traces").
//! That is exactly the layout `LineTrace::server_points` already holds for a live order, so the
//! chart draws an archived line through the same code as a live one.

use std::sync::Arc;

/// Which line of the order one archived trace describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchivedLineKind {
    /// The entry leg: a plain, stop or limit buy on the wire, all one line on the chart.
    Entry,
    /// The exit leg.
    Exit,
}

/// One own or inherited line from a closed trade's archive.
#[derive(Debug, Clone, PartialEq)]
pub struct ArchivedOrderTrace {
    /// `false` for geometry inherited from another order through a join or a split: the line of
    /// an ancestor trade, kept because the position it opened is the one this trade closed.
    pub own: bool,
    pub kind: ArchivedLineKind,
    /// Stop line the core delivered with the trace (Moonbot `SetStopPrice`), horizontal at one
    /// price from the first point's time to `stop_time_ms`. `None` when the archive holds none.
    pub stop_price: Option<f32>,
    pub stop_time_ms: Option<f64>,
    /// `(Unix UTC ms, price)`, in the core's chart-geometry layout. Unset coordinates (a zero
    /// time or a non-positive price) are already dropped, and a trace that lost every point is
    /// not constructed at all.
    pub points: Vec<(f64, f32)>,
}

/// What one `request_traces` asked for one report row ended in.
#[derive(Debug, Clone, PartialEq)]
pub enum ReportTracesOutcome {
    /// The core answered. An EMPTY set is a real answer — no archive for this trade — and not a
    /// reason to ask again on a timer: the core also uses it for archive-storage failures and may
    /// backfill older trades at its own startup, so a user-driven retry stays allowed.
    Ready(Arc<[ArchivedOrderTrace]>),
    /// The request did not produce an answer: transport failure, the 12-second timeout, or a core
    /// too old to know the command. Not evidence of an empty archive.
    Failed(String),
}

/// Project MoonProto's parsed archive into the terminal's own lines.
///
/// Lines of an unknown order type are dropped rather than guessed at, and so is a line with no
/// usable point: the chart would draw nothing for either, and a retained empty line would make
/// "the archive had lines" true of a picture that shows none. A drop is logged, so an answer that
/// held lines the terminal could not read is not silently mistaken for an empty archive.
///
/// Args:
///     traces: What the core answered, as MoonProto parsed it.
///
/// Returns:
///     The drawable lines, in the archive's own order — own lines first, as the core writes them.
pub fn archived_traces_from_proto(traces: &[moonproto::ReportTrace]) -> Vec<ArchivedOrderTrace> {
    let out: Vec<ArchivedOrderTrace> = traces
        .iter()
        .filter_map(|trace| {
            let kind = match trace.order_type {
                moonproto::OrderType::Sell => ArchivedLineKind::Exit,
                moonproto::OrderType::Buy
                | moonproto::OrderType::BuyStop
                | moonproto::OrderType::BuyLimit => ArchivedLineKind::Entry,
                _ => return None,
            };
            let points: Vec<(f64, f32)> = trace
                .points
                .iter()
                .filter_map(|p| {
                    let time_ms = p.time.unix_millis();
                    (time_ms > 1 && p.price.is_finite() && p.price > 0.0)
                        .then_some((time_ms as f64, p.price as f32))
                })
                .collect();
            if points.is_empty() {
                return None;
            }
            let stop_time_ms = trace.stop_time.unix_millis();
            let has_stop =
                trace.stop_price.is_finite() && trace.stop_price > 0.0 && stop_time_ms > 1;
            Some(ArchivedOrderTrace {
                own: trace.own,
                kind,
                stop_price: has_stop.then_some(trace.stop_price as f32),
                stop_time_ms: has_stop.then_some(stop_time_ms as f64),
                points,
            })
        })
        .collect();
    if out.len() != traces.len() {
        log::warn!(
            "[x] report traces: {} of {} archived lines unreadable (unknown type or no usable point)",
            traces.len() - out.len(),
            traces.len()
        );
    }
    out
}

#[cfg(test)]
mod tests;
