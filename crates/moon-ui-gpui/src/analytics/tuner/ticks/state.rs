//! State of the "Entry/Exit" axis: the scope's deals with what the terminal holds for each
//! (tape covered or not, the model's verdict on the fact), the KPI of the whole scope beside
//! the KPI of the replayable subset, the "now" values of the parameter grid, and the queue of
//! tape fetches the user asked for.
//!
//! Split from the rendering (`ticks/mod.rs`) like every other axis: the load paths write here,
//! the render path only reads.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use gpui::Entity;
use moon_ui::MoonInputState;

use super::tape::{PackedTape, PendingDeal};
use crate::load_state::LoadState;
use moon_core::db::tuner::VarStats;
use moon_core::db::tuner::threshold_search::SearchHandle;
use moon_core::db::tuner::ticks::params::range::{FieldSpan, TickRange};
use moon_core::db::tuner::ticks::params::{ParamGroup, ParamSection};
use moon_core::db::tuner::ticks::search::SearchResult;
use moon_core::db::tuner::ticks::{Deal, Verdict, fit_for_search};
use moon_core::market::trade_replay::TickStatus;

/// Share of hits a group needs before it may be searched, per cent, when the search settings do
/// not say: a model that cannot reproduce the fact must not be asked what would have been
/// better. The spec's proposal, to be tuned by practice (`TicksState::gate_pct`).
pub(in crate::analytics::tuner) const DEFAULT_GATE_PCT: u32 = 80;

/// Bytes of tape kept in memory across every fit row, for the variants and the search — what
/// four million prints took before they were packed ([`PackedTape`]), which now holds three times
/// as many. Past it a row is still "covered" — the model ran on it — but its tape is let go and
/// the row sits out of the variant columns; the caption says how many.
pub(in crate::analytics::tuner) const MAX_RETAINED_BYTES: usize = 4_000_000 * 24;

/// What the terminal holds for one deal's window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::analytics::tuner) enum TapeStatus {
    /// The prints cover the window: the model ran.
    Covered,
    /// Nothing, or a hole, inside the window; a fetch may fill it.
    Missing,
    /// No way to ask: the core is not connected, or the coin resolves to no market of its
    /// catalog. The fetch button skips such rows.
    NoAddress,
    /// A fetch is in flight for this row.
    Fetching,
    /// A fetch answered without covering the window; the venue's own word on why.
    Refused(TickStatus),
}

/// One deal of the table with everything the replay learned about it.
#[derive(Clone, Debug)]
pub(in crate::analytics::tuner) struct DealRow {
    pub(in crate::analytics::tuner) deal: Deal,
    pub(in crate::analytics::tuner) tape: TapeStatus,
    /// The fact reproduced, or `None` while the tape is missing.
    pub(in crate::analytics::tuner) verdict: Option<Verdict>,
    /// `(exchange_key, market)` of the deal's core, resolved at load time; `None` is
    /// [`TapeStatus::NoAddress`].
    pub(in crate::analytics::tuner) address: Option<Arc<RowAddress>>,
    /// The window's prints, packed, kept for the variants and the search while the row is fit
    /// and the memory cap allows; `None` otherwise.
    pub(in crate::analytics::tuner) ticks: Option<PackedTape>,
    /// The archived points of the trade's own entry line, when the archive holds it —
    /// where the order stood before the tape begins, and the core's first moves.
    pub(in crate::analytics::tuner) entry_line: Option<Arc<[(i64, f64)]>>,
    /// What the terminal HOLDS of the window, as `(lead_ms, trail_ms)`: how far before the
    /// entry and past the exit the held coverage reaches, clipped to what the window asks for
    /// (the setting's margin, floored for the model). `None` until the tape stage answered, or
    /// when it holds nothing. The table's "tape" column; the exit horizon of the sample is the
    /// shortest trail among the replayable rows.
    pub(in crate::analytics::tuner) held: Option<(i64, i64)>,
}

impl DealRow {
    /// Whether the variants and the search run on this row: its tape covers the window, holds
    /// the shortest tail past the close (`tail`) and the model reproduced it (`fit_for_search`).
    /// The table shows every row; this is the sample.
    pub(in crate::analytics::tuner) fn fit(&self) -> bool {
        self.tape == TapeStatus::Covered
            && self.verdict.as_ref().is_some_and(fit_for_search)
            && super::tail::holds(self.held)
    }

    /// Whether the row is fit but holds no tape — one the memory cap let go under a wider scope.
    /// Carried into a new scope it would stay tapeless: stage C reads only the rows it was not
    /// handed, so the narrower selection it now fits in would never get it back. A report reload
    /// is the same scope, where the cap would only drop it again; a scope reload re-reads it.
    pub(in crate::analytics::tuner) fn lost_tape(&self) -> bool {
        self.fit() && self.ticks.is_none()
    }

