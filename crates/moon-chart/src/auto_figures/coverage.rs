//! Whether the bars of a UTC calendar period cover it: the one check the calendar detectors share
//! (`extremes`, `volume_profile`, `period_levels`).
//!
//! The 5-minute cache is written only while the terminal runs, and the core's ring fills only the
//! hours the core ran, so a period whose start, end or a stretch inside is missing is normal. A
//! level read from part of a period is a different level — a day's high from three of its 288 bars
//! was 1.1 % under the real one on a live cache (BTCUSDT, 19.11.2026) — and a wrong level drawn
//! with confidence is worse than an honest gap.
//!
//! Both sources also hold a bar ONLY where something traded: a quiet market's day has empty
//! 5-minute slots everywhere, and those are not holes — nothing happened in them, so nothing of
//! the period's range is missing. A share of bars present cannot tell the two apart: it rejects a
//! quiet coin's honest day and passes a busy coin's day with a solid two-hour hole (264 of 288
//! bars), where the real high may well sit. What tells them apart is the LENGTH of the empty
//! stretch: a quiet market leaves short ones, a terminal or core that was not running leaves hours
//! in a row. So, with `tf` the bar length of the row:
//!
//! - a period's START is present when a valid bar opens within one bar length of it
//!   ([`opens_at`]) — the period's own first bar, not merely a bar of the input before it;
//! - a FINISHED period's END is present when its last bar's close, one bar length after its open,
//!   reaches it ([`closes_at`]);
//! - a period is COVERED ([`covered`]) when no empty stretch longer than [`MAX_GAP_MS`] lies
//!   anywhere in it: from its start to its first bar, between two neighbouring bars, and from its
//!   last bar's close to its end when it is finished — or, when it is the current one, from that
//!   close up to the caller's reference time, plus the one bar still forming (the rest of the
//!   period has not happened yet). The edges are judged by the same hour as the inside: a quiet
//!   coin's first trade of the day at 00:20 is not a missing start.
//!
//! An open or a close (`period_levels`) needs its exact edge bar ([`opens_at`], [`closes_at`]); a
//! high, a low or a profile needs the whole span (`extremes`, `volume_profile`).
//!
//! Limit: the check sees the bars it is given. On a timeframe coarser than [`MAX_GAP_MS`] (4 h,
//! 1 d) one bar can be resampled from a few 5-minute rows with hours missing inside it, and no gap
//! between bars shows that. The shipped day-extremes row runs on 5-minute bars, where it does.

/// The longest empty stretch, ms, between two neighbouring bars of a period that still counts as
/// a market that did not trade rather than a hole in the record.
///
/// An hour: a quiet coin goes tens of minutes without a trade, while the cache and the ring lose
/// data in whole sessions of the terminal or the core — hours in a row. A longer stretch is where
/// a period's real high, low or volume can sit unseen, and the level drawn without it would be
/// wrong with confidence.
pub const MAX_GAP_MS: f64 = 3_600_000.0;

/// Whether a bar opening at `t_open_ms` is the first bar of a period starting at `start`: it opens
/// at or after the start and within one bar length (`tf`) of it.
pub(crate) fn opens_at(t_open_ms: f64, start: f64, tf: f64) -> bool {
    t_open_ms >= start && t_open_ms < start + tf
}

/// Whether a bar opening at `t_open_ms` is the last bar of a period ending at `end`: its close, one
/// bar length (`tf`) after its open, reaches the end.
pub(crate) fn closes_at(t_open_ms: f64, end: f64, tf: f64) -> bool {
    t_open_ms + tf >= end
}

/// Whether the time from a last bar opening at `last_ms` (its close, `tf` later) up to `reference`
/// is short enough to be a quiet market: [`MAX_GAP_MS`], plus one bar length for the bar after it,
/// which may still be forming and is then rightly absent from closed bars.
fn tail_ok(last_ms: f64, reference: f64, tf: f64) -> bool {
    reference - (last_ms + tf) <= MAX_GAP_MS + tf
}

/// The open time of the last of `opens` (ascending, bars `tf` long) when no two neighbours leave
/// more than [`MAX_GAP_MS`] empty between the first one's close and the second one's open; `None`
/// when one does, or when `opens` is empty.
fn last_without_long_gap(opens: impl IntoIterator<Item = f64>, tf: f64) -> Option<f64> {
    let mut last: Option<f64> = None;
    for t in opens {
        if last.is_some_and(|prev| t - (prev + tf) > MAX_GAP_MS) {
            return None;
        }
        last = Some(t);
    }
    last
}

/// Whether the valid bars of one period — their open times `opens`, ascending — cover the period
/// from `start`: up to `end` when the period is finished (`Some`), up to `reference` (the caller's
/// "now") when it is the current one (`None`). See the module doc.
///
/// `false` for no bars and for a bar length that is not a positive number. Cost O(bars).
pub(crate) fn covered(
    opens: impl IntoIterator<Item = f64>,
    start: f64,
    end: Option<f64>,
    reference: f64,
    tf: f64,
) -> bool {
    if !(tf > 0.0) {
        return false;
    }
    let mut opens = opens.into_iter().peekable();
    // The first bar belongs to the period and leaves at most the allowed stretch before it.
    if !opens
        .peek()
        .is_some_and(|&first| first >= start && first - start <= MAX_GAP_MS)
    {
        return false;
    }
    let Some(last) = last_without_long_gap(opens, tf) else {
        return false;
    };
    match end {
        Some(end) => end - (last + tf) <= MAX_GAP_MS,
        None => tail_ok(last, reference, tf),
    }
}

/// Whether the bars opening at `opens` (ascending, the valid bars from `from` on) cover the
/// open-ended span from `from` up to `reference` well enough to say what traded in it: no empty
/// stretch longer than [`MAX_GAP_MS`] after `from`, between neighbours, or before `reference` (the
/// bar still forming allowed for, as for a current period). Unlike a period, the span has no first
/// bar of its own to require — it begins at a moment, not at a bar — so a span with no bars at all
/// is covered while it is that short: nothing traded in it.
///
/// `false` for a bar length that is not a positive number. Cost O(bars).
pub(crate) fn span_covered(
    opens: impl IntoIterator<Item = f64>,
    from: f64,
    reference: f64,
    tf: f64,
) -> bool {
    if !(tf > 0.0) {
        return false;
    }
    let mut opens = opens.into_iter().peekable();
    let Some(&first) = opens.peek() else {
        return reference - from <= MAX_GAP_MS + tf;
    };
    if first - from > MAX_GAP_MS {
        return false;
    }
    last_without_long_gap(opens, tf).is_some_and(|last| tail_ok(last, reference, tf))
}
