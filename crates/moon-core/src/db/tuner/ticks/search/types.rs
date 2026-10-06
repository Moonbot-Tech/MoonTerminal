//! types items for tick searches.

use super::*;

/// Passes of coordinate descent one restart may take before it is called converged, when the
/// caller does not say ([`SearchParams::max_passes`]).
pub const DEFAULT_MAX_PASSES: usize = 16;

/// One deal with its tape, ready to be replayed as often as the search asks.
#[derive(Clone)]
pub struct PreparedDeal {
    pub deal: Deal,
    /// The window's prints, ascending; shared, never copied per evaluation.
    pub ticks: Arc<[Tick]>,
    /// The archived points of the entry line, when the archive holds it; shared like the
    /// tape.
    pub entry_line: Option<Arc<[(i64, f64)]>>,
    /// How far past the close the HELD COVERAGE of this deal's window reaches, in
    /// milliseconds — the caller's word from the tile store, not the last print's stamp: a
    /// quiet market prints nothing for seconds, and a tail measured by its last print would
    /// read as shorter than what is actually held. What [`common_horizon_ms`] takes the
    /// sample's horizon from; a covered row holds at least the model's tail
    /// (`required_spans`), so it is never under `TAIL_MS` there.
    pub trail_ms: i64,
    /// The values the deal's own strategy holds now, in strategy spelling — the base every
    /// variant and every point of a search is laid over on THIS deal. Shared by the deals of
    /// one strategy; empty when the strategy could not be read, and then every field reads as
    /// default.
    pub own: Arc<HashMap<String, String>>,
}

/// Cut every deal's tape at the same distance past its close — the exit horizon the whole
/// sample is judged on.
///
/// The tapes of a sample were captured under different margins (the setting moves; a close
/// filed under 5 min sits beside one filed under 30 s), and a variant judged on each deal's OWN
/// tape end is judged unevenly: on the long tape it gets minutes to reach its take, on the
/// short one seconds, and a variant that outlives the tape drops out of the tally
/// (`OpenAtWindowEnd`) — the long tapes then flatter every slow exit. One horizon for all,
/// the shortest trail among them, is the only fair comparison the sample allows; a deal a
/// variant has not closed by then still drops out, but now every deal drops out at the same
/// distance. The decision of 2026-09-20.
///
/// Args:
///     deals: The prepared sample; a deal whose tape already ends at or before the horizon is
///         left untouched.
///     horizon_ms: The horizon past each close, from [`common_horizon_ms`].
pub fn clip_to_horizon(deals: &mut [PreparedDeal], horizon_ms: i64) {
    for deal in deals.iter_mut() {
        let end_ms = deal.deal.close_ms.saturating_add(horizon_ms.max(0));
        let keep = deal.ticks.partition_point(|t| (t.time_ms as i64) <= end_ms);
        if keep < deal.ticks.len() {
            deal.ticks = Arc::from(&deal.ticks[..keep]);
        }
    }
}

/// The exit horizon a sample allows: the shortest held trail past the close among its deals
/// ([`PreparedDeal::trail_ms`]), or `None` for an empty sample. See [`clip_to_horizon`].
pub fn common_horizon_ms(deals: &[PreparedDeal]) -> Option<i64> {
    deals.iter().map(|d| d.trail_ms.max(0)).min()
}

/// What one search varies and how.
pub struct SearchParams<'a> {
    /// Values held over every deal's own base ([`PreparedDeal::own`]) before the point is laid
    /// on — the axis passes the variant's edits for every search, so the fields it leaves alone
    /// run at what the earlier searches found. The fields it varies are not held: their values
    /// here are set aside, and each starts from the strategies, or from its grid step where a
    /// strategy holds it off the grid (`pinned`; [`SearchResult::searched`]).
    /// Empty searches from the strategies as they stand.
    pub held: &'a HashMap<String, String>,
    /// Schema defaults for the keys a deal's base leaves out.
    pub defaults: &'a HashMap<String, f64>,
    /// The strategy kind of the sample, for the fields the grid offers.
    pub kind: &'a str,
    /// Whether the Entry group is searched (only when the kind has an entry model).
    pub vary_entry: bool,
    /// Whether the Exit group is searched.
    pub vary_exit: bool,
    /// Field keys held at each deal's base value.
    pub locked: &'a HashSet<String>,
    /// Each number field's candidate values (`params::range::resolve`); a number field without
    /// one has nothing to try and is not varied.
    pub grids: &'a Grids,
    /// Restart count, at least 1.
    pub restarts: usize,
    /// Minimum trades a point must keep, or half the train deals.
    pub min_n: Option<i64>,
    /// Base seed of the restarts; `None` draws one from the clock.
    pub seed: Option<u64>,
    /// Share of the period, oldest first, the search may fit on.
    pub train_frac: f64,
    /// Passes of coordinate descent per restart, at least 1.
    pub max_passes: usize,
    /// The model's own settings, the entry method among them.
    pub model: ModelSettings,
    /// Whether a point whose entry corridor comes nearer the price than a trade's own, at any
    /// moment of that trade's entry order, is out of the search
    /// ([`MshotParams::never_closer_than`]). Read only while the Entry group is searched: a
    /// search of the exit alone moves no corridor.
    pub keep_corridor: bool,
    /// How much riskier than the fact an answer may be, on the deals it is fitted on: a point
    /// past a limit is refused ([`risk`]).
    pub risk: RiskLimits,
    /// Whether a search of both groups screens the entry moves of each step under the exit found
    /// so far and runs a whole exit descent under the best few only ([`screen`]). Read only when
    /// both groups are searched.
    pub screen_entry: bool,
}