    /// Take everything a replay learned about this row from its answer: the tape's word, the
    /// verdict, the model inputs derived for the deal (the price step, the archived pre-spike
    /// ask and take, where the entry order was placed, the core's step lag, what the fact proves
    /// about the stop, the entry the trade ran with, the live deltas), the prints, the entry line
    /// and the held coverage.
    /// Every fold of a replay answer goes through here: the variants replay the STORED row
    /// (`prepared_deals`), so a take lifted to the archive's pre-spike ask in the verdict but
    /// read off the tape in the variants puts the two on different levels, and a row folded
    /// without its held coverage reads a trail of 0 and clips every variant tape at its close
    /// (`common_horizon_ms` is the shortest trail of the sample).
    pub(in crate::analytics::tuner) fn take_replay(&mut self, answer: DealRow) {
        self.tape = answer.tape;
        self.verdict = answer.verdict;
        self.deal.tick = answer.deal.tick;
        self.deal.pre_spike_ask = answer.deal.pre_spike_ask;
        self.deal.archived_take = answer.deal.archived_take;
        self.deal.entry_placed = answer.deal.entry_placed;
        self.deal.step_lag_ms = answer.deal.step_lag_ms;
        self.deal.stop_anchor = answer.deal.stop_anchor;
        self.deal.own_entry = answer.deal.own_entry;
        self.deal.delta_track = answer.deal.delta_track;
        self.deal.bars = answer.deal.bars;
        self.ticks = answer.ticks;
        self.entry_line = answer.entry_line;
        self.held = answer.held;
    }
}

/// Where a deal's prints live, as the replay worker keys them, plus what a fetch needs.
#[derive(Clone, Debug)]
pub(in crate::analytics::tuner) struct RowAddress {
    pub(in crate::analytics::tuner) core_uid: u64,
    /// The venue the core is connected to — decides the public trade route and its retention.
    pub(in crate::analytics::tuner) venue: moon_core::venue::Venue,
    pub(in crate::analytics::tuner) exchange_key: String,
    pub(in crate::analytics::tuner) market: String,
    /// BTC's market on the same exchange, as the catalog spells it — the BTC deltas are read off
    /// its bars; `None` when the catalog names none, and those deltas keep the snapshot.
    pub(in crate::analytics::tuner) btc_market: Option<String>,
    /// The market's price step from the live catalog, when the core reports it.
    pub(in crate::analytics::tuner) tick: Option<f64>,
}

/// One "now" cell of the parameter grid over the selected strategies.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::analytics::tuner) enum NowValue {
    /// Every selected strategy holds this value (or leaves the field at default).
    Same(String),
    /// The selected strategies disagree; the grid prints "varies".
    Differs,
}

/// The loaded picture of the axis for one scope.
#[derive(Clone, Debug, Default)]
pub(in crate::analytics::tuner) struct TicksData {
    /// Chronological by close, as `read_deals` orders them.
    pub(in crate::analytics::tuner) rows: Vec<DealRow>,
    /// Scope rows without millisecond stamps — in the Fact column, not in the table.
    pub(in crate::analytics::tuner) without_ms: usize,
    /// Service rows with stamps the axis never takes (funding, liquidations, joined sells, no
    /// strategy, and a sale that moved more coins than the entry bought — a spot position
    /// topped up from the wallet balance) — in the Fact column, not in the table.
    pub(in crate::analytics::tuner) service: usize,
    /// Trades the tuner cannot be run on — container or unresolved kinds, manual exits — in
    /// the Fact column, not in the table.
    pub(in crate::analytics::tuner) untunable: usize,
    /// One column: the rows fit for the search ([`DealRow::fit`]) — the sample the variants
    /// replay, and the baseline they are compared with. The whole scope is not shown: the axis
    /// works on the fit rows only (the developer's call, 2026-09-23).
    pub(in crate::analytics::tuner) kpi: Vec<VarStats>,
    /// `(hits, answered)` of the entry group over the covered rows.
    pub(in crate::analytics::tuner) entry_share: (usize, usize),
    /// `(hits, answered)` of the exit group over the covered rows.
    pub(in crate::analytics::tuner) exit_share: (usize, usize),
    /// Strategy kinds present among the rows, for the entry group's availability.
    pub(in crate::analytics::tuner) kinds: Vec<String>,
    /// The parameter grid's "now" column, by field key.
    pub(in crate::analytics::tuner) now: HashMap<String, NowValue>,
    /// Each strategy of the rows as it stands now, by `(strategy_id, core_uid)` — the base the
    /// variants and the search lay their values over on that strategy's deals
    /// ([`PreparedDeal::own`]). The grid folds these into one "now" cell; a replay must not,
    /// or a field the strategies disagree on runs every deal at its default.
    pub(in crate::analytics::tuner) own: OwnValues,
    /// The exit fields outside the model each strategy of the scope and of the selection switches
    /// on (`unmodelled.rs`), read with [`Self::own`] — what a search and a write warn about.
    pub(in crate::analytics::tuner) unmodelled: Arc<super::unmodelled::UnmodelledMap>,
    /// The parameter grid's rows by section (`sections::layout`), published with the rows so a
    /// layout never meets another scope's "now" values or kinds.
    pub(in crate::analytics::tuner) grid: Arc<[super::sections::GridSection]>,
    /// Each number knob's automatic search span over the live strategies and the selection
    /// (`params::range::field_span`), by key; a field nothing is known of is absent.
    pub(in crate::analytics::tuner) spans: Arc<HashMap<&'static str, FieldSpan>>,
    /// The number knobs the schema types as integers — their typed ranges are cut to whole
    /// numbers.
    pub(in crate::analytics::tuner) integers: Arc<HashSet<&'static str>>,
}

