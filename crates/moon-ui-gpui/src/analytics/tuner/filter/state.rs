//! State, persisted search settings, and numeric helpers for the "By filter" tuner.

use std::collections::HashMap;
use std::sync::Arc;

use gpui::*;
use moon_ui::MoonInputState;

use super::super::super::LoadState;
use super::super::shared::{N_VAR, SaveDialog};
use moon_core::db::ReadFail;
use moon_core::db::metrics::Tally;
use moon_core::db::tuner::threshold_search::{
    ComposeSkip, ComposedSet, RESTARTS_MIN, SearchHandle, edges_max, heavy_search_supported,
    restarts_max,
};
use moon_core::db::tuner::{
    Bound, FIELDS, FieldClass, HistBucket, StratFilters, VarStats, Variant,
};

/// Every quantile depth the search accepts, coarsest first. What is OFFERED is a prefix of this
/// — see [`edge_options`].
const EDGE_OPTIONS_ALL: [usize; 8] = [4, 8, 16, 32, 64, 128, 256, 512];

/// Selectable quantile depths on THIS machine.
///
/// The finest depth doubles the edges the descent walks on every pass of every restart, so it
/// rides the same machine bar as automatic composition: below `HEAVY_SEARCH_MIN_CORES` it is not
/// offered at all, and a value carried over in `layout.toml` from a larger machine falls back
/// through [`edges_of`] to the default rather than opening a run nobody wants to sit through.
pub(in crate::analytics::tuner) fn edge_options() -> &'static [usize] {
    edge_options_upto(edges_max())
}

/// [`edge_options`] with the ceiling supplied, so both machine classes can be checked in one test
/// run — a given machine only ever exercises one of them.
///
/// Derived from the SEARCH's own ceiling rather than from a list of its own. The dropdown then
/// cannot offer a depth the search would clamp away, whatever either side is retuned to.
fn edge_options_upto(max: usize) -> &'static [usize] {
    let n = EDGE_OPTIONS_ALL.iter().take_while(|d| **d <= max).count();
    &EDGE_OPTIONS_ALL[..n]
}

/// Depth used when nothing is chosen or a stored value is not on offer.
///
/// Must be one of [`edge_options`] on every machine, or the dropdown would open showing a value
/// none of its items is marked as.
pub(super) const DEFAULT_EDGES: usize = 32;

/// Restarts used when the box is empty or unparseable.
pub(in crate::analytics::tuner) const DEFAULT_ITERS: usize = 100;

/// Convert raw "restarts" text to the count used by both the search and persistence.
///
/// Read through [`parse_num`], so the same `k` suffix the box DISPLAYS is one it accepts back.
/// Compact text keeps large counts legible in the narrow box. Bounds come from the search module
/// — including the machine-dependent ceiling, so the box cannot accept a count this machine's
/// search would then quietly cut down.
pub(in crate::analytics::tuner) fn iters_of(text: &str) -> usize {
    let max = restarts_max();
    parse_num(text)
        .filter(|v| *v >= 0.0)
        .map(|v| v.min(max as f64) as usize)
        .unwrap_or(DEFAULT_ITERS)
        .clamp(RESTARTS_MIN, max)
}

/// Canonical text for a restart count after editing finishes.
///
/// Args:
///     text: Raw input text, including optional compact suffixes.
///
/// Returns:
///     Exact compact spelling of the count the search will execute.
pub(in crate::analytics::tuner) fn canonical_iters(text: &str) -> String {
    fmt_bound(iters_of(text) as f64)
}

/// Box text to open the tuner with for a persisted restart count.
///
/// Compact where that is EXACT — 25000 opens as `25k`, which fits the box, while 1234 stays
/// `1234` because `1.23k` is a different number. [`fmt_bound`] owns that distinction, and
/// [`iters_of`] reads either form back, so the box can never display a count the search would
/// not run.
///
/// Missing values use the default; anything above what THIS machine allows is clamped to it — a
/// count saved on a bigger one opens at the local ceiling rather than promising a longer run.
pub(super) fn restore_iters(saved: Option<u32>) -> String {
    let count = saved.map_or(DEFAULT_ITERS, |v| {
        (v as usize).clamp(RESTARTS_MIN, restarts_max())
    });
    fmt_bound(count as f64)
}

