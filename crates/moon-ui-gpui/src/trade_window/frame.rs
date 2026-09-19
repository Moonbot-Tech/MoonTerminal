//! Where a trade window puts its viewport: two pure rules over the trade's own stamps, chosen by
//! the reader — the market around the trade ([`trade_frame`]) or the trade itself
//! ([`fit_frame`]).
//!
//! Separate from `trade_replay::replay_window_ms`, which decides what is FETCHED. Those two answer
//! different questions and must not be confused. The fetch buys context generously and
//! ASYMMETRICALLY - six hours before the entry against two after the exit - because what
//! the market did beforehand is worth more than what it did after. The VIEW cannot inherit that
//! shape: an asymmetric window puts a short trade three quarters of the way to the right, so two
//! trades of different lengths produce two differently composed pictures, which is precisely the
//! complaint this module exists to answer.
//!
//! The rule is deliberately dull, and lives here rather than in `moon-chart` or `moon-core` for
//! the reason `remembered_geometry` and `render::rail_wraps` do: it is a decision this window
//! makes, so it belongs beside this window, where its test can reach it.

/// Fraction of the trade's own duration added as context on EACH side, expressed as a divisor.
///
/// A divisor rather than a multiplier so the arithmetic stays integer-exact on millisecond
/// stamps. `1` asks for the trade's own duration on each side, which puts the trade across a
/// third of the width rather than a half.
///
/// It deliberately EXCEEDS the fetcher's one-half proportional context, and nothing clamps it
/// back - see the note on the frame itself. A half-width trade reads as stretched, so the excess
/// is the point rather than an oversight, and where it runs past the downloaded bars the chart
/// draws empty margin instead of a narrower picture.
///
/// It governs only trades LONGER than the floor below. Everything shorter is decided by that
/// floor, which is why the retunes have all landed there and this constant has not moved.
const CONTEXT_DIVISOR: i64 = 1;

/// Fewest bars of context the window shows on each side of the trade.
///
/// A count of BARS, not a duration, and that is the point. Half of a forty-second scalp is twenty
/// seconds, which at any resolution is no context at all, so a floor is unavoidable; but writing
/// that floor as a fixed number of minutes silently assumes one-minute bars. A replay drawn from
/// individual trades has a far finer resolution and can honestly show a short trade at the same
/// relative size a long one gets, and a fixed floor would deny it that for no reason.
///
/// This floor is what SHORT trades actually get - the proportional arm only wins past it - so it,
/// not the divisor, is the number that decides how a scalp is drawn. It has been retuned twice
/// against the running build, from fifteen to forty-five to ninety, each time because a reader
/// looked at a real scalp and asked for more market around it. The last step was measured rather
/// than guessed: the reader asked for "exactly one more wheel click", and one wheel click is a
/// factor of two (`chartdx::input`, `factor = 2.0`), so the floor doubled.
const MIN_CONTEXT_BARS: i64 = 90;

/// Ceiling on the floor, in milliseconds.
///
/// A short trade's floor is a bar count, and a coarse timeframe would turn ninety bars into days
/// of context for a trade that lasted seconds. This caps that, and ONLY that - it is not a
/// containment bound, see the note on the frame itself.
///
/// It MUST stay above `MIN_CONTEXT_BARS` at the one-minute timeframe the replay pins today, or it
/// silently undoes that floor rather than merely bounding it at coarser resolutions - which is a
/// real trap, not a hypothetical: it has had to move with the floor on both retunes, and a test
/// pins it independently for exactly that reason.
const CONTEXT_CEILING_MS: i64 = 2 * 60 * 60 * 1_000;

/// Frame one trade for viewing: centred, with proportional context.
///
/// Two regimes, and both of them answer "the same everywhere" for the trades they cover. Above
/// the floor the context is the trade's own duration on each side, so the trade occupies a third
/// of the width no matter how long it ran. Below the floor every trade gets the SAME window, so
/// short trades are alike too - they are simply smaller inside it, which at a coarse resolution
/// is a fact about the data rather than a choice this function is free to make: a trade shorter
/// than one bar cannot be drawn as a third of a screen.
///
/// **The frame is NOT clamped to what was fetched, deliberately.** It used to be, and that made
/// the fetch's own margins the real limit on how wide a trade could be shown: every frame
/// collapsed onto the downloaded edges, so the candles could never be drawn thinner however much
/// context was asked for. Widening the fetch instead was considered and rejected by the user -
/// it buys the same picture at the cost of a bigger download on every replay. So the view is
/// allowed to run past the data, and where it does the chart simply draws nothing: a little empty
/// margin beside the trade, which is the accepted price of a readable candle width.
///
/// Args:
///     entry_ms: Position open, in Unix milliseconds.
///     exit_ms: Position close, in Unix milliseconds.
///     bar_ms: Milliseconds one drawn bar covers, taken from the series that actually arrived.
///
/// Returns:
///     The interval to show, or `None` when the stamps cannot describe one.
pub(crate) fn trade_frame(entry_ms: i64, exit_ms: i64, bar_ms: i64) -> Option<(i64, i64)> {
    // A zero-length position is legitimate - a same-second trade, or an ms-exact pair that
    // filled and closed at one instant, arrives with `exit_ms == entry_ms`. It frames on the
    // floor alone, which is the same window every sub-floor trade gets. Only an exit BEFORE the
    // entry is unusable.
    if exit_ms < entry_ms {
        return None;
    }
    let held_ms = exit_ms.saturating_sub(entry_ms);
    // A non-positive bar length means the caller could not say; fall back to the ceiling, which is
    // the widest floor this function may use and therefore the safe answer.
    let floor_ms = if bar_ms > 0 {
        bar_ms
            .saturating_mul(MIN_CONTEXT_BARS)
            .min(CONTEXT_CEILING_MS)
    } else {
        CONTEXT_CEILING_MS
    };
    // A MAXIMUM against the proportional context, never a sum with it - the same discipline the
    // fetch applies to its own floors. Adding them would widen every long trade's frame to buy
    // context it already had, and break the constant-third-width property outright.
    let context_ms = (held_ms / CONTEXT_DIVISOR).max(floor_ms);
    let start_ms = entry_ms.saturating_sub(context_ms);
    let end_ms = exit_ms.saturating_add(context_ms);
    if end_ms <= start_ms {
        return None;
    }
    Some((start_ms, end_ms))
}

