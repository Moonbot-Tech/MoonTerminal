//! The Entry/Exit axis's diagnostic report: where a user's sample drops out before the model
//! answers, and where the model misses on what is left — counted, never listed.
//!
//! The model reproduces ~90 % of exits on the developer's cores; a user who reads far less on
//! theirs cannot send the trades that would show why — they are the user's trading. What they
//! can send is this: the funnel from the scope's rows to the judged sample, each core's venue,
//! build and clock against the tape, and per segment (kind × venue × side × the rule the core
//! closed by) the verdicts with the cause of every ✗ and · (`verify::ExitFinding`) and how many
//! trades ran each rule (`verify::RuleFlags`). Two reports — the developer's and the user's —
//! then compare segment by segment.
//!
//! Nothing that identifies a trade or a setting is written: no coin, price, date, size, profit,
//! core or strategy name, id or `reportuid`. Cores are numbered C1…CN; times are milliseconds
//! relative to an event; prices are per cent off another price. The developer's decisions
//! (2026-10-06): rule FLAGS only, never values; aggregates only; the axis's current scope; cores
//! anonymous.
//!
//! The text is a diagnostic artifact, read by the developer like a log, so it is English and
//! stable rather than localized — the button and its notice are the UI's.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use super::Deal;
use super::exit::stops::StopTrigger;
use super::settings::ModelSettings;
use super::unmodelled::UnmodelledField;
use super::verify::{
    EntryFinding, ExitFinding, ExitMiss, QuoteSide, REASON_TAKE, REASONS_LINE, RULE_FLAG_COUNT,
    Unjudged, Verdict, is_stop_reason, reason_starts_with,
};

mod text;

#[cfg(test)]
mod tests;

/// The report format's version, printed in its first line; raised when a line changes meaning.
pub const REPORT_VERSION: u32 = 3;

/// Which rule closed the fact, by the core's `sellreason` — the segment's last key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CloseClass {
    /// The take (`Sell Price`).
    Take,
    /// The moving line (`Auto Price Down`, `Sell Level`).
    Line,
    /// A stop or a trailing stop.
    Stop,
    /// Anything else the tuner's scope admits.
    Other,
}

impl CloseClass {
    /// The class of a `sellreason`, matched as the verdict matches it.
    pub fn of(sell_reason: &str) -> Self {
        let reason = sell_reason.trim();
        if reason.eq_ignore_ascii_case(REASON_TAKE) {
            Self::Take
        } else if REASONS_LINE.iter().any(|r| reason_starts_with(reason, r)) {
            Self::Line
        } else if is_stop_reason(reason) {
            Self::Stop
        } else {
            Self::Other
        }
    }

    /// The name the report prints.
    fn label(self) -> &'static str {
        match self {
            Self::Take => "take",
            Self::Line => "line",
            Self::Stop => "stop",
            Self::Other => "other",
        }
    }
}

/// Where a row's tape stands, as the funnel counts it. The terminal's own status is richer; the
/// caller narrows it to these.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TapeClass {
    /// The tape covers the window; the row has a verdict once the model answered.
    Covered,
    /// Not fetched yet.
    Missing,
    /// The row resolves to no market (its core offline, or the coin unknown).
    NoAddress,
    /// Being fetched.
    Fetching,
    /// The venue refused or holds nothing, under the reason's name (`NoRoute`, `OutOfRetention`).
    Refused(&'static str),
}

impl TapeClass {
    /// The name the report prints.
    fn label(self) -> &'static str {
        match self {
            Self::Covered => "covered",
            Self::Missing => "not fetched",
            Self::NoAddress => "no address",
            Self::Fetching => "fetching",
            Self::Refused(why) => why,
        }
    }
}

/// One row of the axis as the report reads it.
pub struct ReportRow<'a> {
    pub deal: &'a Deal,
    /// The core's venue as the report names it (`Binance Futures`); `None` when the row has no
    /// address.
    pub venue: Option<String>,
    pub tape: TapeClass,
    /// The model's verdict; `None` without tape, and on a covered row the model has not
    /// answered for yet.
    pub verdict: Option<&'a Verdict>,
    /// The fields the trade's strategy switches on outside the model (`unmodelled`), by name.
    pub outside_model: &'a [UnmodelledField],
}