/// What [`TicksData::tape_budget`] counts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::analytics::tuner) struct TapeBudget {
    pub(in crate::analytics::tuner) rows: usize,
    pub(in crate::analytics::tuner) fit: usize,
    /// Fit rows with their tape in memory — the sample.
    pub(in crate::analytics::tuner) replayable: usize,
    /// Fit rows whose tape the cap let go.
    pub(in crate::analytics::tuner) dropped: usize,
    pub(in crate::analytics::tuner) prints: usize,
    pub(in crate::analytics::tuner) bytes: usize,
}

/// Strategies' current values by `(strategy_id, core_uid)`, one shared map per strategy.
pub(in crate::analytics::tuner) type OwnValues = HashMap<(i64, u64), Arc<HashMap<String, String>>>;

impl TicksData {
    /// Rows whose window the tape covers.
    pub(in crate::analytics::tuner) fn covered(&self) -> usize {
        self.rows
            .iter()
            .filter(|r| r.tape == TapeStatus::Covered)
            .count()
    }

    /// The exit horizon of the replayable sample, in milliseconds — the shortest HELD trail
    /// past the close among the rows the variants and the search replay, the same rule as
    /// `search::common_horizon_ms` over the same rows (`prepared_deals` hands it each row's
    /// `held` trail), or `None` with nothing to replay.
    pub(in crate::analytics::tuner) fn exit_horizon_ms(&self) -> Option<i64> {
        self.replayable()
            .map(|r| r.held.map(|(_, trail)| trail.max(0)).unwrap_or(0))
            .min()
    }

    /// Rows a fetch could still fill: missing, with an address, not already asked.
    pub(in crate::analytics::tuner) fn fetchable(&self) -> impl Iterator<Item = &DealRow> {
        self.rows
            .iter()
            .filter(|r| r.tape == TapeStatus::Missing && r.address.is_some())
    }

    /// Rows fit for the search ([`DealRow::fit`]), tape in memory or not.
    pub(in crate::analytics::tuner) fn fit(&self) -> usize {
        self.rows.iter().filter(|r| r.fit()).count()
    }

    /// Fit rows whose tape is in memory — what the variants and the search replay. A row the
    /// model does not reproduce is out whatever its tape: what the model answers for a variant
    /// of it is not an answer (`fit_for_search`).
    pub(in crate::analytics::tuner) fn replayable(&self) -> impl Iterator<Item = &DealRow> {
        self.rows.iter().filter(|r| r.fit() && r.ticks.is_some())
    }

    /// The share gate per group: whether the model reproduces at least `gate` (a fraction) of
    /// the fact to be searched over. `None` when nothing answered yet.
    pub(in crate::analytics::tuner) fn group_passes(
        &self,
        group: ParamGroup,
        gate: f64,
    ) -> Option<bool> {
        let (hits, n) = self.share_of(group);
        (n > 0).then(|| hits as f64 / n as f64 >= gate)
    }

    /// `(hits, answered)` of one group over the covered rows.
    pub(in crate::analytics::tuner) fn share_of(&self, group: ParamGroup) -> (usize, usize) {
        match group {
            ParamGroup::Entry => self.entry_share,
            ParamGroup::Exit => self.exit_share,
        }
    }

    /// The one kind of the scope, when there is exactly one; the search needs one to know
    /// which fields exist.
    pub(in crate::analytics::tuner) fn single_kind(&self) -> Option<&str> {
        match self.kinds.as_slice() {
            [kind] => Some(kind.as_str()),
            _ => None,
        }
    }

    /// Whether every kind in the scope has an entry model — the Entry group's switch.
    pub(in crate::analytics::tuner) fn entry_modelled(&self) -> bool {
        !self.kinds.is_empty()
            && self
                .kinds
                .iter()
                .all(|k| moon_core::db::tuner::ticks::entry_model_for(k))
    }

    /// The kinds without an entry model, for the group's caption.
    pub(in crate::analytics::tuner) fn unmodelled_kinds(&self) -> Vec<&str> {
        self.kinds
            .iter()
            .map(String::as_str)
            .filter(|k| !moon_core::db::tuner::ticks::entry_model_for(k))
            .collect()
    }
}

