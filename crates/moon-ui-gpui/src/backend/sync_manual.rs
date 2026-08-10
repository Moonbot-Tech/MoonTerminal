use std::collections::HashSet;

use moon_core::feed::{NewStrategySpec, StrategyRow};
use moon_core::session::CoreId;

use crate::Backend;
use crate::controls::MANUAL_KIND;

const STRATEGY_NAME_FIELD: &str = "StrategyName";
const MANUAL_HOOK_FIELD_NAMES: &[&str] = &[
    "StrategySettings",
    "Strategy Settings",
    "ManualStrategySettings",
    "Manual Strategy Settings",
];

fn strategy_field<'a>(row: &'a StrategyRow, name: &str) -> Option<&'a str> {
    row.fields
        .iter()
        .find(|(field, _)| field.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.trim())
        .filter(|value| !value.is_empty())
}

fn strategy_spec_from_row(row: &StrategyRow) -> NewStrategySpec {
    let mut fields = row.fields.clone();
    if let Some((_, value)) = fields
        .iter_mut()
        .find(|(field, _)| field == STRATEGY_NAME_FIELD)
    {
        *value = row.name.clone();
    } else {
        fields.push((STRATEGY_NAME_FIELD.to_string(), row.name.clone()));
    }
    NewStrategySpec {
        kind_ordinal: row.kind_ordinal,
        folder_path: row.folder_path.clone(),
        fields,
        insert_after: None,
    }
}

fn is_manual_strategy(row: &StrategyRow) -> bool {
    row.kind_ordinal == MANUAL_KIND
}

fn is_hook_strategy(row: &StrategyRow) -> bool {
    let kind = row.kind.to_ascii_lowercase();
    kind == "moonhook" || kind == "hook" || kind.contains("hook")
}

fn strategy_by_name_kind<'a>(
    rows: &'a [StrategyRow],
    is_kind: impl Fn(&StrategyRow) -> bool,
    name: &str,
) -> Option<&'a StrategyRow> {
    rows.iter().find(|row| is_kind(row) && row.name == name)
}

fn has_strategy_name_kind(
    rows: &[StrategyRow],
    is_kind: impl Fn(&StrategyRow) -> bool,
    name: &str,
) -> bool {
    strategy_by_name_kind(rows, is_kind, name).is_some()
}

fn linked_hook_name(manual: &StrategyRow, rows: &[StrategyRow]) -> Option<String> {
    for field in MANUAL_HOOK_FIELD_NAMES {
        if let Some(value) = strategy_field(manual, field) {
            return Some(value.to_string());
        }
    }
    manual.fields.iter().find_map(|(_, value)| {
        let value = value.trim();
        (!value.is_empty()
            && rows
                .iter()
                .any(|row| is_hook_strategy(row) && row.name == value))
        .then(|| value.to_string())
    })
}