/// Why a search came back with nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SearchMiss {
    /// Nothing to search — no deals, no field to vary — or the run was stopped before its
    /// first restart finished.
    Nothing,
    /// No point the search visited kept `min_n` trades.
    Floor,
    /// No point the search visited kept a corridor it may propose: `MShotPriceMin` below
    /// `MShotPrice` where the strategy had them so ([`MshotParams::is_ordered`]), and, under
    /// [`SearchParams::keep_corridor`], every trade's corridor at least as far from the price as
    /// the trade's own.
    Corridor,
    /// No point the search visited kept its max drawdown and win rate within the risk limits
    /// of the fact ([`SearchParams::risk`]).
    Risk,
    /// No point the search visited closed every deal it bought inside the tape with something
    /// standing to close each trade — a stop, or a trailing without a take profit ([`closing`]).
    Unclosed,
    /// The scored set holds fewer than [`MIN_SEARCH_DEALS`] deals: whatever a search fits on it
    /// is noise, so none is run ([`sample_floor`]).
    TooFew {
        /// Deals the set holds.
        n: usize,
    },
}

/// How a search went — what shows whether its restarts and passes changed anything.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SearchStats {
    /// Restarts that ran to the end (a stop leaves the rest out).
    pub restarts: usize,
    /// The restart the answer came from: 0 starts from the strategy itself, but for the fields
    /// pinned on their grids ([`pinned`]).
    pub best_restart: usize,
    /// Passes of coordinate descent the winning restart took.
    pub passes: usize,
    /// Whether the winning restart stopped because a pass changed nothing — `false` means it
    /// was still improving when the pass limit cut it.
    pub converged: bool,
    /// Distinct end points among the restarts: 1 means every restart ended at the same point.
    pub distinct: usize,
    /// Points scored over the whole run, each a replay of the training slice.
    pub evaluations: usize,
    /// Restarts that ended on a point the corridor rules or the closing rule (`closing`) refuse
    /// — none of their moves reached an allowed one.
    pub refused: usize,
    /// Deals the strategies as they stand leave open inside the tape, taken out of the sample
    /// before the search (`closing::closable_at_base`).
    pub left_open: usize,
    /// Entry points scored by a whole search of the exit under them — a search of both groups
    /// ([`nested`]); zero for a search of one.
    pub entry_points: usize,
    /// Points scored on the entry fills of a point scored before ([`fills`]) — the exit replayed
    /// alone.
    pub fills_reused: usize,
}

/// What the search found.
#[derive(Clone, Debug)]
pub struct SearchResult {
    /// The winning values, in strategy spelling — only the fields that moved off the base of
    /// at least one deal.
    pub values: Vec<(String, String)>,
    /// The fields the search varied, sorted: each one's answer is in `values`, or it is at the
    /// strategies' own value where that value is on the field's grid ([`pinned`]) — never at what
    /// the held edits had it at, which the search set aside.
    pub searched: Vec<String>,
    /// What they achieve on the deals they were fitted on.
    pub train: Tally,
    /// What they achieve on the deals held back, when any were.
    pub holdout: Option<Tally>,
    /// How many of the deals held back the answer bought and left open inside the tape — none
    /// may be among the deals it was fitted on; the holdout is only scored, so it says them.
    pub holdout_open: usize,
    /// What the `holdout_open` deals would make closed at the last print of their tapes
    /// ([`VariantScore::open_profit`]) — an estimate beside `holdout`, never part of it.
    pub holdout_open_profit: f64,
    /// The fact over the deals the search was fitted on ([`fact_tally`]).
    pub fact_train: Tally,
    /// The fact over the deals held back, when any were — the slice `holdout` is scored on.
    pub fact_holdout: Option<Tally>,
    /// Whether the answer loses to the fact on the holdout: it leaves a held-back deal open, or
    /// its holdout profit is below the fact's there.
    pub holdout_loses: bool,
    /// The seed the restarts were derived from.
    pub seed: u64,
    /// How the run went.
    pub stats: SearchStats,
}
