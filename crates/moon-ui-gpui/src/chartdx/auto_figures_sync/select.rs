//! Which of a row's found lines are drawn right now, and which events each one shows.
//!
//! Both answers depend on the CURRENT PRICE — a level's side of price decides whether `take`
//! counts it as above or below, and `BySign` colours it by that side; an approach is a distance to
//! price — so both are re-derived on a price change, not only when a detector run lands. That pass
//! is O(found + kept) with no allocation beyond the kept list, which is why it can follow the
//! price: the detectors themselves never do.
//!
//! Called from two places with the same arguments, so they cannot disagree: the panel's
//! publication of a finished run (`panels::chart::auto_figures`) and the engine's price pass
//! ([`super::ChartDataState::auto_figures_on_price`]).

use moon_chart::auto_figures::events::{EventSet, line_events};
use moon_chart::auto_figures::{Found, FoundShape, take};
use moon_core::config::auto_figures::{BrokenPolicy, EventMarks, Take};

use super::job::AutoRowOutcome;

/// Names one found line across detector runs of its row, so a line the user hid stays hidden when
/// the next bar close recomputes it.
///
/// A run renumbers its figures and nudges their prices — a level's price is the mean of its
/// touches, and one more touch moves it — so neither the index nor the price identifies a line.
/// Its anchors do: the figure's first anchor (`first_ms`) and its first and last touch. For a
/// pivot line the first anchor is the oldest touch; for a period extreme `first_ms` is the
/// period's first bar, which the high and the low SHARE, and the touch is the bar that set each
/// one; two trend lines may share their first pivot (the trend detector allows one shared pivot)
/// and differ in the last. They stay put until they fall out of the row's lookback (or a new
/// extreme or a further touch is found), which is when the line itself is a different one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct LineKey {
    /// The row's index in its kind's configuration.
    pub row_ix: usize,
    /// 0 horizontal, 1 sloped, 2 squeeze: a level and a trend line of one row may share a touch.
    shape: u8,
    /// `first_ms` as bits: compared for identity, never for order.
    first_ms: u64,
    /// The first touch's time as bits; what tells a period's high from its low.
    touch_ms: u64,
    /// The last touch's time as bits; what tells two trend lines from one shared pivot apart.
    last_touch_ms: u64,
}

impl LineKey {
    /// The key of `found`, a figure of row `row_ix`.
    pub(crate) fn of(row_ix: usize, found: &Found) -> Self {
        let shape = match found.shape {
            FoundShape::Horizontal { .. } => 0,
            FoundShape::Sloped { .. } => 1,
            FoundShape::Squeeze { .. } => 2,
        };
        Self {
            row_ix,
            shape,
            first_ms: found.first_ms.to_bits(),
            touch_ms: found.touches.first().map_or(0, |t| t.t_ms.to_bits()),
            last_touch_ms: found.touches.last().map_or(0, |t| t.t_ms.to_bits()),
        }
    }
}

/// One drawn line of a row: which figure, its side of price, and its current events.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct KeptLine {
    /// Index into the row's `outcome.found`.
    pub ix: usize,
    /// Whether the line lies ABOVE the reference price it was selected against — what a `BySign`
    /// colour reads.
    pub above: bool,
    /// The events the row marks that this line shows now.
    pub events: EventSet,
}

/// The rules one row selects and marks its lines by.
#[derive(Debug, Clone, Copy)]
pub(crate) struct RowRules<'a> {
    pub row_ix: usize,
    pub take: &'a Take,
    pub marks: &'a EventMarks,
    pub policy: BrokenPolicy,
    /// Lines the user hid on this pane (`Hide this line`), for every row.
    pub hidden: &'a [LineKey],
}

/// The lines a row draws against `ref_price` at `at_ms`, each with its side and events.
///
/// `take::select` first, then the hidden lines are dropped — a hidden line leaves its slot empty
/// rather than pulling in the next one, so hiding a line never makes a different one appear —
/// and each survivor gets its events from the row's recent closed bars.
///
/// Counter: `autofigures_events` moves by the number of lines whose events were derived.
///
/// Args:
///     outcome: The row's last accepted detector run.
///     rules: The row's selection and event settings.
///     ref_price: The current price; not a positive finite number keeps nothing.
///     at_ms: The moment sloped lines are projected to, Unix ms.
///     scratch: Reused event buffer, so a price change allocates only the result.
pub(crate) fn select_kept(
    outcome: &AutoRowOutcome,
    rules: &RowRules<'_>,
    ref_price: f64,
    at_ms: f64,
    scratch: &mut Vec<EventSet>,
) -> Vec<KeptLine> {
    let picked = take::select(&outcome.found, rules.take, ref_price, at_ms);
    let mut out = Vec::with_capacity(picked.len());
    for ix in picked {
        let Some(found) = outcome.found.get(ix) else {
            continue;
        };
        if rules.hidden.contains(&LineKey::of(rules.row_ix, found)) {
            continue;
        }
        let Some(price) = found.shape.price_at(at_ms) else {
            continue;
        };
        line_events(
            std::slice::from_ref(found),
            &outcome.recent,
            ref_price,
            at_ms,
            rules.marks,
            rules.policy,
            scratch,
        );
        out.push(KeptLine {
            ix,
            above: price >= ref_price,
            events: scratch.first().copied().unwrap_or_default(),
        });
    }
    crate::diag::bump_by(&crate::diag::AUTOFIGURES_EVENTS, out.len() as u64);
    out
}
