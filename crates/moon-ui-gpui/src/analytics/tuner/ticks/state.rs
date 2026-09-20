//! State of the "Entry/Exit" axis: the scope's deals with what the terminal holds for each
//! (tape covered or not, the model's verdict on the fact), the KPI of the whole scope beside
//! the KPI of the replayable subset, the "now" values of the parameter grid, and the queue of
//! tape fetches the user asked for.
//!
//! Split from the rendering (`ticks/mod.rs`) like every other axis: the load paths write here,
//! the render path only reads.

use std::collections::HashMap;
use std::sync::Arc;

use crate::load_state::LoadState;
use moon_core::db::tuner::VarStats;
use moon_core::db::tuner::ticks::{Deal, Verdict};
use moon_core::market::trade_replay::TickStatus;

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
}

/// Where a deal's prints live, as the replay worker keys them, plus what a fetch needs.
#[derive(Clone, Debug)]
pub(in crate::analytics::tuner) struct RowAddress {
    pub(in crate::analytics::tuner) core_uid: u64,
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
    /// Column 0: the whole scope (the same SQL as every axis' "Fact", stamps or not); column
    /// 1: the rows the tape covers.
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

    /// Rows a fetch could still fill: missing, with an address, not already asked.
    pub(in crate::analytics::tuner) fn fetchable(&self) -> impl Iterator<Item = &DealRow> {
        self.rows
            .iter()
            .filter(|r| r.tape == TapeStatus::Missing && r.address.is_some())
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

/// The user's fetch request: which rows are queued, which one is in flight.
#[derive(Default)]
pub(in crate::analytics::tuner) struct FetchQueue {
    /// `reportuid`s still to ask for, oldest first.
    pub(in crate::analytics::tuner) pending: Vec<i64>,
    /// The row a request is out for.
    pub(in crate::analytics::tuner) in_flight: Option<i64>,
    /// Rows asked for since the button was pressed, for the "N/M" caption.
    pub(in crate::analytics::tuner) done: usize,
    pub(in crate::analytics::tuner) total: usize,
    /// Bumped on every scope change; an answer carrying an older number is dropped.
    pub(in crate::analytics::tuner) seq: u64,
}

impl FetchQueue {
    pub(in crate::analytics::tuner) fn is_active(&self) -> bool {
        self.in_flight.is_some() || !self.pending.is_empty()
    }

    /// Forget everything queued; an answer in flight is retired by the generation.
    pub(in crate::analytics::tuner) fn clear(&mut self) {
        self.pending.clear();
        self.in_flight = None;
        self.done = 0;
        self.total = 0;
        self.seq = self.seq.wrapping_add(1);
    }
}

/// State of the "Entry/Exit" mode.
pub(in crate::analytics) struct TicksState {
    pub(in crate::analytics::tuner) data: LoadState<TicksData>,
    /// `TicksData::kpi` under the shape the shared matrix reads; applied together with `data`.
    pub(in crate::analytics::tuner) kpi: LoadState<Vec<VarStats>>,
    /// Generation of the load in flight; an older completion is dropped.
    pub(in crate::analytics::tuner) seq: u64,
    /// The loaded picture no longer matches the scope or the report generation.
    pub(in crate::analytics::tuner) dirty: bool,
    /// `(column key, descending)` of the deal table.
    pub(in crate::analytics::tuner) sort: Option<(String, bool)>,
    /// The sorted row order, cached against `rows_rev` and the sort.
    pub(in crate::analytics::tuner) order: Option<super::rows::OrderCache>,
    /// Bumped whenever `data` changes, so the cached order is rebuilt.
    pub(in crate::analytics::tuner) rows_rev: u64,
    /// Whether the two parameter groups are unfolded.
    pub(in crate::analytics::tuner) entry_open: bool,
    pub(in crate::analytics::tuner) exit_open: bool,
    pub(in crate::analytics::tuner) fetch: FetchQueue,
}

impl Default for TicksState {
    fn default() -> Self {
        Self {
            data: LoadState::default(),
            kpi: LoadState::default(),
            seq: 0,
            dirty: true,
            sort: Some((super::columns::COL_TIME.to_string(), true)),
            order: None,
            rows_rev: 0,
            entry_open: true,
            exit_open: true,
            fetch: FetchQueue::default(),
        }
    }
}

impl TicksState {
    /// Report-derived numbers are stale; the next entry into the mode reloads them.
    pub(in crate::analytics) fn mark_report_stale(&mut self) {
        self.dirty = true;
    }

    /// The scope changed: every row and the fetch queue belong to the previous scope.
    ///
    /// A row a fetch was out for goes back to "missing": its answer will be dropped by the
    /// queue's generation, and a stale picture kept across a failed reload must not show a
    /// fetch that is not running.
    pub(in crate::analytics) fn invalidate(&mut self) {
        self.dirty = true;
        self.seq = self.seq.wrapping_add(1);
        self.rows_rev = self.rows_rev.wrapping_add(1);
        self.order = None;
        self.fetch.clear();
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
    /// Recompute the covered-subset KPI (column 1) and the ✓ shares from the rows — after a
    /// fetch changed one of them. Column 0, the whole scope, comes from the same SQL every
    /// axis' "Fact" comes from and is left as loaded.
    pub(in crate::analytics::tuner) fn refresh_summary(&mut self) {
        let subset = moon_core::db::tuner::ticks::fact_stats(
            self.rows
                .iter()
                .filter(|r| r.tape == TapeStatus::Covered)
                .map(|r| &r.deal),
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