/// What the terminal knows of a core beyond its rows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CoreFacts {
    /// Whether the core is connected and ready now; an offline core reports no build.
    pub connected: bool,
    /// The Moonbot build the core reported (`770` = 7.70).
    pub version: Option<u32>,
    /// The letter reported with [`Self::version`].
    ///
    /// `None` is an older core that sent no letter. `Some("")` is a release. `Some("R3")` is a
    /// named build. Empty is never folded into `None`.
    pub version_suffix: Option<String>,
    /// The core's adopted time zone, seconds east of UTC — the report axis is corrected by it,
    /// and a wrong one shifts every trade off its tape by hours.
    pub tz_offset_secs: Option<i32>,
}

/// Everything the report is built from.
pub struct ReportInput<'a> {
    /// The terminal's build, as its status bar names it.
    pub build: String,
    /// The scope's period in days; `None` for an open period.
    pub period_days: Option<f64>,
    /// Rows the axis dropped before the table: no millisecond stamps, service rows, a kind or an
    /// exit the tuner cannot be run on.
    pub without_ms: usize,
    pub service: usize,
    pub untunable: usize,
    pub rows: Vec<ReportRow<'a>>,
    pub cores: HashMap<u64, CoreFacts>,
    /// The model's settings as the user runs them.
    pub model: ModelSettings,
}

/// Render the report as text.
pub fn render(input: &ReportInput) -> String {
    text::render(input, &aggregate(input))
}

/// One group's ✓ over the trades it is counted over; a trade not judged is in `n` and never a
/// hit, as the axis's accuracy line counts it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Share {
    pub hits: usize,
    pub n: usize,
}

/// The funnel from the scope's rows to the judged sample.
#[derive(Debug, Default)]
pub(super) struct Funnel {
    /// Rows in the table.
    pub rows: usize,
    /// Rows by tape class.
    pub tape: BTreeMap<TapeClass, usize>,
    /// The venues of the rows whose tape is not covered, by class.
    pub tape_venues: BTreeMap<TapeClass, BTreeSet<String>>,
    /// Covered rows whose long position has a hole in its tape.
    pub holed: usize,
    /// Covered rows the model has not answered for yet — in the shares, not in the segments.
    pub unanswered: usize,
    /// ✓ of the entry group over the covered rows whose kind has an entry model.
    pub entry: Share,
    /// ✓ of the exit group over every covered row.
    pub exit: Share,
}

/// One core, numbered.
#[derive(Debug, Default)]
pub(super) struct CoreLine {
    pub uid: u64,
    pub venue: Option<String>,
    pub rows: usize,
    pub covered: usize,
    pub step_lag_ms: f64,
    pub round_trip_ms: Option<f64>,
    /// `verify::fill_clock_ms` of every covered row that had a print at its fill's price.
    pub clock: Vec<i64>,
    /// Covered rows without such a print.
    pub clock_unmatched: usize,
    /// `StopFacts::ticker_age_ms` of every stop whose reason's quote a print matched before the
    /// archived activation, with no hole of the tape in the lookback.
    pub ticker_age: Vec<i64>,
}

/// A segment's key: kind × venue × side × close.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) struct SegKey {
    pub kind: String,
    pub venue: String,
    pub short: bool,
    pub close: CloseClass,
}

/// The entry group's findings in one segment.
#[derive(Debug, Default)]
pub(super) struct EntryTally {
    pub fact: usize,
    pub hits: usize,
    pub unfilled: usize,
    /// Signed deviation of every `Off` fill, per cent of the fact.
    pub off: Vec<f64>,
}