#[derive(Clone, Debug)]
pub(crate) struct SyncManualStrategyRepairTarget {
    pub(crate) core: CoreId,
    pub(crate) core_name: String,
    pub(crate) missing_manuals: Vec<String>,
    pub(crate) missing_hooks: Vec<String>,
    pub(crate) selectable_manual: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct SyncManualStrategyRepairPlan {
    pub(crate) origin: CoreId,
    pub(crate) market: String,
    pub(crate) candidates: Vec<(CoreId, String)>,
    pub(crate) selected_manual: String,
    pub(crate) selected_hook: Option<String>,
    pub(crate) all_manuals: Vec<String>,
    pub(crate) targets: Vec<SyncManualStrategyRepairTarget>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct SyncManualStrategyRepairResult {
    pub(crate) target_cores: usize,
    pub(crate) copied_manuals: usize,
    pub(crate) copied_hooks: usize,
    pub(crate) selected_existing: usize,
}

impl Backend {
    /// Return synchronized manual-order targets for one explicit chart-stack click.
    ///
    /// Synchronization is intentionally narrow: every replicated order must target the exact same
    /// market, use the same non-empty chart bundle as the origin core, and have the same selected
    /// manual strategy name already enabled on the core. The UI never selects a strategy implicitly.
    pub(crate) fn sync_manual_order_targets(
        &self,
        origin: CoreId,
        market: &str,
        candidates: &[(CoreId, String)],
    ) -> Vec<(CoreId, String)> {
        let Some(origin_bundle) = self.core_chart_bundle(origin) else {
            log::warn!(
                "sync manual order blocked: origin core={} has no chart bundle",
                moon_core::feed::core_label(origin)
            );
            return Vec::new();
        };
        let Some(origin_strategy) = self.selected_manual_strategy_name(origin) else {
            log::warn!(
                "sync manual order blocked: origin core={} has no enabled manual strategy",
                moon_core::feed::core_label(origin)
            );
            return Vec::new();
        };
        let mut seen = HashSet::new();
        candidates
            .iter()
            .filter_map(|(core, candidate_market)| {
                if !seen.insert(*core) || candidate_market != market {
                    return None;
                }
                let server = self
                    .config
                    .servers
                    .iter()
                    .find(|server| server.id == *core)?;
                if !server.active
                    || server.chart_bundle.is_empty()
                    || server.chart_bundle != origin_bundle
                {
                    return None;
                }
                let strategy = self.selected_manual_strategy_name(*core)?;
                if strategy != origin_strategy {
                    return None;
                }
                Some((*core, candidate_market.clone()))
            })
            .collect()
    }

    /// Return synchronized market-action targets for the current comparison stack.
    ///
    /// Unlike manual-order placement, market actions do not require a selected manual strategy:
    /// they operate on the market itself. The scope still stays narrow: exact market, active core,
    /// same non-empty chart bundle as the origin, and deduplicated cores.
    pub(crate) fn sync_market_action_targets(
        &self,
        origin: CoreId,
        market: &str,
        candidates: &[(CoreId, String)],
    ) -> Vec<(CoreId, String)> {
        let Some(origin_bundle) = self.core_chart_bundle(origin) else {
            return vec![(origin, market.to_string())];
        };
        let mut seen = HashSet::new();
        let targets: Vec<(CoreId, String)> = candidates
            .iter()
            .filter_map(|(core, candidate_market)| {
                if !seen.insert(*core) || candidate_market != market {
                    return None;
                }
                let server = self
                    .config
                    .servers
                    .iter()
                    .find(|server| server.id == *core)?;
                if !server.active
                    || server.chart_bundle.is_empty()
                    || server.chart_bundle != origin_bundle
                {
                    return None;
                }
                Some((*core, candidate_market.clone()))
            })
            .collect();
        if targets.is_empty() {
            vec![(origin, market.to_string())]
        } else {
            targets
        }
    }

    pub(crate) fn sync_manual_strategy_repair_plan(
        &self,
        origin: CoreId,
        market: &str,
        candidates: &[(CoreId, String)],
    ) -> Option<SyncManualStrategyRepairPlan> {
        let origin_data = self.session.store().core(origin)?;
        let selected_manual = self.selected_manual_strategy_row(origin)?;
        let selected_hook = linked_hook_name(selected_manual, &origin_data.strategies);
        let all_manuals: Vec<String> = origin_data
            .strategies
            .iter()
            .filter(|row| is_manual_strategy(row))
            .map(|row| row.name.clone())
            .collect();
        let targets = self.sync_market_action_targets(origin, market, candidates);
        let mut repair_targets = Vec::new();
        for (target_core, _) in targets {
            if target_core == origin {
                continue;
            }
            let Some(target_data) = self.session.store().core(target_core) else {
                continue;
            };
            let selected_ok = self
                .selected_manual_strategy_name(target_core)
                .is_some_and(|name| name == selected_manual.name);
            let has_manual = has_strategy_name_kind(
                &target_data.strategies,
                is_manual_strategy,
                &selected_manual.name,
            );
            let missing_manuals = (!has_manual).then(|| selected_manual.name.clone());
            let missing_hooks = selected_hook
                .as_ref()
                .filter(|hook| {
                    !has_strategy_name_kind(&target_data.strategies, is_hook_strategy, hook)
                })
                .cloned();
            if !selected_ok || missing_manuals.is_some() || missing_hooks.is_some() {
                repair_targets.push(SyncManualStrategyRepairTarget {
                    core: target_core,
                    core_name: self.core_name(target_core),
                    missing_manuals: missing_manuals.into_iter().collect(),
                    missing_hooks: missing_hooks.into_iter().collect(),
                    selectable_manual: has_manual && !selected_ok,
                });
            }
        }
        (!repair_targets.is_empty()).then(|| SyncManualStrategyRepairPlan {
            origin,
            market: market.to_string(),
            candidates: candidates.to_vec(),
            selected_manual: selected_manual.name.clone(),
            selected_hook,
            all_manuals,
            targets: repair_targets,
        })
    }

    pub(crate) fn apply_sync_manual_strategy_repair(
        &mut self,
        origin: CoreId,
        market: &str,
        candidates: &[(CoreId, String)],
        all_manuals: bool,
    ) -> SyncManualStrategyRepairResult {
        let mut result = SyncManualStrategyRepairResult::default();
        let Some(origin_data) = self.session.store().core(origin) else {
            return result;
        };
        let selected_manual_name = self
            .selected_manual_strategy_name(origin)
            .map(str::to_string)
            .unwrap_or_default();
        let source_manuals: Vec<StrategyRow> = if all_manuals {
            origin_data
                .strategies
                .iter()
                .filter(|row| is_manual_strategy(row))
                .cloned()
                .collect()
        } else {
            self.selected_manual_strategy_row(origin)
                .cloned()
                .into_iter()
                .collect()
        };
        let origin_strategies = origin_data.strategies.clone();
        let targets = self.sync_market_action_targets(origin, market, candidates);
        for (target_core, _) in targets {
            if target_core == origin {
                continue;
            }
            let Some(target_data) = self.session.store().core(target_core) else {
                continue;
            };
            result.target_cores += 1;
            let target_strategies = target_data.strategies.clone();
            let selected_missing = !selected_manual_name.is_empty()
                && !has_strategy_name_kind(
                    &target_strategies,
                    is_manual_strategy,
                    &selected_manual_name,
                );
            let mut queued_hooks = HashSet::new();
            let mut queued_manuals = HashSet::new();
            let mut specs = Vec::new();
            let mut copied_hooks_here = 0usize;
            let mut copied_manuals_here = 0usize;
            for manual in &source_manuals {
                if let Some(hook_name) = linked_hook_name(manual, &origin_strategies) {
                    if !has_strategy_name_kind(&target_strategies, is_hook_strategy, &hook_name)
                        && queued_hooks.insert(hook_name.clone())
                    {
                        if let Some(hook) =
                            strategy_by_name_kind(&origin_strategies, is_hook_strategy, &hook_name)
                        {
                            specs.push(strategy_spec_from_row(hook));
                            copied_hooks_here += 1;
                        } else {
                            log::warn!(
                                "sync manual strategy repair: hook {hook_name} referenced by {} not found on origin={}",
                                manual.name,
                                moon_core::feed::core_label(origin)
                            );
                        }
                    }
                }
                if !has_strategy_name_kind(&target_strategies, is_manual_strategy, &manual.name)
                    && queued_manuals.insert(manual.name.clone())
                {
                    specs.push(strategy_spec_from_row(manual));
                    copied_manuals_here += 1;
                }
            }
            if !specs.is_empty() {
                match self.session.create_strategies(target_core, specs) {
                    Ok(()) => {
                        result.copied_hooks += copied_hooks_here;
                        result.copied_manuals += copied_manuals_here;
                        log::info!(
                            "sync manual strategy repair: copied hooks={} manuals={} to core={}",
                            copied_hooks_here,
                            copied_manuals_here,
                            moon_core::feed::core_label(target_core)
                        );
                        if selected_missing && queued_manuals.contains(&selected_manual_name) {
                            self.queue_pending_manual_strategy_select(
                                target_core,
                                selected_manual_name.clone(),
                            );
                        }
                    }
                    Err(error) => log::warn!(
                        "sync manual strategy repair failed: core={}: {error:#}",
                        moon_core::feed::core_label(target_core)
                    ),
                }
            }
            if let Some(existing_manual) = target_strategies
                .iter()
                .find(|row| is_manual_strategy(row) && row.name == selected_manual_name)
            {
                self.set_manual_strategy(target_core, true, existing_manual.id);
                result.selected_existing += 1;
            }
        }
        result
    }

    fn core_chart_bundle(&self, core: CoreId) -> Option<&str> {
        self.config
            .servers
            .iter()
            .find(|server| server.id == core)
            .and_then(|server| {
                (!server.chart_bundle.is_empty()).then_some(server.chart_bundle.as_str())
            })
    }

    fn selected_manual_strategy_name(&self, core: CoreId) -> Option<&str> {
        let (on, id) = self.manual_strat_state(core);
        if !on || id == 0 {
            return None;
        }
        self.session
            .store()
            .core(core)?
            .strategies
            .iter()
            .find(|strategy| strategy.id == id)
            .map(|strategy| strategy.name.as_str())
    }

    fn selected_manual_strategy_row(&self, core: CoreId) -> Option<&StrategyRow> {
        let (on, id) = self.manual_strat_state(core);
        if !on || id == 0 {
            return None;
        }
        self.session
            .store()
            .core(core)?
            .strategies
            .iter()
            .find(|strategy| strategy.id == id && is_manual_strategy(strategy))
    }

    fn core_name(&self, core: CoreId) -> String {
        self.session
            .sessions()
            .iter()
            .find(|session| session.id == core)
            .map(|session| session.name.clone())
            .unwrap_or_else(|| moon_core::feed::core_label(core).to_string())
    }

    /// Store and send a manual-strategy selection, then mirror it inside the active chart SYNC
    /// context by exact Manual `StrategyName`.
    pub(crate) fn set_manual_strategy_with_sync(&mut self, core: CoreId, on: bool, id: u64) {
        let strategy_name = self
            .manual_strategy_name_by_id(core, id)
            .map(str::to_string);
        self.set_manual_strategy(core, on, id);
        let Some(strategy_name) = strategy_name else {
            return;
        };
        self.sync_manual_strategy_selection(core, on, &strategy_name);
    }

    /// Replace the runtime chart SYNC scope used by the header/manual-strategy picker.
    pub(crate) fn set_sync_manual_strategy_targets(&mut self, targets: Vec<(CoreId, String)>) {
        self.sync_manual_strategy_targets = targets;
    }

    /// Clear the runtime chart SYNC scope only if it still belongs to the stack that published
    /// `targets`. This prevents unrelated chart stacks from erasing an active SYNC context.
    pub(crate) fn clear_sync_manual_strategy_targets_if(&mut self, targets: &[(CoreId, String)]) {
        if self.sync_manual_strategy_targets == targets {
            self.sync_manual_strategy_targets.clear();
        }
    }

    /// Apply delayed Manual selections for strategies that were just copied and are waiting for a
    /// core snapshot with the new core-owned ID.
    pub(crate) fn tick_pending_manual_strategy_select(&mut self) {
        if self.sync_manual_select_pending.is_empty() {
            return;
        }
        let pending = std::mem::take(&mut self.sync_manual_select_pending);
        for (core, name) in pending {
            let strategy_id = self.session.store().core(core).and_then(|data| {
                strategy_by_name_kind(&data.strategies, is_manual_strategy, &name)
                    .map(|strategy| strategy.id)
            });
            match strategy_id {
                Some(id) => {
                    self.set_manual_strategy(core, true, id);
                    log::info!(
                        "sync manual strategy repair: selected copied Manual {name} on core={}",
                        moon_core::feed::core_label(core)
                    );
                }
                None => self.queue_pending_manual_strategy_select(core, name),
            }
        }
    }

    fn sync_manual_strategy_selection(&mut self, origin: CoreId, on: bool, strategy_name: &str) {
        let Some(origin_bundle) = self.core_chart_bundle(origin) else {
            return;
        };
        let origin_bundle = origin_bundle.to_string();
        let targets = self.sync_manual_strategy_targets.clone();
        let mut seen = HashSet::new();
        for (target_core, _) in targets {
            if target_core == origin || !seen.insert(target_core) {
                continue;
            }
            let Some(server) = self
                .config
                .servers
                .iter()
                .find(|server| server.id == target_core)
            else {
                continue;
            };
            if !server.active
                || server.chart_bundle.is_empty()
                || server.chart_bundle != origin_bundle
            {
                continue;
            }
            let Some(target_data) = self.session.store().core(target_core) else {
                continue;
            };
            if on {
                match strategy_by_name_kind(&target_data.strategies, is_manual_strategy, strategy_name)
                    .map(|target_strategy| target_strategy.id)
                {
                    Some(target_strategy_id) => {
                        self.set_manual_strategy(target_core, true, target_strategy_id);
                    }
                    None => {
                        log::warn!(
                            "sync manual strategy select skipped: core={} has no Manual {strategy_name}",
                            moon_core::feed::core_label(target_core)
                        );
                    }
                }
            } else {
                let (target_on, target_id) = self.manual_strat_state(target_core);
                if !target_on || target_id == 0 {
                    continue;
                }
                if self.selected_manual_strategy_name(target_core) == Some(strategy_name) {
                    self.set_manual_strategy(target_core, false, target_id);
                }
            }
        }
    }

    fn manual_strategy_name_by_id(&self, core: CoreId, id: u64) -> Option<&str> {
        if id == 0 {
            return None;
        }
        self.session
            .store()
            .core(core)?
            .strategies
            .iter()
            .find(|strategy| strategy.id == id && is_manual_strategy(strategy))
            .map(|strategy| strategy.name.as_str())
    }

    fn queue_pending_manual_strategy_select(&mut self, core: CoreId, name: String) {
        if self
            .sync_manual_select_pending
            .iter()
            .any(|(pending_core, pending_name)| *pending_core == core && pending_name == &name)
        {
            return;
        }
        self.sync_manual_select_pending.push((core, name));
    }
}