/// One of the fetch job's words on a row ([`TicksState::edit_rows`]).
pub(in crate::analytics::tuner) enum RowEdit {
    /// A walk went out for the row: a missing row reads "fetching".
    MarkFetching,
    /// The walk was stopped: a fetching row reads "missing" again.
    UnmarkFetching,
    /// The row's replay after the walk.
    Replay(Box<DealRow>),
}

/// The search of the axis, as far as the row shows it.
pub(in crate::analytics::tuner) enum SuggState {
    Idle,
    /// A run in flight: its handle for the stop button and the progress caption, and the
    /// restart count it was LAUNCHED with — the box stays editable while it runs, and the
    /// caption must count against what is actually running.
    Running {
        handle: SearchHandle,
        total: usize,
    },
}

impl Drop for TicksState {
    /// A search outlives nothing: closing the window while one runs would otherwise leave it
    /// on the shared pool to the end, its answer going nowhere.
    fn drop(&mut self) {
        self.stop_search("window closed");
    }
}

/// State of the "Entry/Exit" mode.
pub(in crate::analytics) struct TicksState {
    pub(in crate::analytics::tuner) data: LoadState<TicksData>,
    /// The variant column's edits (В1): field key to value in strategy spelling. An empty map is
    /// an untouched column, drawn as the base. One column: the second one went on 2026-09-25,
    /// its place in the grid taken by the search ranges.
    pub(in crate::analytics::tuner) variant: HashMap<String, String>,
    /// The variant's KPI over the replayable rows, `None` until computed or while the variant is
    /// untouched.
    pub(in crate::analytics::tuner) var_stats: Option<VarStats>,
    /// How many replayable rows the variant KPI was computed over, for its caption.
    pub(in crate::analytics::tuner) var_n: usize,
    /// Generation of the variant KPI recompute; a stale completion is dropped.
    pub(in crate::analytics::tuner) var_seq: u64,
    /// The pending debounced recompute; dropping it cancels it.
    pub(in crate::analytics::tuner) var_task: Option<gpui::Task<()>>,
    /// The grid's and the row's input boxes, created lazily and kept across repaints.
    pub(in crate::analytics::tuner) inputs: HashMap<String, Entity<MoonInputState>>,
    /// The placeholder each range cell's box was last given, by its id in `inputs` — set again
    /// only when what the search takes there moved (`ranges.rs`).
    pub(in crate::analytics::tuner) placeholders: HashMap<String, String>,
    /// Unticked rows: held by the search at В1's value where В1 has one, else at the strategy's,
    /// bar what a switch it turns on needs. Persisted.
    pub(in crate::analytics::tuner) locked: HashSet<String>,
    /// The search ranges typed over the automatic ones, by field key; a field absent is fully
    /// automatic. Persisted.
    pub(in crate::analytics::tuner) ranges: BTreeMap<String, TickRange>,
    /// Steps per field the automatic ranges are cut into, as typed; empty = the default.
    /// Persisted.
    pub(in crate::analytics::tuner) steps: String,
    /// The field "Search" on one field varies — the one whose name was clicked last.
    pub(in crate::analytics::tuner) sel_field: Option<&'static str>,
    /// The search settings, as typed; all but the minimum trades persist.
    pub(in crate::analytics::tuner) iters: String,
    pub(in crate::analytics::tuner) min_trades: String,
    pub(in crate::analytics::tuner) train_pct: usize,
    /// Base seed of the restarts; empty draws one per search.
    pub(in crate::analytics::tuner) seed: String,
    /// The seed the last completed search ran with, to pin it.
    pub(in crate::analytics::tuner) last_seed: Option<u64>,
    /// Passes of coordinate descent per restart; empty = the search's default.
    pub(in crate::analytics::tuner) passes: String,
    /// The group gate, per cent of reproduced trades; empty = [`DEFAULT_GATE_PCT`].
    pub(in crate::analytics::tuner) gate_pct: String,
    /// Whether the search keeps every trade's entry corridor at least as far from the price as
    /// the trade's own (`SearchParams::keep_corridor`). On unless the user turned it off.
    pub(in crate::analytics::tuner) keep_corridor: bool,
    /// Whether the search settings popover is open.
    pub(in crate::analytics::tuner) sugg_cfg_open: bool,
    /// Whether the model settings popover is open.
    pub(in crate::analytics::tuner) model_cfg_open: bool,
    pub(in crate::analytics::tuner) sugg: SuggState,
    pub(in crate::analytics::tuner) sugg_seq: u64,
    /// What the last completed search found, for the holdout caption.
    pub(in crate::analytics::tuner) last_result: Option<SearchResult>,
    /// The last search's failure to say anything, for the status line.
    pub(in crate::analytics::tuner) sugg_note: Option<String>,
    /// `TicksData::kpi` under the shape the shared matrix reads; applied together with `data`.
    pub(in crate::analytics::tuner) kpi: LoadState<Vec<VarStats>>,
    /// Generation of the load in flight; an older completion is dropped.
    pub(in crate::analytics::tuner) seq: u64,
    /// The loaded picture no longer matches the scope or the report generation.
    pub(in crate::analytics::tuner) dirty: bool,
    /// `(column key, descending)` of the deal table.
    pub(in crate::analytics::tuner) sort: Option<(String, bool)>,
    /// The table shows only the rows fit for the search ([`DealRow::fit`]) — the sample the
    /// variants and the search actually run on: tape covering the window, and the model
    /// reproducing the trade. Off by default: the rows without their tape are the ones the
    /// fetch button exists for, and a filter that hides them hides the work to be done.
    ///
    /// The model's verdict became part of the sample on the developer's call (2026-09-23): a
    /// trade the model cannot reproduce on its own settings — a book it has no copy of, a rule
    /// it does not have, an input the record did not keep — answers nothing true for a variant.
    /// The worry that held this back before — a sample narrowed to what the model already fits
    /// is fitted on itself — is why the cut is the verdict on the trade's OWN settings, taken
    /// once per load, never the variant's.
    pub(in crate::analytics::tuner) only_fit: bool,
    /// The sorted row order, cached against `rows_rev` and the sort.
    pub(in crate::analytics::tuner) order: Option<super::rows::OrderCache>,
    /// Bumped whenever `data` changes, so the cached order is rebuilt.
    pub(in crate::analytics::tuner) rows_rev: u64,
    /// The task listening to the process-wide fetch job (`fetch::job`) for this view; `None`
    /// until a batch is started or found running. Dropped with the view, which ends it.
    pub(in crate::analytics::tuner) fetch_task: Option<gpui::Task<()>>,
    /// Whether that task is still in its loop — it ends with the batch, and the next batch
    /// attaches a fresh one.
    pub(in crate::analytics::tuner) fetch_listening: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// Generation of the tape stage in flight; an older stage's answer is dropped (`load.rs`).
    pub(in crate::analytics::tuner) tape_seq: u64,
    /// The model settings every judged row of the table was judged under, when one set is known
    /// — what lets a reload carry a row's verdict instead of replaying it (`load.rs`, stage B).
    /// `None` until a tape stage has judged the table, and from a change of the settings until
    /// the stage that re-judges it has folded.
    pub(in crate::analytics::tuner) judged_under:
        Option<moon_core::db::tuner::ticks::ModelSettings>,
    /// Whether the tape stage of a load is still reading the rows' tape off the worker: until
    /// it folds, every addressed row reads "missing" without meaning it.
    pub(in crate::analytics::tuner) tape_reading: bool,
    /// The trade pane under the table (`trade_pane.rs`).
    pub(in crate::analytics::tuner) trade: super::trade_pane::TradePane,
    /// The variant's result per deal, by `ReportUID`, as `(money in the sample's unit, per cent)`
    /// — what the column counted for that deal (`variant_tally_by_deal`). A deal absent is one
    /// the variant makes no trade of, or one outside the replayed sample. Scored with the
    /// column, cleared with it.
    pub(in crate::analytics::tuner) plan: HashMap<i64, (f64, f64)>,
    /// `sections::schema_signature` of the store when the latest load chose the keys of its
    /// "now" values — set when the load is ASKED, so a load already running under a new schema
    /// is not asked for again (`grid.rs`).
    pub(in crate::analytics::tuner) keys_sig: Option<u64>,
    /// The schema signature a reload was last asked for because the latest load chose its keys
    /// under another; one ask per signature, so a failed reload is not retried every frame.
    pub(in crate::analytics::tuner) schema_reload: Option<u64>,
    /// The grid's sections the user opened; every section starts folded (`grid.rs`).
    pub(in crate::analytics::tuner) open_sections: HashSet<ParamSection>,
}