/// Fraction of a FITTED frame left empty on each side. Passed to the chart as its own padding
/// argument, so the fitted interval itself stays exactly the trade.
///
/// A quarter on each side puts the trade across HALF the width — one wheel click out from the
/// trade filling it (a click is a factor of two, `chartdx::input`), which is what the reader
/// asked for after seeing the trade edge to edge: "a little smaller, one click each way".
pub(crate) const FIT_PAD: f32 = 0.25;

/// One trade's own span for fitting: from where its entry order was PLACED, when the archive
/// holds that line, to its close.
///
/// The placement is the start of the archived entry line — the first point the core recorded —
/// and it is used only when it precedes the fill it belongs to; an archive stamp later than the
/// fill is noise, and the fill is the floor. Without a line the fill is the start: a market entry
/// was placed and filled in one instant as far as the picture is concerned.
///
/// Args:
///     entry_set_ms: Start of the archived entry line, when there is one.
///     entry_fill_ms: The entry fill, Unix milliseconds.
///     close_ms: The close, Unix milliseconds.
///
/// Returns:
///     `(start, end)`, with `start <= end`.
pub(crate) fn trade_span(
    entry_set_ms: Option<i64>,
    entry_fill_ms: i64,
    close_ms: i64,
) -> (i64, i64) {
    let start = entry_set_ms
        .filter(|set| *set < entry_fill_ms)
        .unwrap_or(entry_fill_ms);
    (start.min(close_ms), close_ms.max(start))
}

/// The start of an archived ENTRY line, from the traces the resolver answered with.
///
/// Args:
///     lines: The trade's archived lines.
///
/// Returns:
///     The earliest point of an own entry line, or `None` when the archive holds none.
pub(crate) fn entry_set_ms(lines: &[moon_core::feed::ArchivedOrderTrace]) -> Option<i64> {
    lines
        .iter()
        .filter(|line| line.own && line.kind == moon_core::feed::ArchivedLineKind::Entry)
        .flat_map(|line| line.points.iter().map(|(t, _)| *t))
        .filter(|t| t.is_finite())
        .map(|t| t as i64)
        .min()
}

/// Frame a trade ON ITSELF: its own span, widened to take in the shown neighbours that sit
/// within one trade-length of it.
///
/// The neighbour rule is the reader's own: a neighbour counts when its span touches the subject's
/// span extended by the subject's held time on each side — so a scalp takes in the scalps beside
/// it and a day-long position the day around it, and nothing further, or one distant trade
/// would stretch the frame until the subject was a sliver. Neighbours are offered only while the
/// window shows them; the caller passes none otherwise.
///
/// No floor and no fixed context: that is what the ordinary [`trade_frame`] is for, and the two
/// are the reader's choice between "the market around the trade" and "the trade".
///
/// Args:
///     subject: The subject's own span, from [`trade_span`].
///     neighbours: The shown neighbours' spans, from [`trade_span`] each.
///
/// Returns:
///     The interval to show, or `None` when the subject's span cannot describe one.
pub(crate) fn fit_frame(
    subject: (i64, i64),
    neighbours: impl IntoIterator<Item = (i64, i64)>,
) -> Option<(i64, i64)> {
    let (start, end) = subject;
    if end < start || start <= 0 {
        return None;
    }
    let reach = end - start;
    let (lo, hi) = (start.saturating_sub(reach), end.saturating_add(reach));
    let (mut from, mut to) = (start, end);
    for (n_start, n_end) in neighbours {
        if n_end < n_start || n_end < lo || n_start > hi {
            continue;
        }
        from = from.min(n_start);
        to = to.max(n_end);
    }
    // A same-instant trade with nothing beside it is a point, and a point cannot be framed; the
    // chart's own minimum width takes over from the ordinary rule instead.
    if to <= from {
        return trade_frame(from, to, 60_000);
    }
    Some((from, to))
}

/// The price band a fitted frame asks the auto-Y fit to include: every price the drawn lines
/// and arrows sit at.
///
/// Args:
///     prices: The subject's — and, while shown and taken in, the neighbours' — entry and exit
///         prices and archived line points.
///
/// Returns:
///     `(min, max)` over the finite positive prices, or `None` when there are none.
pub(crate) fn fit_price_range(prices: impl IntoIterator<Item = f32>) -> Option<(f32, f32)> {
    prices
        .into_iter()
        .filter(|p| p.is_finite() && *p > 0.0)
        .fold(None, |acc: Option<(f32, f32)>, p| match acc {
            None => Some((p, p)),
            Some((lo, hi)) => Some((lo.min(p), hi.max(p))),
        })
}

#[cfg(test)]
mod tests;