/// Return `v` when the dropdown offers it, otherwise the default depth.
///
/// Args:
///     v: Candidate quantile depth.
///
/// Returns:
///     Depth selectable on this machine or [`DEFAULT_EDGES`].
pub(super) fn edges_of(v: usize) -> usize {
    edges_of_upto(v, edges_max())
}

/// Return `v` when the supplied machine ceiling offers it, otherwise the default depth.
///
/// Args:
///     v: Candidate quantile depth.
///     max: Machine-specific search ceiling.
///
/// Returns:
///     Selectable depth or [`DEFAULT_EDGES`].
fn edges_of_upto(v: usize, max: usize) -> usize {
    if edge_options_upto(max).contains(&v) {
        v
    } else {
        DEFAULT_EDGES
    }
}

/// Depth to open the tuner with for a persisted value.
///
/// Args:
///     saved: Persisted quantile depth.
///
/// Returns:
///     Depth selectable on this machine or [`DEFAULT_EDGES`].
pub(super) fn restore_edges(saved: Option<u32>) -> usize {
    restore_edges_upto(saved, edges_max())
}

/// Restore a persisted depth against an explicit machine ceiling.
///
/// Args:
///     saved: Persisted quantile depth.
///     max: Machine-specific search ceiling.
///
/// Returns:
///     Selectable restored depth or [`DEFAULT_EDGES`].
fn restore_edges_upto(saved: Option<u32>, max: usize) -> usize {
    saved.map_or(DEFAULT_EDGES, |v| edges_of_upto(v as usize, max))
}

/// Selectable train shares, as a PERCENTAGE of the period the search may fit on.
///
/// 100 is "no split at all", which is why it leads the list: holding trades back is the opt-in,
/// and every scope small enough that a split would starve the search keeps working untouched.
pub(in crate::analytics::tuner) const TRAIN_OPTIONS: [usize; 6] = [100, 90, 80, 70, 60, 50];

/// Train share used when nothing is chosen or a stored value is not on offer.
pub(super) const DEFAULT_TRAIN: usize = 100;

/// Return `v` when the dropdown offers it, otherwise the default share.
pub(super) fn train_of(v: usize) -> usize {
    if TRAIN_OPTIONS.contains(&v) {
        v
    } else {
        DEFAULT_TRAIN
    }
}

/// Train share to open the tuner with for a persisted value.
pub(super) fn restore_train(saved: Option<u32>) -> usize {
    saved.map_or(DEFAULT_TRAIN, |v| train_of(v as usize))
}

/// The stored percentage as the fraction the search takes.
pub(in crate::analytics::tuner) fn train_frac(pct: usize) -> f64 {
    train_of(pct) as f64 / 100.0
}

/// The base seed raw box text asks for: `None` means "draw a fresh one for every search".
///
/// Anything that is not a plain unsigned number reads as no seed rather than as a seed of zero:
/// a typo must not silently pin every future search to one set of random starts.
pub(in crate::analytics::tuner) fn seed_of(text: &str) -> Option<u64> {
    text.trim().parse::<u64>().ok()
}

/// The seed value to persist for the current box text; `None` while it holds no usable seed.
pub(in crate::analytics::tuner) fn persist_seed(text: &str) -> Option<String> {
    seed_of(text).map(|v| v.to_string())
}

/// Box text to open the tuner with for a persisted seed.
///
/// A stored value that no longer parses opens the box empty — the search then draws its own seed,
/// which is the same thing an empty box has always meant.
pub(super) fn restore_seed(saved: Option<String>) -> String {
    saved
        .as_deref()
        .and_then(seed_of)
        .map(|v| v.to_string())
        .unwrap_or_default()
}

/// Checkbox mask to open the tuner with for a persisted list of column ids.
///
/// Always iterates [`FIELDS`] and asks whether each field's column was saved — never the other way
/// round — so the mask spans the table whatever the saved list holds: an id that no longer exists
/// is ignored, and a field added since the save opens unchecked rather than joining the search
/// unannounced. Three grid and search call sites index this by field position, so a mask shorter
/// than the table would panic on the first render.
///
/// `None` (no usable saved list) restores the default: every field whose threshold a strategy can
/// store. An empty list is NOT that — it is the user having unchecked everything, and stays empty.
///
/// Args:
///     saved: Persisted report-column ids, or `None` when no usable preference was loaded.
///
/// Returns:
///     One enabled flag per entry in [`FIELDS`], in the same order.
pub(super) fn restore_enabled(saved: Option<&[String]>) -> Vec<bool> {
    match saved {
        Some(cols) => FIELDS
            .iter()
            .map(|s| cols.iter().any(|c| c == s.col))
            .collect(),
        None => FIELDS.iter().map(|s| s.mapped()).collect(),
    }
}

