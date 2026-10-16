//! Start/stop and field-edit actions dispatched to the session, plus name search.

use super::*;

// The action owns list editing, including its conventional sibling test module.
pub(super) mod list_edit;
use list_edit::{ListEdit, stage_list_values};

#[cfg(test)]
mod tests;

/// Selection and workspace authority captured when opening a list-operation dialog.
#[derive(Clone)]
pub(super) struct ListEditTarget {
    pub(super) workspace_generation: Option<u64>,
    pub(super) keys: Vec<Key>,
    pub(super) field: String,
}

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
    /// The grouped payload, and with it the VALUES: `edit_keys` names the fields, so this is what
    /// makes the pre-dispatch `captured == current` comparison notice a retyped value. Dispatch
    /// regroups the sendable subset rather than sending this.
    actions: Vec<(CoreId, Vec<(u64, Vec<(String, String)>)>)>,
}

impl StartStopPlan {
    /// How many strategies the Start and Stop buttons act on: every captured target — the
    /// strategies that run once ticked, plus the staged unticks the same press sends.
    ///
    /// Zero exactly when the plan holds no per-core action, which is when `apply_start_stop`
    /// returns without sending anything; the footer disables both buttons on it.
    pub(super) fn target_count(&self) -> usize {
        self.targets.len()
    }

    /// Number of core engines the captured action changes, including their filtered-out rows.
    pub(super) fn core_count(&self) -> usize {
        self.actions.len()
    }

    /// Retire only staging sent by this successful engine batch, preserving excluded cores.
    fn clear_applied_staging(&self, staged: &mut HashMap<Key, bool>) {
        let applied_cores: HashSet<CoreId> = self.actions.iter().map(|(core, _)| *core).collect();
        staged.retain(|(core, _), _| !applied_cores.contains(core));
    }
}

/// Resolve bulk engine scope from the tree's actual navigation rows when any filter is active.
/// A collapsed core still has a row; a core behind a collapsed section has none. Without filters,
/// `None` preserves the existing whole-workspace action regardless of tree expansion.
///
/// Args:
///     filter: Current editable tree filter.
///     nav: Navigation rows emitted by the tree builder under that filter and expansion.
///
/// Returns:
///     Visible core identities, or `None` when the tree is unfiltered.
pub(super) fn start_stop_core_scope(
    filter: &filter::StrategyFilter,
    nav: &[tree::ops::NavNode],
) -> Option<HashSet<CoreId>> {
    filter.narrows().then(|| {
        nav.iter()
            .filter_map(|node| match node {
                tree::ops::NavNode::Core(core) => Some(*core),
                _ => None,
            })
            .collect()
    })
}

/// Build the ordered engine payload only for cores whose tree visibility is known.
/// `None` admits every supplied workspace core; an empty visible set admits nothing.
///
/// Args:
///     cores: Canonically ordered workspace cores.
///     data: Live snapshot lookup; a missing snapshot excludes that core.
///     staged: Retained checkbox changes, including cores outside the action scope.
///     visible: Filtered tree's core identities, or `None` to admit all workspace cores.
///
/// Returns:
///     Ordered targets and per-core checkbox payloads for the admitted engines.
fn start_stop_payload<'a>(
    cores: &[(CoreId, String)],
    mut data: impl FnMut(CoreId) -> Option<&'a moon_core::session::store::CoreData>,
    staged: &HashMap<Key, bool>,
    visible: Option<&HashSet<CoreId>>,
) -> (Vec<Key>, Vec<(CoreId, Vec<(u64, bool)>)>) {
    let mut targets = Vec::new();
    let mut actions = Vec::new();
    for (core, _) in cores {
        if visible.is_some_and(|ids| !ids.contains(core)) {
            continue;
        }
        let Some(cd) = data(*core) else {
            continue;
        };
        let (core_targets, checks) = core_start_stop_targets(*core, &cd.strategies, staged);
        if !core_targets.is_empty() {
            targets.extend(core_targets);
            actions.push((*core, checks));
        }
    }
    (targets, actions)
}

