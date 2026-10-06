//! Core Status scope resolution and problem read tracking.

use super::{CoreStatusView, problems};
use crate::Backend;
use crate::workspace::scope_marker::ScopeMarker;
use crate::workspace::{EffectiveCoreScope, RetainedCoreScope};
use gpui::*;
use moon_core::session::CoreId;
use moon_core::session::core_order::{CoreOrder, OrderedCores};
use moon_ui::Panel;

impl CoreStatusView {
    /// Return this panel group's cores in canonical order.
    ///
    /// Unfiltered by workspace-preset membership on purpose — the retained-selection callers
    /// (`toggle_exchange_cores`, `adopt_broadcast_core_filter`) reconcile `sel_cores` against this
    /// list independently of what is currently displayed. The interactive picker uses
    /// [`Self::displayed_scope_cores`] instead.
    pub(in crate::panels) fn scope_cores(&self, b: &Backend) -> OrderedCores {
        CoreOrder::new(&b.config).from_sessions(b.session.sessions(), |s| s.group == self.group)
    }

    /// Return this panel group's cores the active preset actually displays — what the `core_bar`
    /// picker should offer, so it agrees with the rows, which already route through
    /// `effective_workspace_scope`.
    pub(in crate::panels) fn displayed_scope_cores(&self, b: &Backend) -> OrderedCores {
        let preset = b.display_preset(crate::workspace::DisplayOwner::Group(&self.group));
        CoreOrder::new(&b.config).from_sessions(b.session.sessions(), |s| {
            s.group == self.group && b.core_displayed(preset, s.id)
        })
    }

    /// Resolve the effective core scope without overwriting the retained Classic filter.
    ///
    /// Args:
    ///     b: Backend snapshot containing workspace authority and live group membership.
    ///
    /// Returns:
    ///     Effective core scope used by status data and controls.
    pub(in crate::panels) fn effective_scope(&self, b: &Backend) -> EffectiveCoreScope {
        let retained: Vec<CoreId> = self.sel_cores.iter().copied().collect();
        let retained = if retained.is_empty() {
            RetainedCoreScope::All
        } else {
            RetainedCoreScope::Explicit(&retained)
        };
        b.effective_workspace_scope(&self.group, retained)
    }

    /// Recount the findings this operator has not looked at yet.
    ///
    /// A finding counts as unseen while its KIND is missing from that core's seen set — see
    /// `TabBadgeSettings::seen_kinds` for why identity rather than a timestamp.
    ///
    /// Args:
    ///     b: Backend snapshot holding the store and the persisted sets.
    ///
    /// Returns:
    ///     How many findings in scope have not been looked at.
    pub(super) fn count_unseen_problems(&self, b: &Backend) -> usize {
        if !b.tab_badges.counters_visible(self.panel_name()) {
            return 0;
        }
        // Walked with the SAME budget the render arm truncates to, in the same order. Counting the
        // uncapped store instead would light a badge whose tail cores are never drawn, and which
        // could therefore never reach zero.
        let mut budget = problems::PROBLEM_LIST_LIMIT;
        let mut unseen = 0usize;
        for core in self.effective_scope(b).ids().iter().copied() {
            if budget == 0 {
                break;
            }
            let Some(data) = b.session.store().core(core) else {
                continue;
            };
            if !data.problems.supported {
                continue;
            }
            for problem in data.problems.items.iter().take(budget) {
                budget -= 1;
                if !b
                    .tab_badges
                    .core_kind_seen(self.panel_name(), &self.group, core, problem.kind)
                {
                    unseen += 1;
                }
            }
        }
        unseen
    }

    /// Record the findings on screen as looked at.
    ///
    /// Driven by the ROWS being drawn rather than by the store, so the list cap cannot consume a
    /// finding the surface never showed. Called from the Problems arm under a window-active guard —
    /// the News panel's rule, and for its reason: drawing the list IS reading it, while "the tab was
    /// in front while you worked in another app" is not.
    ///
    /// KNOWN LIMIT, shared with News: scrolling is not tracked. Opening the mode marks everything
    /// the merged list holds, including rows below the fold. The alternative — consuming only what
    /// the virtual list built this frame — would leave a badge lit for rows the operator has no way
    /// to know about, which is worse than the reverse.
    ///
    /// Args:
    ///     rows: The findings being rendered, already scoped and capped.
    ///     cx: View context used to mutate the backend and repaint.
    ///
    /// Returns:
    ///     Nothing; a no-op when every core's set already matches what is shown.
    pub(super) fn mark_problems_seen(
        &mut self,
        rows: &[problems::ProblemRow],
        cx: &mut Context<Self>,
    ) {
        // Skips only a REPEAT of the same rows. See `problems_mark_sig` for why a "nothing unseen"
        // test cannot stand in for this.
        let sig = problems::mark_signature(rows);
        if sig == self.problems_mark_sig {
            return;
        }
        self.problems_mark_sig = sig;
        let panel = self.panel_name();
        let group = self.group.clone();
        let marks: Vec<(CoreId, Vec<u8>)> = {
            let b = self.backend.read(cx);
            self.effective_scope(b)
                .ids()
                .iter()
                .copied()
                // A core that has NOT answered is skipped entirely rather than marked empty. The
                // store clears `problems` on a replacement connection, so a core caught mid-
                // reconnect shows nothing — and since marking REPLACES the set, marking it here
                // would forget what was already read and let the identical re-sent list light the
                // badge again. `supported` is precisely "this core has answered on this
                // connection".
                .filter(|core| {
                    b.session
                        .store()
                        .core(*core)
                        .is_some_and(|data| data.problems.supported)
                })
                .map(|core| (core, problems::drawn_kinds(rows, core)))
                .collect()
        };
        let mut changed = false;
        self.backend.update(cx, |b, bcx| {
            for (core, kinds) in marks {
                changed |= b
                    .tab_badges
                    .mark_core_kinds_seen(panel, &group, core, &kinds);
            }
            if changed {
                b.tab_badges_dirty = true;
                bcx.notify();
            }
        });
        if changed {
            let b = self.backend.clone();
            self.unseen_problems = self.count_unseen_problems(b.read(cx));
            // The backend notify above is raised mid-draw, where the window suppresses it for the
            // entity it is already drawing — so this view has to ask for its own next frame or the
            // dock tab keeps the count just consumed. News draws the same line for the same reason.
            cx.notify();
        }
    }

    /// Build this panel's scope marker for the footer and empty-state text.
    ///
    /// Args:
    ///     b: Backend snapshot containing workspace authority and live group membership.
    ///
    /// Returns:
    ///     A marker built from the membership boundary's own counts. Unlike Assets, every
    ///     instance here is scoped to a window group, so there is no unscoped variant to return
    ///     `None` for.
    pub(in crate::panels) fn scope_marker(&self, b: &Backend) -> ScopeMarker {
        let scope = self.effective_scope(b);
        ScopeMarker::new(
            b.display_preset(crate::workspace::DisplayOwner::Group(&self.group)),
            scope.membership_shown(),
            scope.membership_total(),
        )
    }

    /// Return canonically ordered core/name pairs in the current effective scope.
    ///
    /// Args:
    ///     b: Backend snapshot containing canonical configured core order.
    ///
    /// Returns:
    ///     Effective core/name pairs for queries and caches.
    pub(in crate::panels) fn query_cores(&self, b: &Backend) -> Vec<(CoreId, String)> {
        let scope = self.effective_scope(b);
        self.scope_cores(b)
            .into_iter()
            .filter(|(core, _)| scope.contains(*core))
            .collect()
    }
}