/// What the "By filter" threshold search is doing, as one exhaustive state.
///
/// One enum rather than a busy flag plus a borrowed error channel: the flag could not say whether
/// a finished search had failed, and routing suggestion failures into the KPI `LoadState` made a
/// failed SEARCH erase the KPI matrix, which is a different read entirely.
pub(in crate::analytics::tuner) enum SuggestState {
    /// Nothing has run in this scope, or the last run was retired.
    Idle,
    /// A search is in flight.
    Running(SuggestJob),
    /// The last stoppable search finished.
    Done {
        /// Restarts that actually completed, not the number requested — and `None` when the run
        /// composed the field set.
        ///
        /// A composition's counter spans every candidate ranking, every fold and the final
        /// refit, so it is an internal work total rather than the restart count behind the
        /// applied thresholds. Reporting it under the same word as the plain search's would
        /// overstate that count by orders of magnitude, against a restart box the user set
        /// themselves; there is no honest single number here, so none is shown.
        rounds: Option<usize>,
        /// Whether cancellation abandoned any unit of the requested work.
        stopped: bool,
        /// Fitted and held-back scores, or `None` when the search had no answer.
        ///
        /// On an uninterrupted search, no answer means the scope held fewer trades than the
        /// minimum demanded of a suggestion.
        split: Option<SearchSplit>,
    },
    /// The last search could not read the report.
    Failed(ReadFail),
}

/// How the last suggestion's ranges scored, kept apart by whether the search was allowed to see
/// the trades being scored.
///
/// The FIGURES are held rather than recomputed, because they describe the ranges as they were
/// SUGGESTED — the moment the user edits a bound the suggestion is retired, and with it this.
/// Their formatting is deliberately left to render: pre-formatted strings would keep the labels
/// of whichever language was active when the search finished.
pub(in crate::analytics::tuner) struct SearchSplit {
    /// Over the trades the ranges were fitted on.
    pub(in crate::analytics::tuner) train: Tally,
    /// Over the trades held back from the search; `None` when the whole period was fitted on.
    pub(in crate::analytics::tuner) holdout: Option<Tally>,
    /// The field set composition chose, when it ran.
    pub(in crate::analytics::tuner) composed: Option<ComposedSet>,
    /// Why composition did not run, when it was asked for.
    pub(in crate::analytics::tuner) compose_skipped: Option<ComposeSkip>,
}

/// The searches this axis can run, which differ in what the user can do while they run.
pub(in crate::analytics::tuner) enum SuggestJob {
    /// One field over one scan — it finishes before a stop button would be reachable, so it runs
    /// under the window's blocking overlay instead.
    SingleField,
    /// Every field jointly over `total` restarts, stoppable and reporting progress via `handle`.
    AllFields {
        /// Cancellation and completed-restart counter for this run.
        handle: SearchHandle,
        /// Restarts requested for the run.
        total: usize,
    },
    /// Composition: many searches behind one handle, choosing the field set before refitting it.
    ///
    /// Deliberately carries NO total. The forward pass stops when no field is worth adding, so
    /// how much work the run will do is not known when it starts; a denominator invented here
    /// would be a number the progress caption could only contradict.
    Compose {
        /// Cancellation and progress for this run.
        handle: SearchHandle,
    },
}

impl SuggestState {
    /// Whether a search is in flight; every kind blocks starting another.
    pub(in crate::analytics::tuner) fn is_running(&self) -> bool {
        matches!(self, SuggestState::Running(_))
    }

    /// The running stoppable search's handle, and the restart count it was asked for where that
    /// number exists.
    ///
    /// `None` for the total under composition, which is not one search with one denominator but
    /// a sequence of them whose length depends on what it finds.
    pub(in crate::analytics::tuner) fn joint_run(&self) -> Option<(&SearchHandle, Option<usize>)> {
        match self {
            SuggestState::Running(SuggestJob::AllFields { handle, total }) => {
                Some((handle, Some(*total)))
            }
            SuggestState::Running(SuggestJob::Compose { handle }) => Some((handle, None)),
            _ => None,
        }
    }
}

