//! Frame-to-frame reuse of the left pane's whole-account derivations.
//!
//! [`super::cache`] exists because an uncached root view re-renders on every window repaint and a
//! mouse moving over the window IS a repaint. This module is the rest of that same bill. Measured
//! 2026-08-15 on 22 cores and 1727 strategies with the tree COLLAPSED — 22 visible rows — the left
//! pane still cost 868-938 us per frame while the mouse drove 34-58 frames per second. The rows
//! were never the cost: two walks over every strategy of every visible core were, plus a per-glyph
//! measurement of the footer labels, all of them redone for a hover highlight.
//!
//! What each entry is keyed on is the whole correctness argument, and each key is exactly what its
//! producer reads — no wider, because a key that folds in unrelated state does not fail safe, it
//! just moves the walk to a different trigger:
//!
//!   * the kinds list reads the store alone, so it takes the store half of the tree signature. That
//!     half also carries the open-order digest, which the kinds list does NOT read, so an order
//!     opening or closing spends one rebuild it did not need. Bounded by data events rather than by
//!     frames — the distinction this module is about — so it is left alone rather than given a
//!     fourth digest to keep in step.
//!   * the Start/Stop plan reads the store, the staged checkboxes and the workspace generation, and
//!     nothing else. Staging comes from [`TreeSig::staged`] rather than the view half: that half
//!     also hashes the search text, and a plan the search box cannot change must not be rebuilt on
//!     every keystroke.
//!   * the footer label width reads the dictionary and the typography.
//!
//! A stale plan here is not a cosmetic defect — it is the target set the Start and Stop buttons
//! dispatch — so the completeness of that key is a test, not a comment: see
//! `the_plan_key_covers_every_field_the_plan_reads` in [`tests`]. The store half itself is covered
//! by `the_tree_cache_signature_covers_every_input_the_build_reads` in
//! `tests/theme_contract/strategies.rs`.

use std::rc::Rc;

use super::super::actions::StartStopPlan;
use super::super::*;
use super::cache::TreeSig;

#[cfg(test)]
mod tests;

/// Numeric GPUI weight the footer labels are measured at, matching their rendered weight.
const FOOTER_LABEL_WEIGHT: f32 = 400.0;

/// Strategy kinds present across the visible cores, as `(ordinal, name)` in display order.
pub(in crate::strategies) type KindList = Rc<Vec<(u8, String)>>;

/// What the left pane needs each frame that costs a whole-account walk or a font measurement.
pub(in crate::strategies) struct LeftPaneFrame {
    /// Strategy kinds present across the visible cores, for the kind filter.
    pub(super) kinds: KindList,
    /// Exact Start/Stop payload the footer buttons capture.
    pub(super) plan: Arc<StartStopPlan>,
    /// Summed glyph width of the footer's localized labels, including the staged count.
    pub(super) footer_label_width: f32,
    /// Staged checkbox count the footer both measures against and renders.
    pub(super) staged: usize,
}

/// Everything the Start/Stop plan reads, as one comparable key.
#[derive(Clone, Copy, PartialEq, Eq)]
struct PlanKey {
    /// Cores and their strategy snapshots — the store half of the tree signature.
    store: u64,
    /// Staged checkboxes, which decide both the targets and the changes to send.
    staged: u64,
    /// Auto workspace generation the plan is authorized against, or `None` in Classic.
    generation: Option<u64>,
}

/// Everything the footer label width depends on.
#[derive(Clone, PartialEq, Eq)]
struct LabelKey {
    /// Active locale: the labels come from the dictionary, and a language switch reaches windows as
    /// a bare repaint that moves nothing else here.
    locale: SharedString,
    /// Typography the widths were measured under — see [`design::text_metrics_key`].
    metrics: u64,
    /// Staged checkbox count, which the staged label spells out.
    staged: usize,
}

/// Retained results of the left pane's expensive derivations, each with the key it was built from.
#[derive(Default)]
pub(in crate::strategies) struct PaneCache {
    kinds: Option<(u64, KindList)>,
    plan: Option<(PlanKey, Arc<StartStopPlan>)>,
    labels: Option<(LabelKey, f32)>,
}

impl StrategiesView {
    /// Return the left pane's per-frame inputs, rebuilding only the ones whose data moved.
    ///
    /// Args:
    ///     sig: Tree signature already computed for this frame by the adapter cache.
    ///     cores: Visible cores in canonical order.
    ///     cx: Application context used to read the store, the workspace generation and typography.
    ///
    /// Returns:
    ///     Shared handles to the kinds list and Start/Stop plan plus the footer label width.
    pub(in crate::strategies) fn left_pane_frame(
        &mut self,
        sig: TreeSig,
        cores: &crate::core_order::OrderedCores,
        cx: &App,
    ) -> LeftPaneFrame {
        // Counted once and handed on: the footer measures against this number and also renders it.
        let staged = staged_count(self);
        LeftPaneFrame {
            kinds: self.pane_kinds(sig.store, cores, cx),
            plan: self.pane_plan(sig, cores, cx),
            footer_label_width: self.pane_footer_label_width(staged, cx),
            staged,
        }
    }