impl FieldEditPlan {
    /// Capture visible draft identities and values without requiring a UI context.
    ///
    /// `edits` retains every core's drafts; `workspace` only confines this plan. The generation
    /// is captured by the view so delayed clicks retain the existing authority checks.
    fn capture(
        edits: &HashMap<FieldEditKey, String>,
        workspace: Option<&[CoreId]>,
        workspace_generation: Option<u64>,
    ) -> Self {
        let mut edit_keys: Vec<FieldEditKey> = edits
            .keys()
            .filter(|(core, _, _)| strategy_core_is_visible(workspace, *core))
            .cloned()
            .collect();
        edit_keys.sort();
        let mut targets: Vec<Key> = edit_keys.iter().map(|(core, id, _)| (*core, *id)).collect();
        targets.sort_unstable();
        targets.dedup();
        let actions = group_field_edits(edits, &edit_keys);
        Self {
            workspace_generation,
            targets,
            edit_keys,
            actions,
        }
    }

    /// The drafts this plan covers, for a caller that must narrow them (see
    /// [`StrategiesView::sendable_field_edits`]).
    pub(super) fn edit_keys(&self) -> &[FieldEditKey] {
        &self.edit_keys
    }
}

/// Drop drafts whose `(core, id)` no longer appears in that core's live strategy snapshot.
///
/// The store is authoritative regardless of selection, workspace visibility, or deletion source.
/// Returns whether any draft was removed; an empty map needs no store scan.
fn retain_live_field_edits(edits: &mut HashMap<FieldEditKey, String>, store: &CoreStore) -> bool {
    if edits.is_empty() {
        return false;
    }
    let live = kind_ordinals(store, edits.keys().map(|(core, id, _)| (*core, *id)));
    let before = edits.len();
    edits.retain(|(core, id, _), _| live.contains_key(&(*core, *id)));
    edits.len() != before
}

/// Group draft keys and their retained values into the canonical per-core command payload.
///
/// Shared by plan capture and dispatch of its sendable subset, preserving field order in both.
fn group_field_edits(
    edits: &HashMap<FieldEditKey, String>,
    keys: &[FieldEditKey],
) -> Vec<(CoreId, Vec<(u64, Vec<(String, String)>)>)> {
    let mut grouped: std::collections::BTreeMap<
        CoreId,
        std::collections::BTreeMap<u64, Vec<(String, String)>>,
    > = std::collections::BTreeMap::new();
    for (core, id, field) in keys {
        let Some(value) = edits.get(&(*core, *id, field.clone())) else {
            continue;
        };
        grouped
            .entry(*core)
            .or_default()
            .entry(*id)
            .or_default()
            .push((field.clone(), value.clone()));
    }
    grouped
        .into_iter()
        .map(|(core, strategies)| (core, strategies.into_iter().collect()))
        .collect()
}

/// Return whether every captured target still belongs to the same workspace generation.
///
/// Args:
///     captured_generation: Generation captured by the producer, or `None` when unscoped.
///     current_generation: Generation visible immediately before dispatch, or `None` when unscoped.
///     workspace: Concrete scoped ids from Auto or Classic membership, or `None` when unscoped.
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
///     current_generation: Current workspace generation, or `None` when unscoped.
///     workspace: Concrete scoped ids from Auto or Classic membership, or `None` when unscoped.
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

/// Enable refresh only for supported MoonShot drafts with no overlapping core edit.
///
/// The raw names are the wire/schema identifiers. An unchanged resubmission is not a forced
/// refresh; the core remains responsible for deciding whether an accepted field changed.
fn buy_refresh_enabled<'a>(
    fields: impl IntoIterator<Item = (&'a str, &'a str)>,
    has_open_edit: bool,
) -> bool {
    !has_open_edit
        && fields.into_iter().any(|(kind, name)| {
            kind == "MoonShot"
                && matches!(
                    name,
                    "OrderSize" | "Short" | "EmulatorMode" | "AutoCancelBuy"
                )
        })
}

