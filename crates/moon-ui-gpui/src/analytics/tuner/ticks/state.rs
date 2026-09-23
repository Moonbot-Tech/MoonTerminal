//! State of the "Entry/Exit" axis: the scope's deals with what the terminal holds for each
//! (tape covered or not, the model's verdict on the fact), the KPI of the whole scope beside
//! the KPI of the replayable subset, the "now" values of the parameter grid, and the queue of
//! tape fetches the user asked for.
//!
//! Split from the rendering (`ticks/mod.rs`) like every other axis: the load paths write here,
//! the render path only reads.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use gpui::Entity;
use moon_ui::MoonInputState;

use super::super::shared::N_VAR;
use crate::load_state::LoadState;
use moon_core::db::tuner::VarStats;
use moon_core::db::tuner::threshold_search::SearchHandle;
use moon_core::db::tuner::ticks::params::ParamGroup;
use moon_core::db::tuner::ticks::search::SearchResult;
use moon_core::db::tuner::ticks::{Deal, Verdict, fit_for_search};
use moon_core::feed::types::Tick;
use moon_core::market::trade_replay::TickStatus;

/// Share of hits a group needs before it may be searched: a model that cannot reproduce the
/// fact must not be asked what would have been better. The spec's proposal (80 %), to be tuned
/// by practice.
pub(in crate::analytics::tuner) const SHARE_GATE: f64 = 0.8;

/// Ticks kept in memory across every covered row, for the variants and the search. Past it a
/// row is still "covered" — the model ran on it — but its tape is let go and the row sits out
/// of the variant columns; the caption says how many.
pub(in crate::analytics::tuner) const MAX_RETAINED_TICKS: usize = 4_000_000;

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
    /// The window's prints, kept for the variants and the search while the row is covered and
    /// the memory cap allows; `None` otherwise.
    pub(in crate::analytics::tuner) ticks: Option<Arc<[Tick]>>,
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
    /// Whether the variants and the search run on this row: its tape covers the window and the
    /// model reproduced it (`fit_for_search`). The table shows every row; this is the sample.
    pub(in crate::analytics::tuner) fn fit(&self) -> bool {
        self.tape == TapeStatus::Covered && self.verdict.as_ref().is_some_and(fit_for_search)
    }

    /// Take everything a replay learned about this row from its answer: the tape's word, the
    /// verdict, the model inputs derived for the deal (the price step, the archived pre-spike
    /// ask and take, the core's step lag, what the fact proves about the stop, the entry the
    /// trade ran with), the prints, the entry line and the held coverage.
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
        self.deal.step_lag_ms = answer.deal.step_lag_ms;
        self.deal.stop_anchor = answer.deal.stop_anchor;
        self.deal.own_entry = answer.deal.own_entry;
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
    /// Column 0: the whole scope (the same SQL as every axis' "Fact", stamps or not); column
    /// 1: the rows fit for the search ([`DealRow::fit`]) — the sample the variants replay.
    pub(in crate::analytics::tuner) kpi: Vec<VarStats>,
    /// `(hits, answered)` of the entry group over the covered rows.
    pub(in crate::analytics::tuner) entry_share: (usize, usize),
    /// `(hits, answered)` of the exit group over the covered rows.
    pub(in crate::analytics::tuner) exit_share: (usize, usize),
    /// Strategy kinds present among the rows, for the entry group's availability.
    pub(in crate::analytics::tuner) kinds: Vec<String>,
    /// The parameter grid's "now" column, by field key.
    pub(in crate::analytics::tuner) now: HashMap<String, NowValue>,
}

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

    /// The share gate per group: whether the model reproduces enough of the fact to be
    /// searched over. `None` when nothing answered yet.
    pub(in crate::analytics::tuner) fn group_passes(&self, group: ParamGroup) -> Option<bool> {
        let (hits, n) = match group {
            ParamGroup::Entry => self.entry_share,
            ParamGroup::Exit => self.exit_share,
        };
        (n > 0).then(|| hits as f64 / n as f64 >= SHARE_GATE)
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
        self.stop_search();
    }
}

