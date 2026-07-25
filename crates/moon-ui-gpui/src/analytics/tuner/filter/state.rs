//! Tuner state and shared helpers (split out of tuner.rs — file size limit).

use std::collections::HashMap;
use std::sync::Arc;

use gpui::*;
use moon_ui::MoonInputState;

use super::super::super::LoadState;
use super::super::shared::{N_VAR, SaveDialog};
use moon_core::db::tuner::{
    Bound, FIELDS, FieldClass, HistBucket, StratFilters, VarStats, Variant,
};
use moon_core::db::tuner_smart::{RESTARTS_MAX, RESTARTS_MIN};

/// Selectable quantile depths, restricted to values the search accepts.
pub(in crate::analytics::tuner) const EDGE_OPTIONS: [usize; 6] = [4, 8, 16, 32, 64, 128];

/// Depth used when nothing is chosen or a stored value is not on offer.
pub(super) const DEFAULT_EDGES: usize = 64;

/// Restarts used when the box is empty or unparseable.
pub(in crate::analytics::tuner) const DEFAULT_ITERS: usize = 20;

/// Convert raw "restarts" text to the count used by both the search and persistence.
///
/// Bounds come from the search module so the displayed and executed counts cannot drift.
pub(in crate::analytics::tuner) fn iters_of(text: &str) -> usize {
    text.trim()
        .parse::<usize>()
        .unwrap_or(DEFAULT_ITERS)
        .clamp(RESTARTS_MIN, RESTARTS_MAX)
}

/// Box text to open the tuner with for a persisted restart count.
///
/// Missing values use the default; out-of-range values are clamped to the search bounds.
pub(super) fn restore_iters(saved: Option<u32>) -> String {
    saved
        .map_or(DEFAULT_ITERS, |v| {
            (v as usize).clamp(RESTARTS_MIN, RESTARTS_MAX)
        })
        .to_string()
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
    /// Auto-suggestion (the "Search" buttons) is running in the background.
    pub(in crate::analytics::tuner) sugg_busy: bool,
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
    /// Minimum trade count as raw input text; empty selects one fifth of the sample.
    pub(in crate::analytics::tuner) min_trades: String,
    /// Quantile depth, always one of `EDGE_OPTIONS` and persisted across window opens.
    pub(in crate::analytics::tuner) edges: usize,
    /// Whether the field takes part in the auto search (checkboxes); one that is
    /// off but has bounds acts as a fixed filter.
    pub(in crate::analytics::tuner) enabled: Vec<bool>,
    /// "Round the result": bounds coming out of the suggestion are rounded to 3
    /// significant digits OUTWARDS (from down, to up) — so the range does not
    /// lose the trades it found.
    pub(in crate::analytics::tuner) round_results: bool,
    /// Name of the copy being created (the "Make a copy" dialog input).
    pub(in crate::analytics::tuner) copy_name: String,
    pub(in crate::analytics::tuner) seq: u64,
    pub(in crate::analytics::tuner) hist_seq: u64,
    pub(in crate::analytics::tuner) sugg_seq: u64,
    /// User-scope and draft revision guarding asynchronous confirmation-dialog preparation.
    pub(in crate::analytics::tuner) dialog_seq: u64,
}

impl TunerState {
    /// Build state for a newly opened window.
    ///
    /// Bounds reset and only mapped fields participate because both belong to a
    /// strategy-specific search. Restart count and depth are normalized from saved preferences.
    pub(in crate::analytics) fn load(saved_iters: Option<u32>, saved_edges: Option<u32>) -> Self {
        let bounds = vec![vec![(String::new(), String::new()); FIELDS.len()]; N_VAR];
        let enabled = FIELDS.iter().map(|s| s.mapped()).collect();
        Self {
            bounds,
            inputs: HashMap::new(),
            sel_field: 0,
            stats: LoadState::default(),
            hist: LoadState::default(),
            strat: Arc::new(StratFilters::default()),
            sugg_busy: false,
            save_dialog: None,
            dirty: false,
            hist_dirty: false,
            hist_loading: false,
            staged_ignore: HashMap::new(),
            iters: restore_iters(saved_iters),
            min_trades: String::new(),
            edges: restore_edges(saved_edges),
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
        self.sugg_seq = self.sugg_seq.wrapping_add(1);
        self.sugg_busy = false;
        self.mark_dialog_draft_changed();
        self.save_dialog = None;
        self.staged_ignore.clear();
    }

    /// Retire asynchronous Save-dialog preparation after a user scope or draft change.
    pub(in crate::analytics) fn mark_dialog_draft_changed(&mut self) {
        self.dialog_seq = self.dialog_seq.wrapping_add(1);
    }

    /// Retire an auto-suggestion whose inputs or v1 destination changed manually.
    pub(in crate::analytics) fn invalidate_suggest(&mut self) {
        self.sugg_seq = self.sugg_seq.wrapping_add(1);
        self.sugg_busy = false;
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
    let s = s.trim().replace(',', ".");
    if s.is_empty() {
        return None;
    }
    let (mut t, mut mult) = (s.as_str(), 1.0f64);
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
pub(in crate::analytics::tuner) fn fmt_bound(v: f64) -> String {
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