/// One core's share of the Start/Stop plan: what the buttons act on, and the staged ticks they send.
///
/// A row is a target when its effective state (staged over the core's own) is ticked, or when
/// staging changes it — a staged untick is sent by the same press.
///
/// Args:
///     core: Core the rows belong to.
///     rows: The core's live strategy rows, in its own order.
///     staged: Retained staged checkbox values across every core.
///
/// Returns:
///     `(targets, checks)`: target keys in row order, and the `(id, value)` staging differences.
pub(super) fn core_start_stop_targets(
    core: CoreId,
    rows: &[StrategyRow],
    staged: &HashMap<Key, bool>,
) -> (Vec<Key>, Vec<(u64, bool)>) {
    let mut targets = Vec::new();
    let mut checks = Vec::new();
    for row in rows {
        let effective = staged.get(&(core, row.id)).copied().unwrap_or(row.checked);
        if effective != row.checked {
            checks.push((row.id, effective));
        }
        if effective || effective != row.checked {
            targets.push((core, row.id));
        }
    }
    (targets, checks)
}

impl StrategiesView {
    /// Capture the complete Start/Stop payload represented by the current action button.
    ///
    /// Args:
    ///     cores: Canonically ordered cores rendered by the Strategies tree.
    ///     store: Current strategy snapshots used by that render.
    ///     visible: Filtered tree's core rows, or `None` when no filter narrows the tree.
    ///     cx: Application context used to capture the current workspace generation.
    ///
    /// Returns:
    ///     Immutable engine payload covering checked rows and checkbox changes on admitted cores;
    ///     staging on cores outside the current tree scope is not included.
    pub(super) fn start_stop_plan(
        &self,
        cores: &[(CoreId, String)],
        store: &CoreStore,
        visible: Option<&HashSet<CoreId>>,
        cx: &App,
    ) -> StartStopPlan {
        let (targets, actions) =
            start_stop_payload(cores, |core| store.core(core), &self.staged, visible);
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
        // Rebuild visibility at the click, rather than trusting last frame's navigation. A filter,
        // expansion, checkbox or feed change invalidates the whole rendered batch before sending.
        let current = {
            let backend = self.backend.read(cx);
            let store = backend.session.store();
            let cores = visible_strategy_cores(self, backend);
            let visible = self.filter.narrows().then(|| {
                let built = tree::moon::build(self, store, &cores, backend.session.core_venues());
                start_stop_core_scope(&self.filter, &built.nav).unwrap_or_default()
            });
            self.start_stop_plan(&cores, store, visible.as_ref(), cx)
        };
        if *plan != current {
            return;
        }
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
        plan.clear_applied_staging(&mut self.staged);
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

    /// Stage a list operation against each strategy's current draft, pending edit, or live value.
    ///
    /// The dialog captures the selection and workspace generation. Revalidate the entire set and
    /// every field before writing anything; a delayed submit must never edit a surviving subset.
    /// Returns false for a stale dialog and true for a valid operation, including an empty no-op.
    pub(super) fn stage_list_field_value(
        &mut self,
        target: &ListEditTarget,
        entered: &str,
        operation: ListEdit,
        cx: &mut Context<Self>,
    ) -> bool {
        if !strategy_action_authorized(
            target.workspace_generation,
            self.action_workspace_generation(cx),
            self.workspace_cores.as_deref(),
            &target.keys,
        ) {
            return false;
        }
        let current = {
            let store = self.backend.read(cx).session.store();
            list_field_values(self, store, &target.keys, &target.field)
        };
        let Some(current) = current else {
            return false;
        };
        stage_list_values(
            &mut self.field_edits,
            &target.field,
            &current,
            entered,
            operation,
        );
        self.focused_field = Some(target.field.clone());
        cx.notify();
        true
    }

    /// Capture the exact workspace-visible field-edit payload represented by Apply.
    ///
    /// Args:
    ///     cx: Application context used to capture the current workspace generation.
    ///
    /// Returns:
    ///     Canonical grouped edits plus every concrete `(core, strategy)` target.
    pub(super) fn field_edit_plan(&self, cx: &App) -> FieldEditPlan {
        FieldEditPlan::capture(
            &self.field_edits,
            self.workspace_cores.as_deref(),
            self.action_workspace_generation(cx),
        )
    }

    /// Reconcile unsent drafts with the live store and retire widgets holding removed values.
    ///
    /// Called on store refresh, including hidden cores, and after a racing Apply is refused.
    /// Returns whether the action bar needs repainting; surviving drafts are preserved.
    pub(super) fn reconcile_field_edits(&mut self, store: &CoreStore) -> bool {
        if !retain_live_field_edits(&mut self.field_edits, store) {
            return false;
        }
        self.clear_field_editor_cache();
        true
    }

    /// The drafts among `keys` that the core would actually accept.
    ///
    /// Borrowed, not cloned, because the tab strip's Apply calls this every frame only to count them.
    ///
    /// The rejection test needs live store state (a draft is keyed by field NAME, and only the
    /// strategy's own kind says what type that name has), so it deliberately stays OUT of
    /// [`Self::field_edit_plan`]: the captured plan is compared with a fresh one before dispatch,
    /// and a schema that came or went between the render and the click would fail that comparison
    /// and drop the press with no explanation. Applied at dispatch instead, a store change costs
    /// nothing but the accuracy of one button label for one frame.
    pub(super) fn sendable_field_edits<'a>(
        &'a self,
        keys: &'a [FieldEditKey],
        store: &CoreStore,
    ) -> Vec<&'a FieldEditKey> {
        let kinds = kind_ordinals(store, keys.iter().map(|(core, id, _)| (*core, *id)));
        // The schema lookup is memoized, the verdict is NOT: drafts of one multi-selection share a
        // field name but need not share its text, and judging the second by the first would let one
        // row's typo decide for another.
        let mut fields: HashMap<(CoreId, u8, &str), Option<&SchemaField>> = HashMap::new();
        keys.iter()
            .filter(|key| {
                let (core, id, name) = key;
                let Some(text) = self.field_edits.get(*key) else {
                    return false;
                };
                // No live row means there is no valid recipient. A live row without a schema
                // remains sendable below, using the snapshot's own value type at the sender.
                let Some(ord) = kinds.get(&(*core, *id)).copied() else {
                    return false;
                };
                let field = *fields
                    .entry((*core, ord, name.as_str()))
                    .or_insert_with(|| schema_field_in_kind(store, *core, ord, name));
                !field.is_some_and(|f| draft_rejected(f, text))
            })
            .collect()
    }

    /// Check refresh eligibility against the sendable drafts and current per-core edit state.
    ///
    /// The same check runs at render and dispatch so a delayed click cannot send an ineligible
    /// refresh. Kind changes use their staged SignalType, matching the feed's serializer path.
    pub(super) fn can_refresh_buys(&self, keys: &[FieldEditKey], store: &CoreStore) -> bool {
        let has_open_edit = keys.iter().any(|(core, _, _)| {
            store
                .core(*core)
                .is_some_and(|cd| !cd.strategy_edits.is_empty())
        });
        buy_refresh_enabled(
            keys.iter().filter_map(|(core, id, name)| {
                let strategy = row(store, *core, *id)?;
                let kind = self
                    .field_edits
                    .get(&(*core, *id, "SignalType".to_string()))
                    .map(String::as_str)
                    .unwrap_or(strategy.kind.as_str());
                Some((kind, name.as_str()))
            }),
            has_open_edit,
        )
    }

    /// Dispatch one exact field-edit plan and retain every draft hidden by the current scope.
    ///
    /// The current visible plan and workspace generation must still equal the producer snapshot;
    /// otherwise every target is refused before the first core command.
    ///
    /// Args:
    ///     plan: Complete field-edit payload captured by the rendered Apply button.
    ///     apply_to_orders: Request a BUY refresh with this edit batch only.
    ///     cx: View context used to reach the session and publish cleared visible editors.
    ///
    /// Returns:
    ///     Nothing; a failed dispatch retains drafts, while vanished targets lose their drafts.
    pub(super) fn apply_field_edits(
        &mut self,
        plan: &FieldEditPlan,
        apply_to_orders: bool,
        cx: &mut Context<Self>,
    ) {
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
        let res = {
            let store = self.backend.read(cx).session.store();
            !strategy_targets_exist(&plan.targets, |(core, id)| row(store, core, id).is_some())
        };
        if res {
            log::warn!("field Apply refused: a target left the live strategy list");
            let backend = self.backend.clone();
            self.reconcile_field_edits(backend.read(cx).session.store());
            cx.notify();
            return;
        }
        // Drafts the core would refuse are dropped HERE rather than in the captured plan: sending
        // one costs the typed text and changes nothing, which is the very "my value did not
        // arrive" this path exists to prevent. Keeping them staged leaves the row red, revertable,
        // and correctable.
        let sendable: Vec<FieldEditKey> = {
            let backend = self.backend.read(cx);
            let store = backend.session.store();
            self.sendable_field_edits(&plan.edit_keys, store)
                .into_iter()
                .cloned()
                .collect()
        };
        // Every draft refused: there is nothing to send, and clearing the editor cache would take
        // the caret out of the one field the user still has to correct.
        if sendable.is_empty() {
            return;
        }
        let actions = group_field_edits(&self.field_edits, &sendable);
        let b = self.backend.read(cx);
        if apply_to_orders && !self.can_refresh_buys(&sendable, b.session.store()) {
            return;
        }
        let mut requested = 0;
        for (core, edits) in &actions {
            if let Err(error) =
                b.session
                    .edit_strategies_with_order_refresh(*core, edits.clone(), apply_to_orders)
            {
                if requested > 0 {
                    log::info!("{}", t!("strat.buy_refresh_requested", n = requested));
                }
                log::warn!("edit strategies failed: {error}");
                return;
            }
            if apply_to_orders {
                requested += edits.len();
            }
        }
        if requested > 0 {
            log::info!("{}", t!("strat.buy_refresh_requested", n = requested));
        }
        // Clearing here is correct, not lossy: the value these keys displayed now arrives from
        // `CoreData::strategy_edit`/`strategy_edit_notes_since` (the pending/adjusted/superseded
        // tiers in `logic::edited_field_value`) instead of vanishing until the next echo. This
        // function deliberately raises no notification and keeps no intent map of its own — GitHub
        // issue #328 proposed re-adding one; that hand-rolled tracking is what this design replaces.
        self.field_edits.retain(|key, _| !sendable.contains(key));
        self.clear_field_editor_cache();
        cx.notify();
    }

    /// Capture or read the singleton workspace generation for delayed action guards.
    ///
    /// Args:
    ///     cx: Application context used to read the shared revision entity.
    ///
    /// Returns:
    ///     Current generation whenever `workspace_cores` scopes this window (Auto, or Classic
    ///     membership hiding at least one core), or `None` when nothing confines it.
    pub(super) fn action_workspace_generation(&self, cx: &App) -> Option<u64> {
        self.workspace_cores.as_ref()?;
        let revision = self.backend.read(cx).workspace_revision();
        Some(revision.read(cx).generation())
    }

    /// Discard workspace-visible field drafts while preserving drafts hidden by the current scope.
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

    /// Put a query that finds a strategy's name into the search filter and focus the input for Find
    /// All by Name. [`StrategyQuery::phrase_for_name`] turns the name into the AND of its words, so
    /// commas and `!` in it are never read as syntax; a name that yields no phrase leaves the search
    /// unchanged. Writes both the filter and input because `set_value` does not emit Change.
    ///
    /// [`StrategyQuery::phrase_for_name`]: moon_core::strategy_query::StrategyQuery::phrase_for_name
    pub(super) fn search_by_name(
        &mut self,
        name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text = moon_core::strategy_query::StrategyQuery::phrase_for_name(&name);
        // A name with no words (only `!`, commas, whitespace) yields no phrase; clearing the
        // search for it would widen the tree instead of finding anything.
        if text.is_empty() {
            return;
        }
        self.filter.search = text.clone();
        // A deliberate search settles at once, like Enter.
        self.search_prune_debounce = None;
        self.search.update(cx, |st, cx| {
            st.set_value(text, window, cx);
            st.focus(window, cx);
        });
        self.on_filter_changed(cx);
    }
}
