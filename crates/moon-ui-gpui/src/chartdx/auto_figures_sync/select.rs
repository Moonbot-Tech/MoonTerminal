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

use moon_chart::auto_figures::confluence::{ConfluenceScratch, PlacedLine, confluence_in};
use moon_chart::auto_figures::events::{EventSet, line_events};
use moon_chart::auto_figures::{Found, FoundShape, LineTag, take};
use moon_core::config::auto_figures::{AutoEvent, BrokenPolicy, EventMarks, Take};

use super::AutoRowDrawn;
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
///
/// Anchors are not enough for the detectors whose lines share them. A volume profile's POC, VAH
/// and VAL all span the profiled day and have no touches; the day's open and the week's open are
/// the same bar on a Monday. Those lines carry a [`LineTag`], and the tag is part of the key — it
/// is what the line IS, and it does not move between runs, while their price does (a growing day
/// moves its POC). Round prices have neither anchors nor a tag, only their price, which is exact
/// and fixed — so a horizontal line with no touches and no tag is keyed by its price, in `f32`
/// bits: wide enough apart that no two round levels collide (they are at least a tenth of the
/// price's order of magnitude apart), and immune to the last-digit noise of the same level
/// computed from a different order of magnitude.
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
    /// What the line is, for the detectors that draw several kinds of line from one anchor; a
    /// naked POC is keyed as the POC it is ([`key_tag`]).
    tag: Option<LineTag>,
    /// The price as `f32` bits for a horizontal line with no touches and no tag (a round price);
    /// `0` for every other line, whose identity is in its anchors and tag.
    price: u32,
}

impl LineKey {
    /// The key of `found`, a figure of row `row_ix`.
    pub(crate) fn of(row_ix: usize, found: &Found) -> Self {
        let shape = match found.shape {
            FoundShape::Horizontal { .. } => 0,
            FoundShape::Sloped { .. } => 1,
            FoundShape::Squeeze { .. } => 2,
        };
        let price = match found.shape {
            FoundShape::Horizontal { price } if found.touches.is_empty() && found.tag.is_none() => {
                (price as f32).to_bits()
            }
            _ => 0,
        };
        Self {
            row_ix,
            shape,
            first_ms: found.first_ms.to_bits(),
            touch_ms: found.touches.first().map_or(0, |t| t.t_ms.to_bits()),
            last_touch_ms: found.touches.last().map_or(0, |t| t.t_ms.to_bits()),
            tag: found.tag.map(key_tag),
            price,
        }
    }
}

/// The tag as it enters a [`LineKey`]: what the line IS, without what it currently SHOWS.
///
/// A previous day's POC turns naked when price leaves it and plain again when price comes back —
/// the volume profile retags the same line between runs (`LineTag::NakedPoc` ↔ `LineTag::Poc`),
/// so a key with the raw tag would lose a hidden POC to that flip and draw it again. Both are
/// "the POC of that day"; the day itself is told apart by `first_ms`, its first bar.
fn key_tag(tag: LineTag) -> LineTag {
    match tag {
        LineTag::NakedPoc => LineTag::Poc,
        other => other,
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

/// The working buffers of the confluence pass ([`mark_confluence`]), kept by the published set so
/// a price change allocates nothing for it once they have grown.
#[derive(Debug, Default)]
pub(crate) struct ConfluenceBufs {
    /// Every kept line of every row, placed at the moment judged.
    placed: Vec<PlacedLine>,
    /// For each of `placed`, which row (position in the set) and which of its kept lines.
    owners: Vec<(usize, usize)>,
    /// The pass's answer, one flag per `placed` line.
    near: Vec<bool>,
    scratch: ConfluenceScratch,
}

/// Insert [`AutoEvent::Confluence`] into the events of every kept line that lies within its own
/// row's `confluence_pct` of a kept line of ANOTHER row on a DIFFERENT timeframe — for the rows
/// that mark it (`docs-internal/AUTO_FIGURES.md` §13; the rule is
/// `moon_chart::auto_figures::confluence`'s).
///
/// Judged across the rows of ONE pane, which is the only place every timeframe's lines meet: no
/// detector run sees another row. Every kept line takes part as a partner, whether or not its own
/// row marks the event — a 4h level is a place the 1h row agrees with even when the 4h row shows
/// no mark of its own. Hidden lines are not kept, so they neither get nor give a mark.
///
/// Cost: O(n log n) in the kept lines of the pane, nothing at all when no row marks the event,
/// and no allocation once `bufs` has grown.
///
/// Args:
///     rows: The pane's rows, in their published order.
///     kept: Each row's kept lines, parallel to `rows`; the events are written here.
///     at_ms: The moment sloped lines are projected to, Unix ms — the same one their selection
///         used.
///     bufs: Reused working buffers.
pub(crate) fn mark_confluence(
    rows: &[AutoRowDrawn],
    kept: &mut [Vec<KeptLine>],
    at_ms: f64,
    bufs: &mut ConfluenceBufs,
) {
    if !rows.iter().any(|r| r.marks.marks(AutoEvent::Confluence)) {
        return;
    }
    bufs.placed.clear();
    bufs.owners.clear();
    for (i, (row, lines)) in rows.iter().zip(kept.iter()).enumerate() {
        for (k, line) in lines.iter().enumerate() {
            let Some(price) = row
                .outcome
                .found
                .get(line.ix)
                .and_then(|f| f.shape.price_at(at_ms))
            else {
                continue;
            };
            bufs.placed.push(PlacedLine {
                row: row.row_ix,
                tf: row.outcome.key.tf,
                price,
                pct: row.marks.confluence_pct,
            });
            bufs.owners.push((i, k));
        }
    }
    confluence_in(&bufs.placed, &mut bufs.scratch, &mut bufs.near);
    for (&(i, k), &near) in bufs.owners.iter().zip(&bufs.near) {
        if near
            && rows[i].marks.marks(AutoEvent::Confluence)
            && let Some(line) = kept.get_mut(i).and_then(|l| l.get_mut(k))
        {
            line.events.insert(AutoEvent::Confluence);
        }
    }
}