impl Default for TicksState {
    fn default() -> Self {
        Self {
            data: LoadState::default(),
            variant: HashMap::new(),
            var_stats: None,
            var_n: 0,
            var_seq: 0,
            var_task: None,
            inputs: HashMap::new(),
            placeholders: HashMap::new(),
            locked: HashSet::new(),
            ranges: BTreeMap::new(),
            steps: String::new(),
            sel_field: None,
            iters: String::new(),
            min_trades: String::new(),
            train_pct: super::super::filter::state::DEFAULT_TRAIN,
            seed: String::new(),
            last_seed: None,
            passes: String::new(),
            gate_pct: String::new(),
            keep_corridor: true,
            sugg_cfg_open: false,
            model_cfg_open: false,
            sugg: SuggState::Idle,
            sugg_seq: 0,
            last_result: None,
            sugg_note: None,
            kpi: LoadState::default(),
            seq: 0,
            dirty: true,
            sort: Some((super::columns::COL_TIME.to_string(), true)),
            only_fit: false,
            order: None,
            rows_rev: 0,
            fetch_task: None,
            fetch_listening: Default::default(),
            tape_reading: false,
            judged_under: None,
            tape_seq: 0,
            trade: Default::default(),
            plan: HashMap::new(),
            keys_sig: None,
            schema_reload: None,
            open_sections: HashSet::new(),
        }
    }
}

