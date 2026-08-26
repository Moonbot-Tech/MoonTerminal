//! Start/stop and field-edit actions dispatched to the session, plus name search.

use super::*;

#[cfg(test)]
mod tests;

/// Exact Start/Stop payload and workspace authority captured by the rendered action button.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct StartStopPlan {
    workspace_generation: Option<u64>,
    targets: Vec<Key>,
    actions: Vec<(CoreId, Vec<(u64, bool)>)>,
}

/// Exact field-edit payload and workspace authority captured by the rendered Apply button.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct FieldEditPlan {
    workspace_generation: Option<u64>,
    targets: Vec<Key>,
    edit_keys: Vec<FieldEditKey>,
    actions: Vec<(CoreId, Vec<(u64, Vec<(String, String)>)>)>,
}

/// Return whether every captured target still belongs to the same workspace generation.
///
/// Args:
///     captured_generation: Auto generation captured by the producer, or `None` in Classic.
///     current_generation: Auto generation visible immediately before dispatch.
///     workspace: Current concrete Auto cores, or `None` in Classic.
///     targets: Complete captured `(core, strategy)` identity set.
///
/// Returns:
///     `true` only when no target or workspace transition became stale.
pub(super) fn strategy_action_authorized(
    captured_generation: Option<u64>,
    current_generation: Option<u64>,
    workspace: Option<&[CoreId]>,
    targets: &[Key],
) -> bool {
    captured_generation == current_generation
        && targets
            .iter()
            .all(|(core, _)| strategy_core_is_visible(workspace, *core))
}

/// Return whether every captured strategy identity still resolves in the live store.
///
/// Args:
///     targets: Complete immutable target set captured by the producer.
///     exists: Live-store lookup evaluated immediately before dispatch.
///
/// Returns:
///     `true` only when every target still exists; partial batches are rejected.
fn strategy_targets_exist(targets: &[Key], mut exists: impl FnMut(Key) -> bool) -> bool {
    targets.iter().copied().all(&mut exists)
}

/// Return whether a field Apply still represents the exact current draft payload.
///
/// Args:
///     captured: Immutable plan carried by the rendered Apply button.
///     current: Fresh plan derived immediately before dispatch.
///     current_generation: Current Auto workspace generation, or `None` in Classic.
///     workspace: Current concrete Auto core set, or `None` in Classic.
///
/// Returns:
///     `true` only when authority, targets, keys, and values are all unchanged.
fn field_edit_plan_authorized(
    captured: &FieldEditPlan,
    current: &FieldEditPlan,
    current_generation: Option<u64>,
    workspace: Option<&[CoreId]>,
) -> bool {
    strategy_action_authorized(
        captured.workspace_generation,
        current_generation,
        workspace,
        &captured.targets,
    ) && captured == current
}

impl StrategiesView {
    /// Capture the complete Start/Stop payload represented by the current action button.
    ///
    /// Args:
    ///     cores: Canonically ordered cores rendered by the Strategies tree.
    ///     store: Current strategy snapshots used by that render.
    ///     cx: Application context used to capture the Auto workspace generation.
    ///
    /// Returns:
    ///     Immutable action payload; hidden retained Classic staging is not included in Auto.
    pub(super) fn start_stop_plan(
        &self,
        cores: &[(CoreId, String)],
        store: &CoreStore,
        cx: &App,
    ) -> StartStopPlan {
        let mut targets = Vec::new();
        let mut actions = Vec::new();
        for (core, _) in cores {
            let Some(cd) = store.core(*core) else {
                continue;
            };
            let mut checks = Vec::new();
            for row in &cd.strategies {
                let effective = self
                    .staged
                    .get(&(*core, row.id))
                    .copied()
                    .unwrap_or(row.checked);
                if effective != row.checked {
                    checks.push((row.id, effective));
                }
                if effective || effective != row.checked {
                    targets.push((*core, row.id));
                }
            }
            if targets
                .last()
                .is_some_and(|(target_core, _)| target_core == core)
            {
                actions.push((*core, checks));
            }
        }
        StartStopPlan {
            workspace_generation: self.action_workspace_generation(cx),
            targets,
            actions,
        }
    }

