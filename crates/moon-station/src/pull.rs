//! What a terminal pulls from the station (STATION.md §4.9): the recorder's tape around closed
//! trades, the order traces archived at each close, and the market-list prices around trades.
//!
//! All of them are reads of the station's own files and need nothing of the main loop, so the
//! API's thread answers them itself ([`answer_directly`]): a terminal pulling a month of tape does
//! not hold up the cores' drain. An answer never outgrows a frame — the tape and the prices stop
//! at [`TAPE_REPLY_BUDGET`], the price listing at [`PRICE_INDEX_BUDGET`], and say where to go on.

use moon_core::db::order_traces::TraceEntry;
use moon_core::market::price_recorder::remote as prices;
use moon_core::market::trade_replay::trade_cache::{StoredSpan, TapeFile, encode_prints};
use moon_core::station_api::{
    Answer, MAX_TAPE_ITEMS, MAX_TAPE_SPANS, MAX_TRACE_UIDS, PRICE_INDEX_BUDGET, Reply, Request,
    TAPE_REPLY_BUDGET, Tape, TapePiece, TapeResume, TapeWant, Traces, TradeTraces,
};

/// JSON around one piece's prints: its item, bounds and field names.
const PIECE_OVERHEAD: usize = 96;

/// The requests the API's thread answers without the main loop; `None` for the rest.
pub fn answer_directly(request: &Request) -> Option<Reply> {
    Some(
        match request {
            Request::TapeFetch { items } => tape(items),
            Request::TracesFetch {
                core_uid,
                report_uids,
            } => traces(*core_uid, report_uids),
            Request::PricesIndex { since_ms, after } => {
                prices::index(*since_ms, after.as_ref(), PRICE_INDEX_BUDGET)
                    .map(Answer::PriceIndex)
                    .map_err(|e| format!("list the prices: {e}"))
            }
            Request::PricesFetch { items } => check_wants(items).and_then(|()| {
                prices::fetch(items, TAPE_REPLY_BUDGET)
                    .map(Answer::Prices)
                    .map_err(|e| format!("read the prices: {e}"))
            }),
            _ => return None,
        }
        .into(),
    )
}

/// The recorder's tape inside the wanted stretches.
fn tape(items: &[TapeWant]) -> Result<Answer, String> {
    check_wants(items)?;
    let path = moon_core::config::paths::tape_recorder_db_path();
    // No file yet: the recorder has filed nothing, which is an answer, not a failure.
    if !path.exists() {
        return Ok(Answer::Tape(Tape::default()));
    }
    let file = TapeFile::open(&path).map_err(|e| format!("open the tape: {e}"))?;
    let read = |exchange: &str, market: &str, from_ms, to_ms| {
        file.spans(exchange, market, from_ms, to_ms)
            .map(|spans| spans.map(|span| span.map_err(|e| format!("read the tape: {e}"))))
            .map_err(|e| format!("read the tape of {exchange} {market}: {e}"))
    };
    Ok(Answer::Tape(answer_tape(items, TAPE_REPLY_BUDGET, read)?))
}

/// Refuse a request past the API's bounds or with a stretch that is not one.
fn check_wants(items: &[TapeWant]) -> Result<(), String> {
    if items.len() > MAX_TAPE_ITEMS {
        return Err(format!(
            "{} markets in one request, at most {MAX_TAPE_ITEMS}",
            items.len()
        ));
    }
    for want in items {
        if want.spans.len() > MAX_TAPE_SPANS {
            return Err(format!(
                "{} stretches of {} in one request, at most {MAX_TAPE_SPANS}",
                want.spans.len(),
                want.market
            ));
        }
        if let Some((from, to)) = want.spans.iter().find(|(from, to)| from > to) {
            return Err(format!(
                "{}: the stretch {from}..{to} is reversed",
                want.market
            ));
        }
    }
    Ok(())
}