/// Mutable state of the "By filter" tuner inside `AnalyticsView`.
pub(in crate::analytics) struct TunerState {
    /// Variant bounds as text: `[variant][field index] = (from, to)`.
    pub(in crate::analytics::tuner) bounds: Vec<Vec<(String, String)>>,
    /// Cache of the bound inputs (created lazily in render).
    pub(in crate::analytics::tuner) inputs: HashMap<String, Entity<MoonInputState>>,
    /// Histogram field (index into `FIELDS`).
    pub(in crate::analytics::tuner) sel_field: usize,
    /// KPI matrix load state; `dirty` separately marks retained data for recomputation.
    pub(in crate::analytics::tuner) stats: LoadState<Vec<VarStats>>,
    /// Selected-field histogram read state.
    pub(in crate::analytics::tuner) hist: LoadState<Vec<HistBucket>>,
    /// Filter card of the selected strategy (Ignore flags + thresholds).
    pub(in crate::analytics::tuner) strat: Arc<StratFilters>,
    /// What the auto-suggestion (the "Search" buttons) is doing, and how its last run ended.
    pub(in crate::analytics::tuner) sugg: SuggestState,
    /// The open save confirmation dialog (the list of changes).
    pub(in crate::analytics::tuner) save_dialog: Option<Arc<SaveDialog>>,
    /// KPI/strategy data is stale after a scope or report-generation change.
    pub(in crate::analytics::tuner) dirty: bool,
    /// The selected-field histogram is stale after a scope or report-generation change.
    pub(in crate::analytics::tuner) hist_dirty: bool,
    /// Whether the current histogram generation still has a database scan in flight.
    pub(in crate::analytics::tuner) hist_loading: bool,
    /// Staged state of the clickable "ignore" subheadings: flag → the desired
    /// ignore state (semantics: "ignore"; inverted for UseBV_SV_Filter).
    pub(in crate::analytics::tuner) staged_ignore: HashMap<&'static str, bool>,
    /// Restart count as raw input text; persistence stores its normalized value.
    pub(in crate::analytics::tuner) iters: String,
    /// Seed of the random restarts as raw input text; empty draws a fresh seed per search.
    pub(in crate::analytics::tuner) seed: String,
    /// Base seed the last completed search actually ran with, so an interesting result can be
    /// pinned and repeated. Survives an invalidation, which is exactly when it is wanted.
    pub(in crate::analytics::tuner) last_seed: Option<u64>,
    /// Whether the suggestion row's settings popover is open.
    pub(in crate::analytics::tuner) sugg_cfg_open: bool,
    /// Minimum trade count as raw input text; empty selects one tenth of the fitted train window.
    pub(in crate::analytics::tuner) min_trades: String,
    /// Quantile depth, always one of [`edge_options`] and persisted across window opens.
    pub(in crate::analytics::tuner) edges: usize,
    /// Percentage of the period the search may fit on, always one of `TRAIN_OPTIONS`; the rest is
    /// held back to be measured against. 100 means no split.
    pub(in crate::analytics::tuner) train_pct: usize,
    /// Whether the field takes part in the auto search (checkboxes); one that is
    /// off but has bounds acts as a fixed filter.
    pub(in crate::analytics::tuner) enabled: Vec<bool>,
    /// Whether the search also chooses WHICH of the admitted fields to filter on, out of sample,
    /// instead of searching them all. Persisted.
    pub(in crate::analytics::tuner) compose: bool,
    /// "Round the result": bounds coming out of the suggestion are rounded to 3
    /// significant digits OUTWARDS (from down, to up) — so the range does not
    /// lose the trades it found.
    pub(in crate::analytics::tuner) round_results: bool,
    /// Name of the copy being created (the "Make a copy" dialog input).
    pub(in crate::analytics::tuner) copy_name: String,
    /// KPI and selected-strategy request generation.
    pub(in crate::analytics::tuner) seq: u64,
    /// Histogram request generation.
    pub(in crate::analytics::tuner) hist_seq: u64,
    /// Suggestion request generation.
    pub(in crate::analytics::tuner) sugg_seq: u64,
    /// User-scope and draft revision guarding asynchronous confirmation-dialog preparation.
    pub(in crate::analytics::tuner) dialog_seq: u64,
}