/// State of the "Entry/Exit" mode.
pub(in crate::analytics) struct TicksState {
    pub(in crate::analytics::tuner) data: LoadState<TicksData>,
    /// The variant columns' edits: field key to value in strategy spelling. An empty map is
    /// an untouched column, drawn as the base.
    pub(in crate::analytics::tuner) variants: [HashMap<String, String>; N_VAR],
    /// The KPI of each variant over the replayable rows, `None` until computed or while the
    /// variant is untouched.
    pub(in crate::analytics::tuner) var_stats: [Option<VarStats>; N_VAR],
    /// How many replayable rows the variant KPIs were computed over, for their captions.
    pub(in crate::analytics::tuner) var_n: usize,
    /// Generation of the variant KPI recompute; a stale completion is dropped.
    pub(in crate::analytics::tuner) var_seq: u64,
    /// The pending debounced recompute; dropping it cancels it.
    pub(in crate::analytics::tuner) var_task: Option<gpui::Task<()>>,
    /// The grid's and the row's input boxes, created lazily and kept across repaints.
    pub(in crate::analytics::tuner) inputs: HashMap<String, Entity<MoonInputState>>,
    /// Which groups the search may vary.
    pub(in crate::analytics::tuner) vary_entry: bool,
    pub(in crate::analytics::tuner) vary_exit: bool,
    /// Fields held at their base value by the search.
    pub(in crate::analytics::tuner) locked: HashSet<String>,
    /// The search settings, as typed.
    pub(in crate::analytics::tuner) iters: String,
    pub(in crate::analytics::tuner) min_trades: String,
    pub(in crate::analytics::tuner) train_pct: usize,
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
    /// Whether the two parameter groups are unfolded.
    pub(in crate::analytics::tuner) entry_open: bool,
    pub(in crate::analytics::tuner) exit_open: bool,
    /// The task listening to the process-wide fetch job (`fetch::job`) for this view; `None`
    /// until a batch is started or found running. Dropped with the view, which ends it.
    pub(in crate::analytics::tuner) fetch_task: Option<gpui::Task<()>>,
    /// Whether that task is still in its loop — it ends with the batch, and the next batch
    /// attaches a fresh one.
    pub(in crate::analytics::tuner) fetch_listening: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// Whether the tape stage of a load is still reading the rows' tape off the worker: until
    /// it folds, every addressed row reads "missing" without meaning it.
    pub(in crate::analytics::tuner) tape_reading: bool,
}

impl Default for TicksState {
    fn default() -> Self {
        Self {
            data: LoadState::default(),
            variants: Default::default(),
            var_stats: Default::default(),
            var_n: 0,
            var_seq: 0,
            var_task: None,
            inputs: HashMap::new(),
            vary_entry: true,
            vary_exit: true,
            locked: HashSet::new(),
            iters: String::new(),
            min_trades: String::new(),
            train_pct: super::super::filter::state::DEFAULT_TRAIN,
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
            entry_open: true,
            exit_open: true,
            fetch_task: None,
            fetch_listening: Default::default(),
            tape_reading: false,
        }
    }
}

impl TicksState {
    /// Report-derived numbers are stale; the next entry into the mode reloads them.
    pub(in crate::analytics) fn mark_report_stale(&mut self) {
        self.dirty = true;
    }

