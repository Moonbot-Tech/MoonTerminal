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
//! ([`super::ChartDataState::auto_figures_on_price`]). Both select every row of a pane at once
//! ([`select_pane`]) and then run the same cross-row pass over the selections,
//! [`mark_confluence`].
//!
//! One formation two rows both KEEP is drawn once, and that is decided among the rows' selections
//! ([`select_pane`]): each row's `take` first, then the twin under the less precise name leaves
//! its row, which takes its next line in the slot. Twins follow the selections, so they are
//! re-derived with them on a price change — at a cost of O(K²) in the few formations the rows keep.

use moon_chart::auto_figures::confluence::{ConfluenceScratch, PlacedLine, confluence_in};
use moon_chart::auto_figures::events::{EventSet, line_events};
use moon_chart::auto_figures::twins::{TwinRow, select_rows};
use moon_chart::auto_figures::{Found, FoundShape, LineTag, PairSide, take};
use moon_core::config::auto_figures::{AutoEvent, BrokenPolicy, EventMarks, Take};

use super::job::AutoRowOutcome;
use super::{AutoRowDrawn, RowTwins};

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
///
/// A two-line formation (`FoundShape::Pair`) is ONE key — hiding it hides both sides — and it is
/// keyed by the first touch of EACH side (in the two touch slots) rather than by its merged touch
/// list: one more swing on either side moves the formation's last touch on every bar close, while
/// the bar each side starts at stays until the pattern itself changes. Its tag is keyed as its
/// family ([`key_tag`]): a flag that outgrows the flag cap becomes the channel it is, a channel
/// whose sides start converging a triangle — the same stretch, the same formation to a reader who
/// hid it. A double top and a double bottom stay apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct LineKey {
    /// The row's index in its kind's configuration.
    pub row_ix: usize,
    /// 0 horizontal, 1 sloped, 2 squeeze, 3 pair: a level and a trend line of one row may share a
    /// touch.
    shape: u8,
    /// `first_ms` as bits: compared for identity, never for order.
    first_ms: u64,
    /// The first touch's time as bits; what tells a period's high from its low. For a pair, its
    /// upper side's first touch.
    touch_ms: u64,
    /// The last touch's time as bits; what tells two trend lines from one shared pivot apart. For a
    /// pair, its lower side's first touch.
    last_touch_ms: u64,
    /// What the line is, for the detectors that draw several kinds of line from one anchor; a
    /// naked POC is keyed as the POC it is ([`key_tag`]).
    tag: Option<LineTag>,
    /// The price as `f32` bits for a horizontal line with no touches and no tag (a round price);
    /// `0` for every other line, whose identity is in its anchors and tag.
    price: u32,
}

impl LineKey {
    /// Whether this is a two-line formation's or a double's key (`FoundShape::Pair`).
    pub(crate) fn is_formation(&self) -> bool {
        self.shape == 3
    }

    /// Whether `self` and `other` name the same formation, whichever row read it and under
    /// whichever name of its family: the first touch of each side and the family
    /// ([`key_tag`]), the twin identity `moon_chart::auto_figures::twins` uses. Not the first
    /// anchor: a flag's reaches back to its pole, the channel reading of the same sides' does not.
    /// Only keys of rows on one timeframe are compared ([`select_pane`]); `false` for anything but
    /// a formation.
    pub(crate) fn same_formation(&self, other: &Self) -> bool {
        self.is_formation()
            && other.is_formation()
            && self.touch_ms == other.touch_ms
            && self.last_touch_ms == other.last_touch_ms
            && self.tag == other.tag
    }

