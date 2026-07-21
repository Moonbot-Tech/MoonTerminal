//! Tuner state and shared helpers (split out of tuner.rs — file size limit).

use std::collections::HashMap;
use std::sync::Arc;

use gpui::*;
use moon_ui::{MoonInputState, MoonPalette, MoonTooltipView, h_flex, v_flex};

use super::super::{AnalyticsView, LoadState};
use crate::design;
use crate::design::moon;
use moon_core::db::tuner::{
    Bound, FIELDS, FieldClass, HistBucket, StratFilters, VarStats, Variant,
};
use moon_core::db::tuner_smart::{RESTARTS_MAX, RESTARTS_MIN};

/// Variants besides "Fact" (V3 holds 8 — we start with two).
pub(super) const N_VAR: usize = 2;

/// Selectable quantile depths, restricted to values the search accepts.
pub(super) const EDGE_OPTIONS: [usize; 6] = [4, 8, 16, 32, 64, 128];

/// Depth used when nothing is chosen or a stored value is not on offer.
pub(super) const DEFAULT_EDGES: usize = 64;

/// Restarts used when the box is empty or unparseable.
pub(super) const DEFAULT_ITERS: usize = 20;

/// Convert raw "restarts" text to the count used by both the search and persistence.
///
/// Bounds come from the search module so the displayed and executed counts cannot drift.
pub(super) fn iters_of(text: &str) -> usize {
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

/// Which tuning axis draws the shared shell (toolbar + suggestion row). The shell
/// is one for every mode; the actions (Search/Save/Copy) are dispatched by kind.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum TunerKind {
    /// "By filter" — thresholds of the market fields (the full set of actions).
    Filter,
    /// "By time" — the weekly schedule (actions arrive in phase 2b).
    Time,
}

/// A write target: a strategy ON A SPECIFIC core. `core` is the list row's `core_uid`,
/// which equals the live store's `CoreId` (identity verified: the report ingest writes
/// `core_uid: server.uid`, and strategies.sqlite `core_uid` is already used as a CoreId).
/// `None` is a legacy key without a core: write to every core of the strategy (back-compat).
#[derive(Clone)]
pub(super) struct SaveTarget {
    pub(super) sid: i64,
    pub(super) core: Option<u64>,
    pub(super) name: String,
}

/// A prepared save: target(s) + the list of changes. Single selection = 1 target;
/// multi-select (Ctrl) = N targets (the same changes into each one's core). Copy = always 1.
pub(super) struct SaveDialog {
    pub(super) targets: Vec<SaveTarget>,
    pub(super) changes: Vec<(String, String)>,
    /// Current parameter values of the FIRST (anchor) target, indexed like `changes`
    /// (the dialog shows "now → next"); in multi-select the preview reflects the anchor
    /// and the rest are written blind. None — the parameter is absent from the strategy.
    pub(super) olds: Vec<Option<String>>,
    /// true — the "Make a copy" dialog: confirming creates a NEW strategy with these
    /// fields (name from the input) rather than editing the source.
    pub(super) copy: bool,
    /// Warnings (overwriting a foreign slot type, fields that did not fit) — shown in
    /// the confirmation dialog, not only in the log.
    pub(super) warns: Vec<String>,
}