    /// Start or stop the exact checked-strategy plan captured by the rendered button.
    ///
    /// The complete target set is revalidated before the first command. A stale multi-core action
    /// is refused whole instead of being filtered to whichever targets remain visible.
    ///
    /// Args:
    ///     plan: Exact core/strategy payload captured by the producer.
    ///     start: Whether to start rather than stop the captured checked strategies.
    ///     cx: View context used to revalidate workspace authority and dispatch commands.
    ///
    /// Returns:
    ///     Nothing; failed authority or delivery preserves all retained staging.
    pub(super) fn apply_start_stop(
        &mut self,
        plan: &StartStopPlan,
        start: bool,
        cx: &mut Context<Self>,
    ) {
        let current_generation = self.action_workspace_generation(cx);
        if plan.actions.is_empty()
            || !strategy_action_authorized(
                plan.workspace_generation,
                current_generation,
                self.workspace_cores.as_deref(),
                &plan.targets,
            )
        {
            return;
        }
        // A removed strategy changes the captured identity just as surely as a hidden core. Check
        // every row before the first per-core command so no surviving subset can be sent.
        if {
            let store = self.backend.read(cx).session.store();
            !strategy_targets_exist(&plan.targets, |(core, id)| row(store, core, id).is_some())
        } {
            return;
        }
        let applied_cores: HashSet<CoreId> = plan.actions.iter().map(|(core, _)| *core).collect();
        let b = self.backend.read(cx);
        for (core, checks) in &plan.actions {
            if let Err(error) = b
                .session
                .apply_strategies(*core, checks.clone(), Some(start))
            {
                log::warn!("apply strategies failed: {error}");
                return;
            }
        }
        self.staged
            .retain(|(core, _), _| !applied_cores.contains(core));
        cx.notify();
    }

    /// Stage one field value for workspace-visible keys without changing hidden retained drafts.
    ///
    /// Args:
    ///     keys: Effective strategy keys rendered by the current parameter panel.
    ///     field: Strategy field name being edited.
    ///     value: New serialized field value.
    ///     cx: View context used to publish the visible draft change.
    ///
    /// Returns:
    ///     Nothing; version snapshots and keys outside the current workspace remain unchanged.
    pub(super) fn stage_field_value(
        &mut self,
        keys: &[Key],
        field: &str,
        value: String,
        cx: &mut Context<Self>,
    ) {
        // Persisted snapshot views are read-only, including the snapshot labeled current. Live mode
        // has no selected snapshot. Controls are disabled, and this setter guard blocks indirect
        // paths such as the color picker as a backstop.
        if keys.is_empty() || self.viewing_version() {
            return;
        }
        self.focused_field = Some(field.to_string());
        for (core, id) in keys {
            if !strategy_core_is_visible(self.workspace_cores.as_deref(), *core) {
                continue;
            }
            self.field_edits
                .insert((*core, *id, field.to_string()), value.clone());
        }
        cx.notify();
    }

    /// Capture the exact workspace-visible field-edit payload represented by Apply.
    ///
    /// Args:
    ///     cx: Application context used to capture the Auto workspace generation.
    ///
    /// Returns:
    ///     Canonical grouped edits plus every concrete `(core, strategy)` target.
    pub(super) fn field_edit_plan(&self, cx: &App) -> FieldEditPlan {
        let mut edit_keys: Vec<FieldEditKey> = self
            .field_edits
            .keys()
            .filter(|(core, _, _)| strategy_core_is_visible(self.workspace_cores.as_deref(), *core))
            .cloned()
            .collect();
        edit_keys.sort();
        let mut targets: Vec<Key> = edit_keys.iter().map(|(core, id, _)| (*core, *id)).collect();
        targets.sort_unstable();
        targets.dedup();

        let mut grouped: std::collections::BTreeMap<
            CoreId,
            std::collections::BTreeMap<u64, Vec<(String, String)>>,
        > = std::collections::BTreeMap::new();
        for (core, id, field) in &edit_keys {
            let Some(value) = self.field_edits.get(&(*core, *id, field.clone())) else {
                continue;
            };
            grouped
                .entry(*core)
                .or_default()
                .entry(*id)
                .or_default()
                .push((field.clone(), value.clone()));
        }
        let actions = grouped
            .into_iter()
            .map(|(core, strategies)| (core, strategies.into_iter().collect()))
            .collect();
        FieldEditPlan {
            workspace_generation: self.action_workspace_generation(cx),
            targets,
            edit_keys,
            actions,
        }
    }