impl TunerState {
    /// Build state for a newly opened window.
    ///
    /// Bounds reset because they belong to a strategy-specific search. Restart count, depth,
    /// seed, train share and the field checkboxes are normalized from saved preferences; with
    /// nothing saved the checkboxes fall back to "only mapped fields participate".
    ///
    /// Args:
    ///     saved_iters: Persisted random-restart count.
    ///     saved_edges: Persisted quantile depth.
    ///     saved_seed: Persisted random seed text.
    ///     saved_train: Persisted training share.
    ///     saved_fields: Persisted report-column ids admitted into the automatic search.
    ///     saved_compose: Whether the search also composes the field set. Ignored on a machine
    ///         that does not clear the heavy-search bar, where the switch is not shown either.
    ///
    /// Returns:
    ///     Normalized state for a newly opened tuner.
    pub(in crate::analytics) fn load(
        saved_iters: Option<u32>,
        saved_edges: Option<u32>,
        saved_seed: Option<String>,
        saved_train: Option<u32>,
        saved_fields: Option<Vec<String>>,
        saved_compose: bool,
    ) -> Self {
        let bounds = vec![vec![(String::new(), String::new()); FIELDS.len()]; N_VAR];
        let enabled = restore_enabled(saved_fields.as_deref());
        Self {
            bounds,
            inputs: HashMap::new(),
            sel_field: 0,
            stats: LoadState::default(),
            hist: LoadState::default(),
            strat: Arc::new(StratFilters::default()),
            sugg: SuggestState::Idle,
            save_dialog: None,
            dirty: false,
            hist_dirty: false,
            hist_loading: false,
            staged_ignore: HashMap::new(),
            iters: restore_iters(saved_iters),
            seed: restore_seed(saved_seed),
            last_seed: None,
            sugg_cfg_open: false,
            min_trades: String::new(),
            edges: restore_edges(saved_edges),
            train_pct: restore_train(saved_train),
            // A setting saved on a bigger machine must not switch on a feature this one never
            // shows a control for: the run button would silently change what it does.
            compose: saved_compose && heavy_search_supported(),
            round_results: true,
            copy_name: String::new(),
            enabled,
            seq: 0,
            hist_seq: 0,
            sugg_seq: 0,
            dialog_seq: 0,
        }
    }

    /// The checked fields as report-column ids, in [`FIELDS`] order — what persistence stores.
    ///
    /// The inverse of [`restore_enabled`]. Ids rather than positions for the same reason: the
    /// table's order is presentation order and must stay free to change.
    ///
    /// Returns:
    ///     Enabled report-column ids in [`FIELDS`] order.
    pub(in crate::analytics::tuner) fn enabled_cols(&self) -> Vec<String> {
        FIELDS
            .iter()
            .zip(self.enabled.iter())
            .filter(|(_, on)| **on)
            .map(|(s, _)| s.col.to_string())
            .collect()
    }

    /// Mark tuner calculations dirty after a scope, filter, or period change.
    ///
    /// Current data remains until recomputation completes to avoid a loading
    /// flash; a completed non-data result clears it. Recompute on mode entry or
    /// an explicit reload.
    ///
    /// The method has no return value; callers start or defer replacement reads.
    pub(in crate::analytics) fn invalidate(&mut self) {
        self.dirty = true;
        self.hist_dirty = true;
        self.seq = self.seq.wrapping_add(1);
        self.hist_seq = self.hist_seq.wrapping_add(1);
        self.hist_loading = false;
        self.invalidate_suggest();
        self.mark_dialog_draft_changed();
        self.save_dialog = None;
        self.staged_ignore.clear();
    }

    /// Retire asynchronous Save-dialog preparation after a user scope or draft change.
    pub(in crate::analytics) fn mark_dialog_draft_changed(&mut self) {
        self.dialog_seq = self.dialog_seq.wrapping_add(1);
    }

    /// Retire an auto-suggestion whose inputs or v1 destination changed manually.
    ///
    /// Advancing the generation only stops the RESULT from landing. The search itself has to be
    /// told as well, or an answer nobody will read goes on occupying every worker to the end.
    pub(in crate::analytics) fn invalidate_suggest(&mut self) {
        self.sugg_seq = self.sugg_seq.wrapping_add(1);
        self.stop_suggest();
        self.sugg = SuggestState::Idle;
    }