impl TicksState {
    /// The group gate as a fraction: the typed per cent, else [`DEFAULT_GATE_PCT`], within 0–100.
    pub(in crate::analytics::tuner) fn gate(&self) -> f64 {
        let pct = self
            .gate_pct
            .trim()
            .parse::<u32>()
            .unwrap_or(DEFAULT_GATE_PCT)
            .min(100);
        f64::from(pct) / 100.0
    }

    /// Take the persisted settings of the axis (`WindowLayout::analytics_ticks`).
    pub(in crate::analytics) fn restore(&mut self, saved: &moon_core::config::TicksAxisLayout) {
        self.iters = saved.iters.map(|n| n.to_string()).unwrap_or_default();
        self.train_pct = saved
            .train
            .map(|n| n as usize)
            .filter(|n| super::super::filter::state::TRAIN_OPTIONS.contains(n))
            .unwrap_or(super::super::filter::state::DEFAULT_TRAIN);
        self.seed = saved.seed.clone().unwrap_or_default();
        self.passes = saved.passes.map(|n| n.to_string()).unwrap_or_default();
        self.gate_pct = saved.gate_pct.map(|n| n.to_string()).unwrap_or_default();
        self.locked = saved.locked.iter().cloned().collect();
        self.trade.open = saved.trade_open;
        self.keep_corridor = !saved.allow_closer_corridor;
        self.steps = saved
            .steps_per_param
            .map(|n| n.to_string())
            .unwrap_or_default();
        self.ranges = saved.ranges.clone();
    }

    /// Steps per field the automatic ranges are cut into, out of the typed box.
    pub(in crate::analytics::tuner) fn steps_per_param(&self) -> u32 {
        moon_core::db::tuner::ticks::params::range::steps_of(self.steps.trim().parse::<u32>().ok())
    }

    /// The axis' settings as the layout persists them, the model's from their process-wide
    /// store.
    pub(in crate::analytics::tuner) fn saved(&self) -> moon_core::config::TicksAxisLayout {
        let number = |text: &str| text.trim().parse::<u32>().ok();
        let mut locked: Vec<String> = self.locked.iter().cloned().collect();
        locked.sort();
        moon_core::config::TicksAxisLayout {
            iters: number(&self.iters),
            train: Some(self.train_pct as u32),
            seed: Some(self.seed.trim().to_string()).filter(|s| s.parse::<u64>().is_ok()),
            passes: number(&self.passes),
            gate_pct: number(&self.gate_pct),
            locked,
            model: super::model_cfg::current(),
            trade_open: self.trade.open,
            allow_closer_corridor: !self.keep_corridor,
            min_tail_s: Some(super::tail::current_s()),
            steps_per_param: number(&self.steps),
            ranges: self.ranges.clone(),
        }
    }

    /// Report-derived numbers are stale; the next entry into the mode reloads them.
    pub(in crate::analytics) fn mark_report_stale(&mut self) {
        self.dirty = true;
    }

    /// The scope changed: every row belongs to the previous scope. The fetch batch is the
    /// process's, not the scope's, and runs on; its answers land on rows by id where present.
    pub(in crate::analytics) fn invalidate(&mut self) {
        self.retire_rows();
        // A running search and its last answer describe the previous scope's deals.
        self.stop_search("scope change");
        // Nor does the last search's holdout: В1's caption would print it beside a column
        // rescored over deals it never saw.
        self.last_result = None;
        // A note about the previous scope's search says nothing about this one.
        self.sugg_note = None;
    }

    /// The report axis moved — a core's clock offset measured, or measured again: the rows are
    /// read again on the new axis, but the scope is the same strategies over the same period,
    /// and a running search goes on. It runs on its own copy of the deals and lands in В1, whose
    /// columns are then rescored over the reloaded rows — as across a report that moved
    /// (`load.rs`). Stopped here, no search outlived the minutes after a start, while the cores
    /// adopt their offsets one by one (2026-09-24: the axis moved every ~30 s).
    pub(in crate::analytics) fn invalidate_for_axis(&mut self) {
        self.retire_rows();
    }

