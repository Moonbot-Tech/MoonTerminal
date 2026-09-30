//! What a terminal pulls from the station (STATION.md §4.9): the recorder's tape around closed
//! trades, and the order traces archived at each close.
//!
//! Both are reads of the station's own files and need nothing of the main loop, so the API's
//! thread answers them itself ([`answer_directly`]): a terminal pulling a month of tape does not
//! hold up the cores' drain. An answer never outgrows a frame — the tape stops at
//! [`TAPE_REPLY_BUDGET`] and says where to go on.

use moon_core::db::order_traces::TraceEntry;
use moon_core::market::trade_replay::trade_cache::{StoredSpan, TapeFile, encode_prints};
use moon_core::station_api::{
    Answer, MAX_TAPE_ITEMS, MAX_TAPE_SPANS, MAX_TRACE_UIDS, Reply, Request, TAPE_REPLY_BUDGET,
    Tape, TapePiece, TapeResume, TapeWant, Traces, TradeTraces,
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
///     read: `(exchange, market, from_ms, to_ms)` → the held spans intersecting it, ascending.
pub(crate) fn answer_tape(
    items: &[TapeWant],
    budget: usize,
    mut read: impl FnMut(&str, &str, i64, i64) -> Result<Vec<StoredSpan>, String>,
) -> Result<Tape, String> {
    let mut tape = Tape::default();
    let mut used = 0usize;
    for (item, want) in items.iter().enumerate() {
        let item = item as u32;
        for &(from, to) in &want.spans {
            for span in read(&want.exchange, &want.market, from, to)? {
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
                match fitting_prefix(&ticks, room, tape.pieces.is_empty()) {
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

/// The longest run of whole milliseconds from the start of `ticks` whose packing fits `room`:
/// the last millisecond it covers and the packing. `None` when not even the first millisecond
/// fits — unless `forced`, when the first millisecond goes out whatever its size.
fn fitting_prefix(
    ticks: &[moon_core::feed::types::Tick],
    room: usize,
    forced: bool,
) -> Option<(i64, String)> {
    // Every index where a new millisecond starts, and the end: a prefix ends at one of them.
    let mut ends: Vec<usize> = (1..ticks.len())
        .filter(|&i| ticks[i].time_ms as i64 != ticks[i - 1].time_ms as i64)
        .collect();
    ends.push(ticks.len());
    let packed = |end: usize| encode_prints(&ticks[..end]);
    // The largest end whose packing fits, by bisection: the packing grows with every print.
    let (mut lo, mut hi) = (0usize, ends.len());
    while lo < hi {
        let mid = (lo + hi) / 2;
        if packed(ends[mid]).len() <= room {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    let end = match lo {
        0 if forced && !ticks.is_empty() => ends[0],
        0 => return None,
        n => ends[n - 1],
    };
    Some((ticks[end - 1].time_ms as i64, packed(end)))
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