/// Answer `items` from what `read` holds, at most `budget` bytes of prints.
///
/// A held span inside a wanted stretch becomes one piece, cut to the stretch. A piece that does
/// not fit is cut at a millisecond boundary — every print of its last millisecond included, so the
/// covered stretch it claims holds all its prints — and the answer says to go on from the next
/// one. The first piece of an answer always goes out, at least its first millisecond, so a
/// client that asks again always gets further.
///
/// Args:
///     items: The wanted stretches.
///     budget: Most bytes of packed prints, JSON overhead included.
///     read: `(exchange, market, from_ms, to_ms)` → lazy held spans intersecting it, ascending;
///         only spans through the page's cut are requested, and read errors are returned.
pub(crate) fn answer_tape<I: Iterator<Item = Result<StoredSpan, String>>>(
    items: &[TapeWant],
    budget: usize,
    mut read: impl FnMut(&str, &str, i64, i64) -> Result<I, String>,
) -> Result<Tape, String> {
    let mut tape = Tape::default();
    let mut used = 0usize;
    for (item, want) in items.iter().enumerate() {
        let item = item as u32;
        for &(from, to) in &want.spans {
            for span in read(&want.exchange, &want.market, from, to)? {
                let span = span?;
                let (a, b) = (from.max(span.from_ms), to.min(span.to_ms));
                if a > b {
                    continue;
                }
                let ticks: Vec<_> = span
                    .ticks
                    .into_iter()
                    .filter(|t| (a..=b).contains(&(t.time_ms as i64)))
                    .collect();
                let prints = encode_prints(&ticks);
                if used + prints.len() + PIECE_OVERHEAD <= budget {
                    used += prints.len() + PIECE_OVERHEAD;
                    tape.pieces.push(TapePiece {
                        item,
                        from_ms: a,
                        to_ms: b,
                        prints,
                    });
                    continue;
                }
                let room = budget.saturating_sub(used + PIECE_OVERHEAD);
                match fitting_prefix(&ticks, room, tape.pieces.is_empty(), prints.len()) {
                    Some((cut_ms, prints)) => {
                        tape.pieces.push(TapePiece {
                            item,
                            from_ms: a,
                            to_ms: cut_ms,
                            prints,
                        });
                        tape.resume = Some(TapeResume {
                            item,
                            from_ms: cut_ms + 1,
                        });
                    }
                    None => {
                        tape.resume = Some(TapeResume { item, from_ms: a });
                    }
                }
                return Ok(tape);
            }
        }
    }
    Ok(tape)
}

/// A budgeted run of whole milliseconds from `ticks`, estimated from the already packed span.
/// Verify the actual packing and shrink on overshoot: compression is not additive per print.
/// Page cuts may leave headroom, but every covered millisecond remains complete. The first
/// millisecond still goes out when forced, so repeated pulls always make progress.
fn fitting_prefix(
    ticks: &[moon_core::feed::types::Tick],
    room: usize,
    forced: bool,
    whole_bytes: usize,
) -> Option<(i64, String)> {
    let first = ticks.first()?.time_ms as i64;
    let first_end = ticks.partition_point(|tick| tick.time_ms as i64 == first);
    let estimate = ticks.len().saturating_mul(room) / whole_bytes.max(1);
    let mut end = (estimate.saturating_mul(95) / 100).clamp(first_end, ticks.len());
    // Round down without splitting a millisecond; the very first group is the progress floor.
    while end > first_end
        && end < ticks.len()
        && ticks[end - 1].time_ms as i64 == ticks[end].time_ms as i64
    {
        end -= 1;
    }
    loop {
        let prints = encode_prints(&ticks[..end]);
        if prints.len() <= room || (forced && end == first_end) {
            return Some((ticks[end - 1].time_ms as i64, prints));
        }
        if end == first_end {
            return None;
        }
        let estimate = end.saturating_mul(room) / prints.len();
        end = (estimate.saturating_mul(95) / 100)
            .max(first_end)
            .min(end - 1);
        while end > first_end && ticks[end - 1].time_ms as i64 == ticks[end].time_ms as i64 {
            end -= 1;
        }
    }
}

/// The archived traces of these trades of one core; a trade without lines is left out.
fn traces(core_uid: u64, report_uids: &[i64]) -> Result<Answer, String> {
    if report_uids.len() > MAX_TRACE_UIDS {
        return Err(format!(
            "{} trades in one request, at most {MAX_TRACE_UIDS}",
            report_uids.len()
        ));
    }
    let held = moon_core::db::order_traces::read_many(core_uid, report_uids)
        .map_err(|e| format!("read the traces: {e:?}"))?;
    Ok(Answer::Traces(answer_traces(
        report_uids,
        &held,
        TAPE_REPLY_BUDGET,
    )))
}

/// The traces of `report_uids` in their order, as far as `budget` bytes of JSON go: the answer
/// says how many it covered, and the client asks again for the rest. The first trade always goes
/// out, so asking again gets further; one alone past the budget is left out with a warning — its
/// frame would be refused whole.
pub(crate) fn answer_traces(
    report_uids: &[i64],
    held: &std::collections::HashMap<i64, TraceEntry>,
    budget: usize,
) -> Traces {
    let mut out = Traces::default();
    let mut used = 0usize;
    for &report_uid in report_uids {
        if let Some(TraceEntry::Lines(lines)) = held.get(&report_uid) {
            let trade = TradeTraces {
                report_uid,
                lines: lines.iter().map(Into::into).collect(),
            };
            let size = serde_json::to_vec(&trade).map_or(usize::MAX, |json| json.len());
            if used + size > budget {
                if out.answered > 0 {
                    return out;
                }
                log::warn!(
                    "api: the traces of trade {report_uid} ({size} bytes) do not fit one answer; left out"
                );
            } else {
                used += size;
                out.trades.push(trade);
            }
        }
        out.answered += 1;
    }
    out
}

#[cfg(test)]
mod tests;