    /// Folds that backed `fi` under the last composed set, out of the folds it was composed on.
    ///
    /// Derived from the suggestion itself rather than mirrored into a field of its own. The badge
    /// then cannot outlive what it describes: every path that replaces or clears `sugg` — a new
    /// search, a failed read, a single-field suggestion, a manual edit — retires the badge with
    /// it, which a parallel vector only did on the paths that remembered to reset it.
    ///
    /// `Some((0, k))` is a real answer and NOT the same as `None`: a chosen field no fold could
    /// place is exactly what the support figure exists to expose.
    pub(in crate::analytics::tuner) fn compose_support(&self, fi: usize) -> Option<(u8, u8)> {
        let SuggestState::Done {
            split: Some(split), ..
        } = &self.sugg
        else {
            return None;
        };
        let set = split.composed.as_ref()?;
        let col = FIELDS[fi].col;
        set.fields
            .iter()
            .find(|(c, _)| *c == col)
            .map(|(_, support)| (*support, set.folds))
    }

    /// Whether every MAPPED field takes part in the automatic search.
    ///
    /// Unmapped fields have no strategy parameter to write, so "all enabled" has never meant all
    /// of them. One definition, because the master checkbox's tick and its caption's click both
    /// ask this question and must not answer it differently.
    pub(in crate::analytics::tuner) fn all_mapped_enabled(&self) -> bool {
        FIELDS
            .iter()
            .enumerate()
            .filter(|(_, s)| s.mapped())
            .all(|(fi, _)| self.enabled[fi])
    }

    /// Ask the running stoppable search to stop, leaving the state for the caller to set.
    ///
    /// Returns whether a stoppable search was actually running, so a Stop click can tell an
    /// interrupted run from a click that arrived after it had already finished.
    pub(in crate::analytics::tuner) fn stop_suggest(&mut self) -> bool {
        match self.sugg.joint_run() {
            Some((handle, _)) => {
                handle.cancel();
                true
            }
            None => false,
        }
    }

    /// Mark report-derived calculations stale without discarding the user's staged tuner edits.
    ///
    /// A new trade changes KPI and histogram inputs, but it does not change the selected strategy,
    /// draft bounds, staged ignore switches, or an already-open confirmation dialog.
    ///
    /// The method has no return value; callers subsequently reload the active report-derived view.
    pub(in crate::analytics) fn mark_report_stale(&mut self) {
        self.dirty = true;
        self.hist_dirty = true;
    }

    /// Apply a selected-strategy snapshot without inventing an empty automatic-refresh baseline.
    ///
    /// Args:
    ///     strat: Lossy strategy read whose `found` flag distinguishes a confirmed row.
    ///     preserve_missing: Whether an unreadable row should retain the previous baseline.
    ///
    /// Explicit scope changes pass `false` so fields from the old strategy cannot remain visible.
    pub(in crate::analytics::tuner) fn apply_strategy_read(
        &mut self,
        strat: StratFilters,
        preserve_missing: bool,
    ) {
        if strat.found || !preserve_missing {
            self.strat = Arc::new(strat);
        }
    }

    /// Whether entering the "Filters" mode requires a recomputation.
    pub(in crate::analytics) fn needs_reload(&self) -> bool {
        self.stats.data().is_none() || self.dirty || self.hist_dirty
    }

    /// Variants for the query: [the empty "Fact", v1..vN].
    pub(in crate::analytics::tuner) fn variants(&self) -> Vec<Variant> {
        let mut out = vec![Variant::default()];
        for v in &self.bounds {
            let bounds = v
                .iter()
                .enumerate()
                .filter_map(|(fi, (from, to))| {
                    let from = parse_num(from);
                    let to = parse_num(to);
                    (from.is_some() || to.is_some()).then(|| Bound {
                        field: FIELDS[fi].col.to_string(),
                        from,
                        to,
                    })
                })
                .collect();
            out.push(Variant {
                bounds,
                ..Default::default()
            });
        }
        out
    }
}