    /// Strategy kinds across the visible cores, rebuilt only when the store moved.
    fn pane_kinds(
        &mut self,
        store_sig: u64,
        cores: &crate::core_order::OrderedCores,
        cx: &App,
    ) -> KindList {
        if let Some((key, kinds)) = &self.pane_cache.kinds
            && *key == store_sig
        {
            return Rc::clone(kinds);
        }
        crate::diag::bump(&crate::diag::STRAT_PANE_BUILD);
        let built = Rc::new(kinds_present(cores, self.backend.read(cx).session.store()));
        self.pane_cache.kinds = Some((store_sig, Rc::clone(&built)));
        built
    }

    /// Start/Stop payload for the footer buttons, rebuilt only when its inputs moved.
    fn pane_plan(
        &mut self,
        sig: TreeSig,
        cores: &crate::core_order::OrderedCores,
        cx: &App,
    ) -> Arc<StartStopPlan> {
        let key = PlanKey {
            store: sig.store,
            staged: sig.staged,
            generation: self.action_workspace_generation(cx),
        };
        if let Some((cached, plan)) = &self.pane_cache.plan
            && *cached == key
        {
            return Arc::clone(plan);
        }
        crate::diag::bump(&crate::diag::STRAT_PANE_BUILD);
        let built = {
            let store = self.backend.read(cx).session.store();
            Arc::new(self.start_stop_plan(cores, store, cx))
        };
        self.pane_cache.plan = Some((key, Arc::clone(&built)));
        built
    }

    /// Summed width of the footer's labels, remeasured only on a language, typography or staging
    /// change.
    fn pane_footer_label_width(&mut self, staged: usize, cx: &App) -> f32 {
        let locale = rust_i18n::locale();
        let locale: &str = &locale;
        let metrics =
            design::text_metrics_key(cx, design::ACTION_LABEL_BASE, FOOTER_LABEL_WEIGHT, true);
        // Compared field by field rather than against a freshly built key: owning the locale means
        // allocating it, and doing that on a HIT would put a per-frame allocation in the one path
        // this module exists to keep empty.
        if let Some((cached, width)) = &self.pane_cache.labels
            && cached.staged == staged
            && cached.metrics == metrics
            && cached.locale.as_ref() == locale
        {
            return *width;
        }
        crate::diag::bump(&crate::diag::STRAT_PANE_BUILD);
        let width = footer_label_width(cx, staged);
        let key = LabelKey {
            locale: SharedString::from(locale.to_string()),
            metrics,
            staged,
        };
        self.pane_cache.labels = Some((key, width));
        width
    }
}

/// Return the distinct strategy kinds present across `cores`, ordered by displayed name.
///
/// Private to this module on purpose: it walks every strategy of every visible core, so the only
/// legitimate caller is the keyed one above. Privacy says that to the compiler; a comment would
/// only say it to a reader.
///
/// Args:
///     cores: Visible cores in canonical order.
///     store: Per-core data behind the strategy rows.
///
/// Returns:
///     `(ordinal, name)` per kind, ordered case-insensitively by name.
fn kinds_present(cores: &[(CoreId, String)], store: &CoreStore) -> Vec<(u8, String)> {
    let mut map: std::collections::BTreeMap<u8, String> = std::collections::BTreeMap::new();
    for (c, _) in cores {
        if let Some(cd) = store.core(*c) {
            for r in &cd.strategies {
                map.entry(r.kind_ordinal).or_insert_with(|| r.kind.clone());
            }
        }
    }
    let mut v: Vec<(u8, String)> = map.into_iter().collect();
    v.sort_by_key(|(_, name)| name.to_lowercase());
    v
}

/// Measure one full set of footer labels at the size and weight they are rendered with.
///
/// Every measurement is a per-character glyph advance with no cache underneath
/// (`design::ui_text_width`), which is why the result is retained rather than recomputed per frame.
///
/// Args:
///     cx: Application context providing the text system and font tokens.
///     staged: Staged checkbox count; zero renders no staged label and measures none.
///
/// Returns:
///     Summed glyph width of the labels the footer would show at full density.
fn footer_label_width(cx: &App, staged: usize) -> f32 {
    let mut width: f32 = [
        t!("strat.action_copy"),
        t!("strat.action_paste"),
        t!("strat.action_delete"),
        t!("strat.start_checked"),
        t!("strat.stop_checked"),
    ]
    .iter()
    .map(|label| {
        design::ui_text_width(
            cx,
            label,
            design::ACTION_LABEL_BASE,
            FOOTER_LABEL_WEIGHT,
            true,
        )
    })
    .sum();
    if staged > 0 {
        width += design::ui_text_width(
            cx,
            &t!("strat.staged", n = staged),
            design::ACTION_LABEL_BASE,
            FOOTER_LABEL_WEIGHT,
            true,
        );
    }
    width
}