/// The exit group's findings in one segment. A same-rule miss can fail on several parts at
/// once, so `level`, `late` and the two line counts may sum past the misses.
#[derive(Debug, Default)]
pub(super) struct ExitTally {
    pub hits: usize,
    pub misses: usize,
    pub stop_not_fired: usize,
    pub no_level: usize,
    /// Signed deviation of every level miss that had one, per cent of the fact: a modifier sum or
    /// a take shift the model reads wrong leans one way, a price grid's rounding both.
    pub level: Vec<f64>,
    /// Level misses whose deviation could not be formed.
    pub level_no_dev: usize,
    /// Model minus core, ms, of every moment miss.
    pub late: Vec<i64>,
    /// Line misses at the archived take itself.
    pub line_at_take: usize,
    /// Line misses at a later move.
    pub line_later: usize,
    pub unjudged: BTreeMap<&'static str, usize>,
    /// Judged trades whose model closed on a stop, by the rule that fired it.
    pub triggers: BTreeMap<StopTrigger, usize>,
    /// Moment misses by the rule that fired the model's stop: `[early, late]`.
    pub late_by: BTreeMap<StopTrigger, [usize; 2]>,
    /// Model − core, ms, of every judged stop whose activation the archive holds, hit or miss — a
    /// stop the verdict timed against the close is left out (the close trails the activation).
    pub moments: Vec<i64>,
    /// Where the core's reason quote stood against the level, per trade that printed one.
    pub quotes: BTreeMap<QuoteSide, usize>,
}

/// One segment's counts.
#[derive(Debug)]
pub(super) struct Segment {
    pub key: SegKey,
    pub n: usize,
    pub entry: EntryTally,
    pub exit: ExitTally,
    /// Trades per rule flag, in `RuleFlags::named`'s order.
    pub rules: [usize; RULE_FLAG_COUNT],
    /// Trades per field outside the model.
    pub outside: BTreeMap<&'static str, usize>,
}

/// The report's numbers, before the text.
#[derive(Debug, Default)]
pub(super) struct Report {
    pub funnel: Funnel,
    /// In the report's order: most rows first, then by uid.
    pub cores: Vec<CoreLine>,
    /// In the report's order: most ✗ and · first.
    pub segments: Vec<Segment>,
}

/// Count the report's numbers off the rows.
pub(super) fn aggregate(input: &ReportInput) -> Report {
    let mut funnel = Funnel {
        rows: input.rows.len(),
        ..Funnel::default()
    };
    let mut cores: HashMap<u64, CoreLine> = HashMap::new();
    let mut segments: HashMap<SegKey, Segment> = HashMap::new();
    for row in &input.rows {
        let deal = row.deal;
        *funnel.tape.entry(row.tape).or_default() += 1;
        let core = cores.entry(deal.core_uid).or_insert_with(|| CoreLine {
            uid: deal.core_uid,
            ..CoreLine::default()
        });
        core.rows += 1;
        if core.venue.is_none() {
            core.venue.clone_from(&row.venue);
        }
        let venue = row.venue.clone().unwrap_or_else(|| "?".to_string());
        if row.tape != TapeClass::Covered {
            funnel
                .tape_venues
                .entry(row.tape)
                .or_default()
                .insert(venue);
            continue;
        }
        core.covered += 1;
        let Some(verdict) = row.verdict else {
            // Tape and no answer yet: unjudged in both groups, as the accuracy line counts it —
            // the entry only where the kind has an entry model, as the funnel's entry share is.
            funnel.unanswered += 1;
            funnel.exit.n += 1;
            funnel.entry.n += usize::from(super::entry_model_for(&deal.kind));
            continue;
        };
        // Each row carries its core's calibration as of its own replay; a row replayed before
        // the core had one must not wipe the one a later row brought.
        if deal.step_lag_ms > 0.0 {
            core.step_lag_ms = deal.step_lag_ms;
        }
        core.round_trip_ms = deal.round_trip_ms.or(core.round_trip_ms);
        match verdict.fill_clock_ms {
            Some(dt) => core.clock.push(dt),
            None => core.clock_unmatched += 1,
        }
        core.ticker_age.extend(verdict.stop.ticker_age_ms);
        funnel.holed += usize::from(deal.gap.is_some());
        if verdict.entry_finding != EntryFinding::Fact {
            funnel.entry.n += 1;
            funnel.entry.hits += usize::from(verdict.entry == Some(true));
        }
        funnel.exit.n += 1;
        funnel.exit.hits += usize::from(verdict.exit == Some(true));
        let key = SegKey {
            kind: deal.kind.clone(),
            venue,
            short: deal.is_short,
            close: CloseClass::of(&deal.sell_reason),
        };
        let seg = segments.entry(key.clone()).or_insert_with(|| Segment {
            key,
            n: 0,
            entry: EntryTally::default(),
            exit: ExitTally::default(),
            rules: [0; RULE_FLAG_COUNT],
            outside: BTreeMap::new(),
        });
        add(seg, verdict, row.outside_model);
    }
    let mut cores: Vec<CoreLine> = cores.into_values().collect();
    cores.sort_by(|a, b| b.rows.cmp(&a.rows).then(a.uid.cmp(&b.uid)));
    let mut segments: Vec<Segment> = segments.into_values().collect();
    segments.sort_by(|a, b| {
        let off = |s: &Segment| s.n - s.exit.hits;
        off(b)
            .cmp(&off(a))
            .then(b.n.cmp(&a.n))
            .then(a.key.cmp(&b.key))
    });
    Report {
        funnel,
        cores,
        segments,
    }
}