    /// The scope changed: every row belongs to the previous scope. The fetch batch is the
    /// process's, not the scope's, and runs on; its answers land on rows by id where present.
    pub(in crate::analytics) fn invalidate(&mut self) {
        self.dirty = true;
        self.seq = self.seq.wrapping_add(1);
        self.rows_rev = self.rows_rev.wrapping_add(1);
        self.order = None;
        // The variant KPIs and a running search describe the previous scope's deals; the
        // variant EDITS are the user's and stay, to be rescored over the new scope.
        self.var_seq = self.var_seq.wrapping_add(1);
        self.var_task = None;
        self.var_stats = Default::default();
        self.stop_search();
        // A note about the previous scope's search says nothing about this one.
        self.sugg_note = None;
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
    pub(in crate::analytics::tuner) fn stop_search(&mut self) {
        if let SuggState::Running { handle, .. } = &self.sugg {
            handle.cancel();
        }
        self.sugg = SuggState::Idle;
        self.sugg_seq = self.sugg_seq.wrapping_add(1);
    }

    /// The variant's changes over the base as `(key, value)` pairs, sorted — what Save writes
    /// and what the KPI is computed for.
    pub(in crate::analytics::tuner) fn variant_changes(
        &self,
        index: usize,
    ) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = self.variants[index]
            .iter()
            .filter(|(_, v)| !v.trim().is_empty())
            .map(|(k, v)| (k.clone(), v.trim().to_string()))
            .collect();
        out.sort();
        out
    }

    /// Whether the first variant holds anything to write.
    pub(in crate::analytics::tuner) fn has_changes(&self) -> bool {
        !self.variant_changes(0).is_empty()
    }

    /// Set one cell of a variant; an empty value clears it.
    pub(in crate::analytics::tuner) fn set_variant(
        &mut self,
        index: usize,
        key: &str,
        value: String,
    ) {
        if value.trim().is_empty() {
            self.variants[index].remove(key);
        } else {
            self.variants[index].insert(key.to_string(), value);
        }
    }

    /// Replace one row's replay result in place, after a fetch, keeping the rest.
    ///
    /// Args:
    ///     report_uid: The row.
    ///     update: What the fetch learned.
    pub(in crate::analytics::tuner) fn update_row(
        &mut self,
        report_uid: i64,
        update: impl FnOnce(&mut DealRow),
    ) {
        let Some(data) = self.data.data_mut() else {
            return;
        };
        let Some(row) = data
            .rows
            .iter_mut()
            .find(|r| r.deal.report_uid == report_uid)
        else {
            return;
        };
        update(row);
        data.retain_within_cap();
        data.refresh_summary();
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
    /// Let go of the tapes past the memory cap, oldest rows first — the newest deals are the
    /// ones a variant is most likely to be judged on. A row whose tape is dropped stays
    /// covered; it simply sits out of the variant columns. Run after every change of the
    /// retained set: the bulk load and each fetched row.
    pub(in crate::analytics::tuner) fn retain_within_cap(&mut self) {
        let mut held = 0usize;
        // Newest first: the rows are chronological, so walk them backwards.
        for row in self.rows.iter_mut().rev() {
            let Some(ticks) = row.ticks.as_ref() else {
                continue;
            };
            if held + ticks.len() > MAX_RETAINED_TICKS {
                row.ticks = None;
            } else {
                held += ticks.len();
            }
        }
    }

    /// Recompute the fit-subset KPI (column 1) and the ✓ shares from the rows — after a fetch
    /// changed one of them. Column 0, the whole scope, comes from the same SQL every axis'
    /// "Fact" comes from and is left as loaded. The shares stay over every covered row: they are
    /// how much of the tape the model reproduces, which is what the fit subset is cut from.
    pub(in crate::analytics::tuner) fn refresh_summary(&mut self) {
        let subset = moon_core::db::tuner::ticks::fact_stats(
            self.rows.iter().filter(|r| r.fit()).map(|r| &r.deal),
        );
        match self.kpi.get_mut(1) {
            Some(slot) => *slot = subset,
            None => self.kpi.push(subset),
        }
        let verdicts = || self.rows.iter().filter_map(|r| r.verdict.as_ref());
        self.entry_share = moon_core::db::tuner::ticks::verify::share(verdicts().map(|v| v.entry));
        self.exit_share = moon_core::db::tuner::ticks::verify::share(verdicts().map(|v| v.exit));
    }
}
