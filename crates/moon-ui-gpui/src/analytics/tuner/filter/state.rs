//! State, persisted search settings, and numeric helpers for the "By filter" tuner.

use std::collections::HashMap;
use std::sync::Arc;

use gpui::*;
use moon_ui::MoonInputState;

use super::super::super::LoadState;
use super::super::shared::{N_VAR, SaveDialog};
use moon_core::db::ReadFail;
use moon_core::db::metrics::Tally;
use moon_core::db::tuner::threshold_search::{RESTARTS_MAX, RESTARTS_MIN, SearchHandle};
use moon_core::db::tuner::{
    Bound, FIELDS, FieldClass, HistBucket, StratFilters, VarStats, Variant,
};

/// Selectable quantile depths, restricted to values the search accepts.
pub(in crate::analytics::tuner) const EDGE_OPTIONS: [usize; 7] = [4, 8, 16, 32, 64, 128, 256];

/// Depth used when nothing is chosen or a stored value is not on offer.
///
/// Must be one of [`EDGE_OPTIONS`], or the dropdown would open showing a value none of its items
/// is marked as.
pub(super) const DEFAULT_EDGES: usize = 32;

/// Restarts used when the box is empty or unparseable.
pub(in crate::analytics::tuner) const DEFAULT_ITERS: usize = 100;

/// Convert raw "restarts" text to the count used by both the search and persistence.
///
/// Read through [`parse_num`], so the same `k` suffix the box DISPLAYS is one it accepts back:
/// the ceiling is five digits and the box is narrow enough that `20000` shows only its tail.
/// Bounds come from the search module so the displayed and executed counts cannot drift.
pub(in crate::analytics::tuner) fn iters_of(text: &str) -> usize {
    parse_num(text)
        .filter(|v| *v >= 0.0)
        .map(|v| v.min(RESTARTS_MAX as f64) as usize)
        .unwrap_or(DEFAULT_ITERS)
        .clamp(RESTARTS_MIN, RESTARTS_MAX)
}

/// Box text to open the tuner with for a persisted restart count.
///
/// Compact where that is EXACT — 20000 opens as `20k`, which fits the box, while 1234 stays
/// `1234` because `1.23k` is a different number. [`fmt_bound`] owns that distinction, and
/// [`iters_of`] reads either form back, so the box can never display a count the search would
/// not run.
///
/// Missing values use the default; out-of-range values are clamped to the search bounds.
pub(super) fn restore_iters(saved: Option<u32>) -> String {
    let count = saved.map_or(DEFAULT_ITERS, |v| {
        (v as usize).clamp(RESTARTS_MIN, RESTARTS_MAX)
    });
    fmt_bound(count as f64)
}

/// Return `v` when the dropdown offers it, otherwise the default depth.
pub(super) fn edges_of(v: usize) -> usize {
    if EDGE_OPTIONS.contains(&v) {
        v
    } else {
        DEFAULT_EDGES
    }
}

/// Depth to open the tuner with for a persisted value.
pub(super) fn restore_edges(saved: Option<u32>) -> usize {
    saved.map_or(DEFAULT_EDGES, |v| edges_of(v as usize))
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
    /// The last joint search finished.
    Done {
        /// Restarts that actually completed, not the number requested.
        rounds: usize,
        /// Whether cancellation abandoned any requested restart.
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
}

/// The two searches this axis can run, which differ in what the user can do while they run.
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
}

impl SuggestState {
    /// Whether a search is in flight; both kinds block starting another.
    pub(in crate::analytics::tuner) fn is_running(&self) -> bool {
        matches!(self, SuggestState::Running(_))
    }

    /// The running joint search's handle and requested restart count, if that is what is running.
    pub(in crate::analytics::tuner) fn joint_run(&self) -> Option<(&SearchHandle, usize)> {
        match self {
            SuggestState::Running(SuggestJob::AllFields { handle, total }) => {
                Some((handle, *total))
            }
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
    /// Quantile depth, always one of `EDGE_OPTIONS` and persisted across window opens.
    pub(in crate::analytics::tuner) edges: usize,
    /// Percentage of the period the search may fit on, always one of `TRAIN_OPTIONS`; the rest is
    /// held back to be measured against. 100 means no split.
    pub(in crate::analytics::tuner) train_pct: usize,
    /// Whether the field takes part in the auto search (checkboxes); one that is
    /// off but has bounds acts as a fixed filter.
    pub(in crate::analytics::tuner) enabled: Vec<bool>,
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
    /// Bounds reset and only mapped fields participate because both belong to a
    /// strategy-specific search. Restart count, depth, seed and train share are normalized from
    /// saved preferences.
    pub(in crate::analytics) fn load(
        saved_iters: Option<u32>,
        saved_edges: Option<u32>,
        saved_seed: Option<String>,
        saved_train: Option<u32>,
    ) -> Self {
        let bounds = vec![vec![(String::new(), String::new()); FIELDS.len()]; N_VAR];
        let enabled = FIELDS.iter().map(|s| s.mapped()).collect();
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
            round_results: true,
            copy_name: String::new(),
            enabled,
            seq: 0,
            hist_seq: 0,
            sugg_seq: 0,
            dialog_seq: 0,
        }
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

    /// Ask the running joint search to stop, leaving the state for the caller to set.
    ///
    /// Returns whether a joint search was actually running, so a Stop click can tell an
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