/// Count one verdict into its segment.
fn add(seg: &mut Segment, verdict: &Verdict, outside: &[UnmodelledField]) {
    seg.n += 1;
    match verdict.entry_finding {
        EntryFinding::Fact => seg.entry.fact += 1,
        EntryFinding::Hit => seg.entry.hits += 1,
        EntryFinding::Unfilled => seg.entry.unfilled += 1,
        // An `Off` fill always carries its deviation; one that could not form it still counts.
        EntryFinding::Off => seg
            .entry
            .off
            .push(verdict.entry_dev_pct.unwrap_or(f64::NAN)),
    }
    let exit = &mut seg.exit;
    match verdict.exit_finding {
        ExitFinding::Hit => exit.hits += 1,
        ExitFinding::Miss(miss) => {
            exit.misses += 1;
            match miss {
                ExitMiss::StopNotFired => exit.stop_not_fired += 1,
                ExitMiss::NoLevel => exit.no_level += 1,
                ExitMiss::Off(parts) => {
                    if parts.level {
                        match verdict.exit_dev_pct {
                            Some(dev) => exit.level.push(dev),
                            None => exit.level_no_dev += 1,
                        }
                    }
                    if let Some(late) = parts.late_ms {
                        exit.late.push(late);
                        if let Some(trigger) = verdict.stop.trigger {
                            exit.late_by.entry(trigger).or_default()[usize::from(late >= 0)] += 1;
                        }
                    }
                    match parts.first_unmatched {
                        Some(0) => exit.line_at_take += 1,
                        Some(_) => exit.line_later += 1,
                        None => {}
                    }
                }
            }
        }
        ExitFinding::Unjudged(why) => {
            *exit.unjudged.entry(unjudged_label(why)).or_default() += 1;
        }
    }
    // The stop facts count over judged trades only: an unjudged one says nothing about the rules.
    if verdict.exit.is_some() {
        if let Some(trigger) = verdict.stop.trigger {
            *exit.triggers.entry(trigger).or_default() += 1;
        }
        exit.moments.extend(verdict.stop.moment_ms);
    }
    // The quote is the fact's own, whatever the model made of the trade.
    if let Some(quote) = verdict.stop.quote {
        *exit.quotes.entry(quote).or_default() += 1;
    }
    for (i, (_, on)) in verdict.rules.named().iter().enumerate() {
        seg.rules[i] += usize::from(*on);
    }
    for field in outside {
        *seg.outside.entry(field.key).or_default() += 1;
    }
}

/// The name the report prints for an unjudged exit.
fn unjudged_label(why: Unjudged) -> &'static str {
    use super::exit::UnmodelledRule;
    match why {
        Unjudged::Rule(UnmodelledRule::SellShot) => "rule SellShot",
        Unjudged::Rule(UnmodelledRule::SellSpread) => "rule SellSpread",
        Unjudged::Rule(UnmodelledRule::NoAutoSell) => "rule AutoSell off",
        Unjudged::InGap => "in tape hole",
        Unjudged::TakeUnknown => "take unknown",
        Unjudged::OtherRule => "closed by another rule",
    }
}