/// A number out of an input field: comma reads as a dot, k/M/B/T suffixes (and
/// the Cyrillic к/м), empty/garbage = None.
pub(in crate::analytics::tuner) fn parse_num(s: &str) -> Option<f64> {
    let trimmed = s.trim();
    // Only a decimal comma needs rewriting, and it is the rare case — every bound this reads back
    // from `fmt_bound` is already dot-separated. Borrowing otherwise keeps the allocation off a
    // path the fields grid walks a hundred times per frame.
    let s: std::borrow::Cow<'_, str> = if trimmed.contains(',') {
        std::borrow::Cow::Owned(trimmed.replace(',', "."))
    } else {
        std::borrow::Cow::Borrowed(trimmed)
    };
    if s.is_empty() {
        return None;
    }
    let (mut t, mut mult) = (s.as_ref(), 1.0f64);
    if let Some(c) = s.chars().last() {
        let m = match c {
            'k' | 'K' | 'к' | 'К' => 1e3,
            'm' | 'M' | 'м' | 'М' => 1e6,
            'b' | 'B' => 1e9,
            't' | 'T' => 1e12,
            _ => 1.0,
        };
        if m != 1.0 {
            mult = m;
            t = &s[..s.len() - c.len_utf8()];
        }
    }
    t.trim()
        .parse::<f64>()
        .ok()
        .map(|v| v * mult)
        .filter(|v| v.is_finite())
}

/// Number formatting for bounds/chips: large ones carry a k/M/B/T suffix (read
/// back by `parse_num`), the rest get up to 4 decimals with no trailing zeros.
///
/// A bound is not only displayed — it is the STORED value of a v1 threshold, read back through
/// [`parse_num`] to build the KPI query and to write the strategy. So a compact form that does
/// not survive that round trip is not a shortened number, it is a DIFFERENT filter: an upper
/// bound of `1234567` shown as `1.23M` quietly drops every trade between the two, and `0.000123`
/// shown as `0.0001` moves the threshold by a fifth. Where the compact form cannot represent the
/// value, the exact one is used instead — longer, but the number on screen is the number applied.
pub(in crate::analytics::tuner) fn fmt_bound(v: f64) -> String {
    let compact = fmt_bound_compact(v);
    if parse_num(&compact) == Some(v) {
        compact
    } else {
        // Rust's `{}` for f64 emits the shortest decimal that reads back as the same value.
        format!("{v}")
    }
}

/// The compact display form, which may not survive a `parse_num` round trip.
fn fmt_bound_compact(v: f64) -> String {
    let a = v.abs();
    let (div, suf) = if a >= 1e12 {
        (1e12, "T")
    } else if a >= 1e9 {
        (1e9, "B")
    } else if a >= 1e6 {
        (1e6, "M")
    } else if a >= 1e3 {
        (1e3, "k")
    } else {
        (1.0, "")
    };
    let x = v / div;
    let mut s = if suf.is_empty() {
        format!("{x:.4}")
    } else {
        format!("{x:.2}")
    };
    if s.contains('.') {
        while s.ends_with('0') {
            s.pop();
        }
        if s.ends_with('.') {
            s.pop();
        }
    }
    s.push_str(suf);
    s
}

pub(in crate::analytics::tuner) fn staged_dirty(
    f: &StratFilters,
    staged: &HashMap<&'static str, bool>,
) -> bool {
    staged.iter().any(|(flag, want)| {
        let cur = match *flag {
            "IgnoreFilters" => f.ignore_filters,
            "IgnorePing" => f.ignore_ping,
            "IgnoreDelta" => f.ignore_delta,
            "IgnoreVolume" => f.ignore_volume,
            "IgnoreBase" => f.ignore_base,
            "UseBV_SV_Filter" => !f.use_bvsv,
            _ => return false,
        };
        *want != cur
    })
}

pub(in crate::analytics::tuner) fn flag_of(
    class: FieldClass,
    f: &StratFilters,
) -> (&'static str, bool) {
    match class {
        FieldClass::Filter => ("IgnoreFilters", f.ignore_filters),
        FieldClass::Ping => ("IgnorePing", f.ignore_ping),
        FieldClass::Base => ("IgnoreBase", f.ignore_base),
        FieldClass::BvSv => ("UseBV_SV_Filter", !f.use_bvsv),
        FieldClass::Delta | FieldClass::DeltaSlot => ("IgnoreDelta", f.ignore_delta),
        FieldClass::Volume => ("IgnoreVolume", f.ignore_volume),
    }
}

// The sibling uses explicit imports because the parent's `gpui::*` re-export shadows `#[test]`.
#[cfg(test)]
mod tests;