/// Tuner state inside `AnalyticsView`.
pub(in crate::analytics) struct TunerState {
    /// Variant bounds as text: `[variant][field index] = (from, to)`.
    pub(super) bounds: Vec<Vec<(String, String)>>,
    /// Cache of the bound inputs (created lazily in render).
    pub(super) inputs: HashMap<String, Entity<MoonInputState>>,
    /// Histogram field (index into `FIELDS`).
    pub(super) sel_field: usize,
    /// KPI matrix load state; `dirty` separately marks retained data for recomputation.
    pub(super) stats: LoadState<Vec<VarStats>>,
    /// Selected-field histogram read state.
    pub(super) hist: LoadState<Vec<HistBucket>>,
    /// Filter card of the selected strategy (Ignore flags + thresholds).
    pub(super) strat: Arc<StratFilters>,
    /// Auto-suggestion (the "Search" buttons) is running in the background.
    pub(super) sugg_busy: bool,
    /// The open save confirmation dialog (the list of changes).
    pub(super) save_dialog: Option<Arc<SaveDialog>>,
    /// Data is stale (the scope/filters changed) — we keep showing the old one
    /// without a "Loading…" flash, but recompute on entering the mode.
    pub(super) dirty: bool,
    /// Staged state of the clickable "ignore" subheadings: flag → the desired
    /// ignore state (semantics: "ignore"; inverted for UseBV_SV_Filter).
    pub(super) staged_ignore: HashMap<&'static str, bool>,
    /// Restart count as raw input text; persistence stores its normalized value.
    pub(super) iters: String,
    /// Minimum trade count as raw input text; empty selects one fifth of the sample.
    pub(super) min_trades: String,
    /// Quantile depth, always one of `EDGE_OPTIONS` and persisted across window opens.
    pub(super) edges: usize,
    /// Whether the field takes part in the auto search (checkboxes); one that is
    /// off but has bounds acts as a fixed filter.
    pub(super) enabled: Vec<bool>,
    /// "Round the result": bounds coming out of the suggestion are rounded to 3
    /// significant digits OUTWARDS (from down, to up) — so the range does not
    /// lose the trades it found.
    pub(super) round_results: bool,
    /// Name of the copy being created (the "Make a copy" dialog input).
    pub(super) copy_name: String,
    pub(super) seq: u64,
    pub(super) hist_seq: u64,
    pub(super) sugg_seq: u64,
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
        }
    }

    /// Mark tuner calculations dirty after a scope, filter, or period change.
    ///
    /// Current data remains until recomputation completes to avoid a loading
    /// flash; a completed non-data result clears it. Recompute on mode entry or
    /// an explicit reload.
    pub(in crate::analytics) fn invalidate(&mut self) {
        self.dirty = true;
        self.save_dialog = None;
        self.staged_ignore.clear();
        // Advance every generation even when no replacement starts immediately,
        // so a request from another scope cannot publish KPI, clear `dirty`, or
        // apply v1 bounds here.
        self.seq = self.seq.wrapping_add(1);
        self.hist_seq = self.hist_seq.wrapping_add(1);
        self.sugg_seq = self.sugg_seq.wrapping_add(1);
        // Clear busy with the generation change because an orphaned request
        // exits before its completion handler can re-enable suggestion buttons.
        self.sugg_busy = false;
    }

    /// Whether entering the "Filters" mode requires a recomputation.
    pub(in crate::analytics) fn needs_reload(&self) -> bool {
        self.stats.data().is_none() || self.dirty
    }

    /// Variants for the query: [the empty "Fact", v1..vN].
    pub(super) fn variants(&self) -> Vec<Variant> {
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
pub(super) fn parse_num(s: &str) -> Option<f64> {
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
pub(super) fn fmt_bound(v: f64) -> String {
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

/// A card with a title and a subtitle (the shared look of the Analytics cards).
pub(super) fn card(
    title: String,
    sub: String,
    body: AnyElement,
    p: MoonPalette,
    cx: &Context<AnalyticsView>,
) -> AnyElement {
    let mut head = h_flex()
        .w_full()
        .px(design::ui_px(cx, 12.0))
        .py(design::ui_px(cx, 8.0))
        .items_center()
        .gap(design::ui_px(cx, 8.0))
        .child(
            div()
                .text_size(design::t_title(cx))
                .font_weight(FontWeight::SEMIBOLD)
                .child(title),
        );
    if !sub.is_empty() {
        head = head.child(
            div()
                .text_size(design::t_caption(cx))
                .text_color(moon(p.text_muted))
                .child(sub),
        );
    }
    v_flex()
        .w_full()
        // Inside a scrolling column the cards must not shrink to the viewport height.
        .flex_none()
        .rounded(design::ui_px(cx, 8.0))
        .bg(moon(p.panel))
        .border_1()
        .border_color(moon(p.border))
        .overflow_hidden()
        .child(head)
        .child(body)
        .into_any_element()
}

/// Whether any staged "ignore" differs from the strategy's current flags.
pub(super) fn staged_dirty(f: &StratFilters, staged: &HashMap<&'static str, bool>) -> bool {
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

/// A tuner-grid glyph button (→ / ← / ✕) with a hover tooltip `tip`; `hover` is the hover
/// color. SHARED by both axes ("By filter" and "By time") — the caller adds its own `.on_click`.
/// Returns a stateful div.
pub(super) fn glyph_btn(
    id: impl Into<ElementId>,
    glyph: &'static str,
    tip: String,
    hover: u32,
    p: MoonPalette,
    cx: &Context<AnalyticsView>,
) -> Stateful<Div> {
    div()
        .id(id)
        .w(design::ui_px(cx, 12.0))
        .flex_none()
        .cursor_pointer()
        .text_size(design::t_caption(cx))
        .text_color(moon(p.text_muted))
        .hover(move |s| s.text_color(moon(hover)))
        .tooltip(move |_w, cx| cx.new(|_| MoonTooltipView::new(tip.clone())).into())
        .child(glyph)
}

/// The group's ignore flag + its CURRENT state on the strategy (semantics:
/// "is ignored"; UseBV_SV_Filter is inverted — it is an enabler).
pub(super) fn flag_of(class: FieldClass, f: &StratFilters) -> (&'static str, bool) {
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