    /// The key of `found`, a figure of row `row_ix`.
    pub(crate) fn of(row_ix: usize, found: &Found) -> Self {
        let shape = match found.shape {
            FoundShape::Horizontal { .. } => 0,
            FoundShape::Sloped { .. } => 1,
            FoundShape::Squeeze { .. } => 2,
            FoundShape::Pair { .. } => 3,
        };
        let (touch_ms, last_touch_ms) = match found.shape {
            FoundShape::Pair { upper, lower } => (upper.t0_ms.to_bits(), lower.t0_ms.to_bits()),
            _ => (
                found.touches.first().map_or(0, |t| t.t_ms.to_bits()),
                found.touches.last().map_or(0, |t| t.t_ms.to_bits()),
            ),
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
            touch_ms,
            last_touch_ms,
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
///
/// A two-line formation is retagged between runs too: a flag past the flag cap is a channel, a
/// channel whose width starts shrinking a triangle or a wedge. All four are keyed as `Channel`,
/// "the two-line formation on that stretch" (see [`LineKey`]); a double keeps its own tag.
fn key_tag(tag: LineTag) -> LineTag {
    match tag {
        LineTag::NakedPoc => LineTag::Poc,
        LineTag::Triangle | LineTag::Wedge | LineTag::Flag => LineTag::Channel,
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
    /// For a two-line formation, the side it was selected by — the one nearer the reference price
    /// (`FoundShape::near_side`); `None` for every other shape. What the line's caption, event
    /// mark and confluence read (`FoundShape::side_price_at`), so all of them name the boundary
    /// `take` kept it by and agree with its `above`.
    pub side: Option<PairSide>,
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
    /// The formations the user hid through ANOTHER row on this row's timeframe: a hidden
    /// formation hides every reading of it on that timeframe ([`LineKey::same_formation`]), so the
    /// channel row's reading cannot take the place of the flag the user hid. Empty for a row
    /// selected on its own.
    pub hidden_twins: &'a [LineKey],
    /// What the twin pass decided for this row's kept lines ([`select_pane`]): which show the
    /// joined marks of their twins' rows.
    pub twins: &'a RowTwins,
}

impl RowRules<'_> {
    /// The marks found line `ix` shows: its row's own, or — for a formation that other rows keep
    /// too — the union with theirs ([`RowTwins::marks`]).
    pub(crate) fn marks_of(&self, ix: usize) -> &EventMarks {
        self.twins.marks_of(ix).unwrap_or(self.marks)
    }

    /// Whether the user hid `found`, a line of this row: its own key, or a formation hidden
    /// through another row on this row's timeframe.
    fn hides(&self, found: &Found) -> bool {
        let key = LineKey::of(self.row_ix, found);
        self.hidden.contains(&key) || self.hidden_twins.iter().any(|k| k.same_formation(&key))
    }
}

/// The found lines a row keeps against `ref_price` at `at_ms`, ranked: `take::select_skipping`
/// with `skip` out of the running (the twin pass's refill, [`select_pane`]), then the hidden lines
/// dropped — a hidden line leaves its slot empty rather than pulling in the next one, so hiding a
/// line never makes a different one appear.
///
/// Args:
///     outcome: The row's last accepted detector run.
///     rules: The row's selection settings.
///     ref_price: The current price; not a positive finite number keeps nothing.
///     at_ms: The moment sloped lines are projected to, Unix ms.
///     skip: Found lines out of the running, `take::select_skipping`'s contract.
pub(crate) fn pick(
    outcome: &AutoRowOutcome,
    rules: &RowRules<'_>,
    ref_price: f64,
    at_ms: f64,
    skip: &[bool],
) -> Vec<usize> {
    let mut picked = take::select_skipping(&outcome.found, rules.take, ref_price, at_ms, skip);
    picked.retain(|&ix| outcome.found.get(ix).is_some_and(|f| !rules.hides(f)));
    picked
}

/// The lines `picked` (a row's kept found lines, [`pick`]) draw, each with its side of
/// `ref_price` and its events from the row's recent closed bars, under the marks it shows
/// ([`RowRules::marks_of`]).
///
/// Counter: `autofigures_events` moves by the number of lines whose events were derived.
///
/// Args:
///     outcome: The row's last accepted detector run.
///     rules: The row's event settings and the marks its twins joined.
///     picked: The kept found lines, ranked.
///     ref_price: The price they were kept against.
///     at_ms: The moment sloped lines are projected to, Unix ms.
///     scratch: Reused event buffer, so a price change allocates only the result.
pub(crate) fn kept_lines(
    outcome: &AutoRowOutcome,
    rules: &RowRules<'_>,
    picked: &[usize],
    ref_price: f64,
    at_ms: f64,
    scratch: &mut Vec<EventSet>,
) -> Vec<KeptLine> {
    let mut out = Vec::with_capacity(picked.len());
    for &ix in picked {
        let Some(found) = outcome.found.get(ix) else {
            continue;
        };
        // The side `take` measured a pair by; every other shape has one line.
        let side = found.shape.near_side(at_ms, ref_price);
        let Some(price) = found.shape.side_price_at(at_ms, side) else {
            continue;
        };
        line_events(
            std::slice::from_ref(found),
            &outcome.recent,
            ref_price,
            at_ms,
            rules.marks_of(ix),
            rules.policy,
            scratch,
        );
        out.push(KeptLine {
            ix,
            above: price >= ref_price,
            side,
            events: scratch.first().copied().unwrap_or_default(),
        });
    }
    crate::diag::bump_by(&crate::diag::AUTOFIGURES_EVENTS, out.len() as u64);
    out
}

/// Every row of one pane selected at once, into `kept` (one list per row, overwritten), with one
/// formation two rows both KEEP drawn once; returns what the twin pass decided for each row.
///
/// The order is `moon_chart::auto_figures::twins::select_rows`'s, which the screener's scan
/// follows too, so the two agree on what a market draws: each row's own selection first ([`pick`],
/// hidden lines and formations hidden through another row included), then among the formations so
/// kept the same pair of sides ("Flags" and "Channels" both keeping one flag) is drawn once, under
/// the more precise name, `Flag > Wedge = Triangle > Channel`, the upper row on a tie; the row that
/// lost its twin takes its next line once, and the survivor shows the union of the marks of every
/// row of its set (`twins::union_marks`), so dropping the twin never drops an event its row asked
/// for. A formation the more precise row does NOT keep — its side, its `count` — stays with the
/// row that does, under that row's name. A dropped twin is not drawn, so it is no confluence
/// partner and no target of the hover card, "Hide this line" or "Pin" (they hit-test the drawn
/// lines only).
///
/// Runs on a publication and on every price change (a row's selection follows the price, so its
/// twins do too). The twin part costs O(K²) in the K formations the rows keep — a few per row —
/// plus one more `take` for a row that lost a twin; nothing walks every row's found lines
/// pairwise, and the few small vectors it allocates live for the pass.
///
/// Args:
///     rows: The pane's rows, in their published order.
///     hidden: Lines the user hid on this pane.
///     ref_price: The reference price of row `i` (the price pass hands every row the same one).
///     at_ms: The moment sloped lines are projected to, Unix ms.
///     scratch: Reused event buffer.
///     kept: Each row's drawn lines, parallel to `rows`.
pub(crate) fn select_pane(
    rows: &[AutoRowDrawn],
    hidden: &[LineKey],
    ref_price: &dyn Fn(usize) -> f64,
    at_ms: f64,
    scratch: &mut Vec<EventSet>,
    kept: &mut Vec<Vec<KeptLine>>,
) -> Vec<RowTwins> {
    let tf_of = |row_ix: usize| {
        rows.iter()
            .find(|r| r.row_ix == row_ix)
            .map(|r| r.outcome.key.tf)
    };
    // Per row: the formations hidden through another row of the same timeframe.
    let hidden_twins: Vec<Vec<LineKey>> = rows
        .iter()
        .map(|row| {
            hidden
                .iter()
                .filter(|k| {
                    k.row_ix != row.row_ix
                        && k.is_formation()
                        && tf_of(k.row_ix) == Some(row.outcome.key.tf)
                })
                .copied()
                .collect()
        })
        .collect();
    let no_twins = RowTwins::default();
    fn rules<'a>(
        row: &'a AutoRowDrawn,
        hidden: &'a [LineKey],
        hidden_twins: &'a [LineKey],
        twins: &'a RowTwins,
    ) -> RowRules<'a> {
        RowRules {
            row_ix: row.row_ix,
            take: &row.take,
            marks: &row.marks,
            policy: row.policy,
            hidden,
            hidden_twins,
            twins,
        }
    }
    let twin_rows: Vec<TwinRow<'_>> = rows
        .iter()
        .map(|row| TwinRow {
            tf: row.outcome.key.tf,
            found: &row.outcome.found,
            marks: &row.marks,
        })
        .collect();
    let picks = select_rows(&twin_rows, |i, skip| {
        let rules = rules(&rows[i], hidden, &hidden_twins[i], &no_twins);
        pick(&rows[i].outcome, &rules, ref_price(i), at_ms, skip)
    });
    kept.clear();
    let mut out = Vec::with_capacity(rows.len());
    for (i, pick) in picks.into_iter().enumerate() {
        let twins = RowTwins { marks: pick.joined };
        kept.push(kept_lines(
            &rows[i].outcome,
            &rules(&rows[i], hidden, &hidden_twins[i], &twins),
            &pick.kept,
            ref_price(i),
            at_ms,
            scratch,
        ));
        out.push(twins);
    }
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
/// detector run sees another row. A line's "own row" here is the marks it shows (`twins`, the
/// selection's [`select_pane`] answer, over the row's own): a formation whose twin another row kept
/// carries that row's confluence mark too. Every kept line takes part as a partner, whether or not its own
/// row marks the event — a 4h level is a place the 1h row agrees with even when the 4h row shows
/// no mark of its own. Hidden lines are not kept, so they neither get nor give a mark. A two-line
/// formation takes part at the side it was kept by ([`KeptLine::side`]), its far side not at all —
/// one placed price per kept line.
///
/// Cost: O(n log n) in the kept lines of the pane, nothing at all when no row marks the event,
/// and no allocation once `bufs` has grown.
///
/// Args:
///     rows: The pane's rows, in their published order.
///     kept: Each row's kept lines, parallel to `rows`; the events are written here.
///     twins: What the selection's twin pass decided for each row, parallel to `rows`.
///     at_ms: The moment sloped lines are projected to, Unix ms — the same one their selection
///         used.
///     bufs: Reused working buffers.
pub(crate) fn mark_confluence(
    rows: &[AutoRowDrawn],
    kept: &mut [Vec<KeptLine>],
    twins: &[RowTwins],
    at_ms: f64,
    bufs: &mut ConfluenceBufs,
) {
    let marks_of = |i: usize, ix: usize| -> &EventMarks {
        twins
            .get(i)
            .and_then(|t| t.marks_of(ix))
            .unwrap_or(&rows[i].marks)
    };
    let marks_confluence = |i: usize| {
        rows[i].marks.marks(AutoEvent::Confluence)
            || twins
                .get(i)
                .is_some_and(|t| t.marks.iter().any(|(_, m)| m.marks(AutoEvent::Confluence)))
    };
    if !(0..rows.len()).any(marks_confluence) {
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
                .and_then(|f| f.shape.side_price_at(at_ms, line.side))
            else {
                continue;
            };
            bufs.placed.push(PlacedLine {
                row: row.row_ix,
                tf: row.outcome.key.tf,
                price,
                pct: marks_of(i, line.ix).confluence_pct,
            });
            bufs.owners.push((i, k));
        }
    }
    confluence_in(&bufs.placed, &mut bufs.scratch, &mut bufs.near);
    for (&(i, k), &near) in bufs.owners.iter().zip(&bufs.near) {
        if !near {
            continue;
        }
        if let Some(line) = kept.get_mut(i).and_then(|l| l.get_mut(k))
            && marks_of(i, line.ix).marks(AutoEvent::Confluence)
        {
            line.events.insert(AutoEvent::Confluence);
        }
    }
}