    /// Retire the rows and everything scored over them; the variant EDITS are the user's and
    /// stay, to be rescored over the new rows.
    fn retire_rows(&mut self) {
        self.dirty = true;
        self.seq = self.seq.wrapping_add(1);
        self.rows_rev = self.rows_rev.wrapping_add(1);
        self.order = None;
        self.var_seq = self.var_seq.wrapping_add(1);
        self.var_task = None;
        self.var_stats = None;
        self.plan.clear();
        if let Some(data) = self.data.data_mut() {
            for row in &mut data.rows {
                if row.tape == TapeStatus::Fetching {
                    row.tape = TapeStatus::Missing;
                }
            }
        }
    }

    /// Whether entering the mode requires a load — mirrors `CoinsState::needs_reload`.
    pub(in crate::analytics) fn needs_reload(&self) -> bool {
        self.data.data().is_none() || self.dirty
    }

    /// Ask a running search to stop and forget it; its completion is dropped by the
    /// generation.
    pub(in crate::analytics::tuner) fn stop_search(&mut self, reason: &str) {
        if let SuggState::Running { handle, total } = &self.sugg {
            log::info!(
                target: moon_core::diagnostics::TICKS_AXIS_TARGET,
                "[x] ticks search: stopped by {reason} at {}/{total} restart(s)",
                handle.completed()
            );
            handle.cancel();
        }
        self.sugg = SuggState::Idle;
        self.sugg_seq = self.sugg_seq.wrapping_add(1);
    }

    /// The variant's changes over the base as `(key, value)` pairs, sorted — what Save writes
    /// and what the KPI is computed for.
    pub(in crate::analytics::tuner) fn variant_changes(&self) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = self
            .variant
            .iter()
            .filter(|(_, v)| !v.trim().is_empty())
            .map(|(k, v)| (k.clone(), v.trim().to_string()))
            .collect();
        out.sort();
        out
    }

    /// Whether the variant holds anything to write.
    pub(in crate::analytics::tuner) fn has_changes(&self) -> bool {
        !self.variant_changes().is_empty()
    }

    /// Set one cell of the variant; an empty value clears it.
    pub(in crate::analytics::tuner) fn set_variant(&mut self, key: &str, value: String) {
        if value.trim().is_empty() {
            self.variant.remove(key);
        } else {
            self.variant.insert(key.to_string(), value);
        }
    }

    /// Apply the fetch job's word on some rows, by id, in order — then ONE recount. A hop of the
    /// job's listener brings every answer queued since the last one, and a recount per answer
    /// is a pass over the whole table each: hundreds of answers served off the disk in a few
    /// seconds made that a pass over the table hundreds of times. A row not in the table (the
    /// scope moved on) is skipped.
    ///
    /// Args:
    ///     edits: `(report_uid, what to do)`, in the order the job said it.
    pub(in crate::analytics::tuner) fn edit_rows(
        &mut self,
        edits: impl IntoIterator<Item = (i64, RowEdit)>,
    ) {
        let Some(data) = self.data.data_mut() else {
            return;
        };
        let index: HashMap<i64, usize> = data
            .rows
            .iter()
            .enumerate()
            .map(|(i, r)| (r.deal.report_uid, i))
            .collect();
        let mut touched = false;
        let mut replayed = false;
        for (uid, edit) in edits {
            let Some(row) = index.get(&uid).and_then(|&i| data.rows.get_mut(i)) else {
                continue;
            };
            touched = true;
            match edit {
                RowEdit::MarkFetching if row.tape == TapeStatus::Missing => {
                    row.tape = TapeStatus::Fetching;
                }
                RowEdit::UnmarkFetching if row.tape == TapeStatus::Fetching => {
                    row.tape = TapeStatus::Missing;
                }
                RowEdit::MarkFetching | RowEdit::UnmarkFetching => {}
                RowEdit::Replay(answer) => {
                    row.take_replay(*answer);
                    replayed = true;
                }
            }
        }
        if !touched {
            return;
        }
        // A mark moves neither the sample nor the shares; only an answer is worth the recount.
        if replayed {
            data.retain_within_cap();
            data.refresh_summary();
        }
        let kpi = data.kpi.clone();
        // In place as well, so the two load states stay in the same phase: a stale picture
        // edited during a reload stays stale in both, and a reload's outcome lands on both.
        if let Some(slot) = self.kpi.data_mut() {
            *slot = kpi;
        }
        // The tape and model columns sort by what just changed, so the order is rebuilt; the
        // rows themselves stay where they are.
        self.rows_rev = self.rows_rev.wrapping_add(1);
        self.order = None;
    }

    /// Fold a batch of answered rows in by id — the tape stage's whole result — with one
    /// summary pass rather than one per row. A row not in the table (the scope moved on) is
    /// dropped.
    pub(in crate::analytics::tuner) fn update_rows(&mut self, answers: Vec<DealRow>) {
        let Some(data) = self.data.data_mut() else {
            return;
        };
        let index: HashMap<i64, usize> = data
            .rows
            .iter()
            .enumerate()
            .map(|(i, r)| (r.deal.report_uid, i))
            .collect();
        for answer in answers {
            let Some(&i) = index.get(&answer.deal.report_uid) else {
                continue;
            };
            let slot = &mut data.rows[i];
            // The fetch job may have covered the row while this batch was being read, and
            // said so through the listener; a read from before its walk must not undo that.
            // Coverage only grows between reloads, so the fresher word is the covered one —
            // and a refusal the walk itself gave outranks a plain "missing" read from before
            // it, or the row would read as fetchable again and be queued once more.
            let stale = match (slot.tape, answer.tape) {
                (TapeStatus::Covered, other) => other != TapeStatus::Covered,
                (TapeStatus::Refused(_), TapeStatus::Missing) => true,
                _ => false,
            };
            if stale {
                continue;
            }
            slot.take_replay(answer);
        }
        data.retain_within_cap();
        data.refresh_summary();
        let kpi = data.kpi.clone();
        if let Some(slot) = self.kpi.data_mut() {
            *slot = kpi;
        }
        self.rows_rev = self.rows_rev.wrapping_add(1);
        self.order = None;
    }

    /// Publish one loaded picture (or its failure) to both load states at once.
    pub(in crate::analytics::tuner) fn publish(
        &mut self,
        result: Result<TicksData, moon_core::db::ReadFail>,
        keep_on_failure: bool,
    ) {
        let kpi = result.as_ref().map(|d| d.kpi.clone()).map_err(Clone::clone);
        self.kpi.apply_or_keep(kpi, keep_on_failure);
        self.data.apply_or_keep(result, keep_on_failure);
        self.rows_rev = self.rows_rev.wrapping_add(1);
        self.order = None;
    }
}