    /// Dispatch one exact field-edit plan and retain every hidden Classic draft.
    ///
    /// The current visible plan and workspace generation must still equal the producer snapshot;
    /// otherwise every target is refused before the first core command.
    ///
    /// Args:
    ///     plan: Complete field-edit payload captured by the rendered Apply button.
    ///     cx: View context used to reach the session and publish cleared visible editors.
    ///
    /// Returns:
    ///     Nothing; a failed core dispatch returns before clearing any visible or hidden draft.
    pub(super) fn apply_field_edits(&mut self, plan: &FieldEditPlan, cx: &mut Context<Self>) {
        let current_generation = self.action_workspace_generation(cx);
        let current = self.field_edit_plan(cx);
        if plan.edit_keys.is_empty()
            || !field_edit_plan_authorized(
                plan,
                &current,
                current_generation,
                self.workspace_cores.as_deref(),
            )
        {
            return;
        }
        if {
            let store = self.backend.read(cx).session.store();
            !strategy_targets_exist(&plan.targets, |(core, id)| row(store, core, id).is_some())
        } {
            return;
        }
        let b = self.backend.read(cx);
        for (core, edits) in &plan.actions {
            if let Err(error) = b.session.edit_strategies(*core, edits.clone()) {
                log::warn!("edit strategies failed: {error}");
                return;
            }
        }
        // Clearing here is correct, not lossy: the value these keys displayed now arrives from
        // `CoreData::strategy_edit`/`strategy_edit_notes_since` (the pending/adjusted/superseded
        // tiers in `logic::edited_field_value`) instead of vanishing until the next echo. This
        // function deliberately raises no notification and keeps no intent map of its own — GitHub
        // issue #328 proposed re-adding one; that hand-rolled tracking is what this design replaces.
        self.field_edits
            .retain(|key, _| !plan.edit_keys.contains(key));
        self.clear_field_editor_cache();
        cx.notify();
    }

    /// Capture or read the singleton Auto workspace generation for delayed action guards.
    ///
    /// Args:
    ///     cx: Application context used to read the shared revision entity.
    ///
    /// Returns:
    ///     Current generation in Auto, or `None` for intentionally unscoped Classic behavior.
    pub(super) fn action_workspace_generation(&self, cx: &App) -> Option<u64> {
        self.workspace_cores.as_ref()?;
        let revision = self.backend.read(cx).workspace_revision();
        Some(revision.read(cx).generation())
    }

    /// Discard workspace-visible field drafts while preserving hidden retained Classic drafts.
    ///
    /// Args:
    ///     cx: View context used to clear visible editor widgets and request repaint.
    ///
    /// Returns:
    ///     Nothing; a scope with no visible drafts leaves all retained state untouched.
    pub(super) fn discard_field_edits(&mut self, cx: &mut Context<Self>) {
        let before = self.field_edits.len();
        self.field_edits.retain(|(core, _, _), _| {
            !strategy_core_is_visible(self.workspace_cores.as_deref(), *core)
        });
        if self.field_edits.len() == before {
            return;
        }
        self.clear_field_editor_cache();
        cx.notify();
    }

    /// Clear editor widgets after applying or discarding visible drafts, preserving hidden values.
    fn clear_field_editor_cache(&mut self) {
        self.field_inputs.clear();
        self.field_memos.clear();
        self.field_colors.clear();
        self.focused_field = None;
    }

    /// Put a strategy's full name into the search filter and focus the input for Find All by Name.
    /// Writes both the filter and input because `set_value` does not emit Change.
    pub(super) fn search_by_name(
        &mut self,
        name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.filter.search = name.clone();
        self.search.update(cx, |st, cx| {
            st.set_value(name, window, cx);
            st.focus(window, cx);
        });
        cx.notify();
    }
}
