//! Pure strategy projection and exact-selection state for the Report filter.

use std::collections::{HashMap, HashSet};

use moon_core::config::AppConfig;
use moon_core::db::{ReportStrategy, ReportStrategyKey};
use moon_ui::MoonComponentIndexPath;

use super::{ReportStrategyChoice, ReportStrategyGroup, ReportStrategyItem};
use crate::core_order::{CoreOrder, OrderedCores};

/// Merge Report metadata into the application's canonical core order.
///
/// Args:
///     strategies: Exact strategy identities and names, including retained stale choices.
///     cores: Database core identities and display names.
///     config: Current configuration supplying canonical core order.
///
/// Returns:
///     Every metadata core in canonical order, including scoped cores not yet in the database
///     core snapshot.
pub(in crate::panels::report) fn ordered_strategy_cores(
    strategies: &[ReportStrategy],
    cores: &[(u64, String)],
    config: &AppConfig,
) -> OrderedCores {
    let mut core_names = cores.to_vec();
    let mut known: HashSet<u64> = core_names.iter().map(|(core_uid, _)| *core_uid).collect();
    for strategy in strategies {
        if known.insert(strategy.key.core_uid) {
            core_names.push((strategy.key.core_uid, strategy.key.core_uid.to_string()));
        }
    }
    CoreOrder::new(config).from_db(core_names)
}

/// Build the global All row plus one strategy section per core that has strategies.
///
/// The drop criterion is membership in this metadata snapshot: a core contributing no row at all
/// has nothing to toggle — its core-wide row would render permanently disabled — so its whole
/// section goes rather than listing a core name over an empty group. A core present ONLY through a
/// stale choice retained by [`merge_strategy_metadata`] deliberately keeps its section, disabled
/// core-wide row and all: that is what makes the stale choice removable.
///
/// Args:
///     strategies: Exact strategy identities and display names.
///     ordered_cores: Database cores arranged by the canonical ordering policy.
///     all_label: Localized global All-strategies label.
///     manual_label: Localized label for manual orders.
///
/// Returns:
///     The global All row, then one section per non-empty core: its name as the group-toggle row,
///     followed by that core's exact strategies.
pub(in crate::panels::report) fn strategy_groups(
    strategies: &[ReportStrategy],
    ordered_cores: &[(u64, String)],
    all_label: &str,
    manual_label: &str,
) -> Vec<ReportStrategyGroup> {
    let mut strategies_by_core: HashMap<u64, Vec<&ReportStrategy>> = HashMap::new();
    for strategy in strategies {
        strategies_by_core
            .entry(strategy.key.core_uid)
            .or_default()
            .push(strategy);
    }
    let mut groups = vec![ReportStrategyGroup {
        title: None,
        items: vec![strategy_item(
            ReportStrategyChoice::All,
            all_label,
            all_label,
        )],
    }];
    groups.extend(ordered_cores.iter().filter_map(|(core_uid, core_name)| {
        let core_strategies = strategies_by_core.get(core_uid)?;
        // The core NAME is the group row, exactly as the core selector's exchange row is: clicking
        // it toggles the whole group. A separate non-interactive title above an "All strategies"
        // row would say the same thing twice, and only one of the two would be clickable.
        let mut items = vec![strategy_item(
            ReportStrategyChoice::Core(*core_uid),
            core_name,
            core_name,
        )];
        items.extend(core_strategies.iter().map(|strategy| {
            let label = if strategy.key.strategy_id == 0 {
                manual_label
            } else {
                strategy.name.as_str()
            };
            strategy_item(ReportStrategyChoice::Exact(strategy.key), label, core_name)
        }));
        Some(ReportStrategyGroup { title: None, items })
    }));
    groups
}

/// Build one searchable strategy row.
///
/// Args:
///     choice: Semantic global, core-wide, or exact choice.
///     row_label: Label shown inside the section.
///     search_context: Core/global text included in the search haystack.
///
/// Returns:
///     A row whose search matches both its label and core context.
fn strategy_item(
    choice: ReportStrategyChoice,
    row_label: &str,
    search_context: &str,
) -> ReportStrategyItem {
    ReportStrategyItem {
        choice,
        row_label: row_label.to_string().into(),
        search_text: format!("{search_context} {row_label}").to_lowercase(),
    }
}

/// Find every selected exact key in full grouped widget order.
///
/// Args:
///     groups: Full grouped strategy hierarchy.
///     selected: Canonical exact set, or `None` for implicit All.
///
/// Returns:
///     Full-list indices for selected exact rows; action rows are never stored as selections.
pub(in crate::panels::report) fn strategy_choice_indices(
    groups: &[ReportStrategyGroup],
    selected: Option<&HashSet<ReportStrategyKey>>,
) -> Vec<MoonComponentIndexPath> {
    let Some(selected) = selected else {
        return Vec::new();
    };
    groups
        .iter()
        .enumerate()
        .flat_map(|(section, group)| {
            group
                .items
                .iter()
                .enumerate()
                .filter_map(move |(row, item)| match item.choice {
                    ReportStrategyChoice::Exact(key) if selected.contains(&key) => {
                        Some(MoonComponentIndexPath::new(row).section(section))
                    }
                    _ => None,
                })
        })
        .collect()
}