impl TicksData {
    /// A row as the variants and the search replay it — its tape, its archived entry line, its
    /// held trail, its strategy's current values; `None` without a tape in memory. The tape
    /// stays packed and uncut: the caller unpacks off the UI thread and cuts the sample at its
    /// one horizon ([`prepare_sample`](super::tape::prepare_sample)).
    pub(in crate::analytics::tuner) fn prepared(&self, row: &DealRow) -> Option<PendingDeal> {
        Some(PendingDeal {
            deal: row.deal.clone(),
            tape: row.ticks.clone()?,
            entry_line: row.entry_line.clone(),
            trail_ms: row.held.map(|(_, trail)| trail).unwrap_or(0),
            own: self
                .own
                .get(&(row.deal.strategy_id, row.deal.core_uid))
                .cloned()
                .unwrap_or_default(),
        })
    }

    /// Let go of the tapes nothing replays — a row that is not fit is never in the sample — and
    /// of those past the memory cap, oldest rows first: the newest deals are the ones a variant
    /// is most likely to be judged on. A row whose tape is dropped stays covered; it simply sits
    /// out of the variant columns. Run after every change of the retained set: the bulk load
    /// and each fetched row.
    pub(in crate::analytics::tuner) fn retain_within_cap(&mut self) {
        let mut held = 0usize;
        // Newest first: the rows are chronological, so walk them backwards.
        for row in self.rows.iter_mut().rev() {
            let Some(bytes) = row.ticks.as_ref().map(PackedTape::bytes) else {
                continue;
            };
            if !row.fit() || held + bytes > MAX_RETAINED_BYTES {
                row.ticks = None;
            } else {
                held += bytes;
            }
        }
    }

    /// What the table holds of its tape, for the load's log line: the rows, the fit ones, the
    /// fit ones with their tape in memory, those the cap let go, and the prints and bytes held.
    pub(in crate::analytics::tuner) fn tape_budget(&self) -> TapeBudget {
        let mut budget = TapeBudget {
            rows: self.rows.len(),
            ..TapeBudget::default()
        };
        for row in self.rows.iter().filter(|r| r.fit()) {
            budget.fit += 1;
            match &row.ticks {
                Some(tape) => {
                    budget.replayable += 1;
                    budget.prints += tape.len();
                    budget.bytes += tape.bytes();
                }
                None => budget.dropped += 1,
            }
        }
        budget
    }

    /// Recompute the fit-subset KPI and the ✓ shares from the rows — after a load or a fetch
    /// changed them. The shares stay over every covered row: they are how much of the tape the
    /// model reproduces, which is what the fit subset is cut from.
    pub(in crate::analytics::tuner) fn refresh_summary(&mut self) {
        let subset = moon_core::db::tuner::ticks::fact_stats(
            self.rows.iter().filter(|r| r.fit()).map(|r| &r.deal),
        );
        self.kpi = vec![subset];
        let verdicts = || self.rows.iter().filter_map(|r| r.verdict.as_ref());
        self.entry_share = moon_core::db::tuner::ticks::verify::share(verdicts().map(|v| v.entry));
        self.exit_share = moon_core::db::tuner::ticks::verify::share(verdicts().map(|v| v.exit));
    }
}