/// Convert widget event values into the canonical exact selection.
///
/// Args:
///     choices: Values emitted after a user toggle.
///
/// Returns:
///     `None` when no exact checkbox remains, matching the shared core selector's implicit All
///     convention; otherwise every exact key.
pub(in crate::panels::report) fn exact_strategy_selection(
    choices: &[ReportStrategyChoice],
) -> Option<HashSet<ReportStrategyKey>> {
    let selected = choices
        .iter()
        .filter_map(|choice| match choice {
            ReportStrategyChoice::Exact(key) => Some(*key),
            ReportStrategyChoice::All | ReportStrategyChoice::Core(_) => None,
        })
        .collect::<HashSet<_>>();
    (!selected.is_empty()).then_some(selected)
}

/// Whether canonical strategy state represents implicit or exact complete All.
///
/// Args:
///     available: Exact keys confirmed by current metadata.
///     selected: Canonical exact selection; `None` is implicit All.
///
/// Returns:
///     True for implicit All or exact equality with a non-empty available set. Extra stale keys
///     intentionally keep the result partial.
pub(in crate::panels::report) fn strategy_selection_is_all(
    available: &HashSet<ReportStrategyKey>,
    selected: Option<&HashSet<ReportStrategyKey>>,
) -> bool {
    match selected {
        None => true,
        Some(selected) => !available.is_empty() && selected == available,
    }
}

/// Resolve the compact trigger summary and global All-row state.
///
/// Args:
///     available: Exact keys confirmed by current metadata.
///     selected: Canonical exact selection.
///     all_label: Localized All-strategies label.
///     strategies_n: Localized exact-count formatter.
///
/// Returns:
///     Compact trigger text and whether the selection reads as All.
pub(in crate::panels::report) fn strategy_selection_summary(
    available: &HashSet<ReportStrategyKey>,
    selected: Option<&HashSet<ReportStrategyKey>>,
    all_label: &str,
    strategies_n: impl Fn(usize) -> String,
) -> (String, bool) {
    let all_on = strategy_selection_is_all(available, selected);
    let label = if all_on {
        all_label.to_string()
    } else {
        strategies_n(selected.map_or(0, HashSet::len))
    };
    (label, all_on)
}

/// Normalize canonical strategy state for rows, totals, and export.
///
/// Args:
///     selected: Canonical exact selection; only `None` is unconstrained.
///
/// Returns:
///     `None` for implicit All; otherwise a deterministic exact-key vector, including a complete
///     explicit set. Keeping complete sets exact prevents new or legacy rows from entering a
///     report without a corresponding selected checkbox.
pub(in crate::panels::report) fn normalized_strategy_filter_keys(
    selected: Option<&HashSet<ReportStrategyKey>>,
) -> Option<Vec<ReportStrategyKey>> {
    let mut keys = selected?.iter().copied().collect::<Vec<_>>();
    keys.sort_unstable_by_key(|key| (key.core_uid, key.strategy_id));
    Some(keys)
}

/// Merge refreshed metadata with every selected choice missing from that refresh.
///
/// Args:
///     previous: Existing display metadata used to retain known stale labels.
///     refreshed: Newly available strategies from the database snapshot.
///     selected: Canonical exact selection.
///
/// Returns:
///     Display metadata containing removable stale choices, plus only the genuinely refreshed
///     available-key set used by complete-summary presentation and core-wide toggles.
pub(in crate::panels::report) fn merge_strategy_metadata(
    previous: &[ReportStrategy],
    mut refreshed: Vec<ReportStrategy>,
    selected: Option<&HashSet<ReportStrategyKey>>,
) -> (Vec<ReportStrategy>, HashSet<ReportStrategyKey>) {
    let available = refreshed
        .iter()
        .map(|strategy| strategy.key)
        .collect::<HashSet<_>>();
    let previous_by_key = previous
        .iter()
        .map(|strategy| (strategy.key, strategy))
        .collect::<HashMap<_, _>>();
    let mut missing = selected
        .into_iter()
        .flatten()
        .filter(|key| !available.contains(key))
        .copied()
        .collect::<Vec<_>>();
    missing.sort_unstable_by_key(|key| (key.core_uid, key.strategy_id));
    for key in missing {
        refreshed.push(
            previous_by_key
                .get(&key)
                .map(|strategy| (*strategy).clone())
                .unwrap_or_else(|| ReportStrategy {
                    key,
                    name: key.strategy_id.to_string(),
                }),
        );
    }
    (refreshed, available)
}
