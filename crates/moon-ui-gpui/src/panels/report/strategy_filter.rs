//! Grouped, searchable strategy choices for the Report filter.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use gpui::{AnyElement, App, IntoElement, SharedString, Task, Window};
use moon_core::config::AppConfig;
use moon_core::db::{ReportStrategy, ReportStrategyKey};
use moon_ui::{
    MoonComponentIndexPath, MoonSearchableGroup, MoonSearchableListDelegate, MoonSearchableListItem,
};

use crate::core_order::{CoreOrder, OrderedCores};

/// Shared retained search text used when Report replaces strategy metadata.
pub(super) type ReportStrategySearch = Rc<RefCell<String>>;

/// One semantic choice exposed by the Report strategy filter.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ReportStrategyChoice {
    /// Every strategy belonging to one core.
    Core(u64),
    /// One exact strategy identity on one core.
    Exact(ReportStrategyKey),
}

impl ReportStrategyChoice {
    /// Return the core constrained by this choice.
    ///
    /// Returns:
    ///     The selected core UID.
    fn core_uid(self) -> u64 {
        match self {
            Self::Core(core_uid) => core_uid,
            Self::Exact(strategy) => strategy.core_uid,
        }
    }
}

/// One strategy row with separate section-row and trigger labels.
#[derive(Clone)]
pub(super) struct ReportStrategyItem {
    choice: ReportStrategyChoice,
    row_label: SharedString,
    trigger_label: SharedString,
    search_text: String,
}

impl MoonSearchableListItem for ReportStrategyItem {
    type Value = ReportStrategyChoice;

    /// Return the context-rich label shown in the closed trigger.
    ///
    /// Returns:
    ///     The row label combined with its core name.
    fn title(&self) -> SharedString {
        self.trigger_label.clone()
    }

    /// Render only the row label because the section already identifies the core.
    ///
    /// Args:
    ///     _: Window and application contexts unused by the plain-text row.
    ///
    /// Returns:
    ///     The row label as a GPUI element.
    fn render(&self, _: &mut Window, _: &mut App) -> impl IntoElement {
        self.row_label.clone()
    }

    /// Return the semantic filter choice represented by this row.
    ///
    /// Returns:
    ///     The core-wide or exact strategy identity.
    fn value(&self) -> &Self::Value {
        &self.choice
    }

    /// Match both the strategy label and its core so a core search retains the whole section.
    ///
    /// Args:
    ///     query: Case-insensitive strategy or core substring.
    ///
    /// Returns:
    ///     Whether the cached lowercase search text contains the query.
    fn matches(&self, query: &str) -> bool {
        self.matches_normalized(&query.to_lowercase())
    }
}

impl ReportStrategyItem {
    /// Match a query normalized once for the complete filtering pass.
    ///
    /// Args:
    ///     normalized_query: Lowercase strategy or core substring.
    ///
    /// Returns:
    ///     Whether the cached lowercase search text contains the query.
    fn matches_normalized(&self, normalized_query: &str) -> bool {
        self.search_text.contains(normalized_query)
    }
}

/// Grouped delegate that reapplies retained search after metadata replacement.
pub(super) struct ReportStrategyDelegate {
    groups: Vec<MoonSearchableGroup<ReportStrategyItem>>,
    matched_groups: Vec<MoonSearchableGroup<ReportStrategyItem>>,
    search: ReportStrategySearch,
}

impl ReportStrategyDelegate {
    /// Create retained search state for one Report panel.
    ///
    /// Returns:
    ///     An initially empty query shared by replacement delegates.
    pub(super) fn search_state() -> ReportStrategySearch {
        Rc::new(RefCell::new(String::new()))
    }

    /// Build a delegate whose initial rows agree with the retained query.
    ///
    /// Args:
    ///     groups: Complete grouped strategy hierarchy.
    ///     search: Retained query shared with future replacements.
    ///
    /// Returns:
    ///     A filtered delegate ready for the first replacement render.
    pub(super) fn new(
        groups: Vec<MoonSearchableGroup<ReportStrategyItem>>,
        search: ReportStrategySearch,
    ) -> Self {
        Self::with_initial_query(groups, search, true)
    }

    /// Build an unfiltered delegate for full-list selection synchronization.
    ///
    /// Args:
    ///     groups: Complete grouped strategy hierarchy.
    ///     search: Retained query shared with the filtered replacement.
    ///
    /// Returns:
    ///     A delegate that exposes every row without clearing the retained query.
    pub(super) fn unfiltered(
        groups: Vec<MoonSearchableGroup<ReportStrategyItem>>,
        search: ReportStrategySearch,
    ) -> Self {
        Self::with_initial_query(groups, search, false)
    }

    /// Build a delegate with filtered or complete initial visibility.
    ///
    /// Args:
    ///     groups: Complete grouped strategy hierarchy.
    ///     search: Retained query shared with replacement delegates.
    ///     apply_query: Whether to filter the first visible snapshot.
    ///
    /// Returns:
    ///     A configured grouped delegate.
    fn with_initial_query(
        groups: Vec<MoonSearchableGroup<ReportStrategyItem>>,
        search: ReportStrategySearch,
        apply_query: bool,
    ) -> Self {
        let mut delegate = Self {
            matched_groups: groups.clone(),
            groups,
            search,
        };
        if apply_query {
            let query = delegate.search.borrow().clone();
            delegate.apply_search(&query);
        }
        delegate
    }

    /// Rebuild visible groups from the complete hierarchy.
    ///
    /// Args:
    ///     query: Case-insensitive strategy or core substring.
    ///
    /// Returns:
    ///     Nothing; empty sections disappear from the visible result.
    fn apply_search(&mut self, query: &str) {
        if query.is_empty() {
            self.matched_groups.clone_from(&self.groups);
            return;
        }
        let normalized_query = query.to_lowercase();
        self.matched_groups = self
            .groups
            .iter()
            .filter_map(|group| {
                let mut matched = group.clone();
                matched
                    .items
                    .retain(|item| item.matches_normalized(&normalized_query));
                (!matched.items.is_empty()).then_some(matched)
            })
            .collect();
    }
}

impl MoonSearchableListDelegate for ReportStrategyDelegate {
    type Item = ReportStrategyItem;

    /// Return the number of visible core sections.
    ///
    /// Args:
    ///     _: Application context unused by retained in-memory groups.
    ///
    /// Returns:
    ///     The filtered section count.
    fn sections_count(&self, _: &App) -> usize {
        self.matched_groups.len()
    }

    /// Return the number of visible rows in one section.
    ///
    /// Args:
    ///     section: Visible section index.
    ///
    /// Returns:
    ///     Its row count, or zero for an invalid section.
    fn items_count(&self, section: usize) -> usize {
        self.matched_groups
            .get(section)
            .map_or(0, |group| group.items.len())
    }

    /// Render one visible core title.
    ///
    /// Args:
    ///     section: Visible section index.
    ///
    /// Returns:
    ///     The section title as an element when the index is valid.
    #[allow(deprecated)]
    fn section(&self, section: usize) -> Option<AnyElement> {
        self.matched_groups
            .get(section)
            .map(|group| group.title.clone().into_any_element())
    }

    /// Return one visible strategy row.
    ///
    /// Args:
    ///     index: Visible section and row.
    ///
    /// Returns:
    ///     The retained item when the index is valid.
    fn item(&self, index: MoonComponentIndexPath) -> Option<&Self::Item> {
        self.matched_groups.get(index.section)?.items.get(index.row)
    }

    /// Find one semantic choice inside the current filtered result.
    ///
    /// Args:
    ///     value: Choice identity to locate.
    ///
    /// Returns:
    ///     Its visible index when present.
    fn position<V>(&self, value: &V) -> Option<MoonComponentIndexPath>
    where
        Self::Item: MoonSearchableListItem<Value = V>,
        V: PartialEq,
    {
        self.matched_groups
            .iter()
            .enumerate()
            .find_map(|(section, group)| {
                group
                    .items
                    .iter()
                    .position(|item| item.value() == value)
                    .map(|row| MoonComponentIndexPath::new(row).section(section))
            })
    }

    /// Refilter rows and retain the query for the next metadata replacement.
    ///
    /// Args:
    ///     query: Current strategy/core query.
    ///     _: Window and application contexts unused by synchronous filtering.
    ///
    /// Returns:
    ///     An already-complete task after visible rows are rebuilt.
    fn perform_search(&mut self, query: &str, _: &mut Window, _: &mut App) -> Task<()> {
        self.search.replace(query.to_string());
        self.apply_search(query);
        Task::ready(())
    }
}

/// Merge Report metadata into the application's canonical core order.
///
/// Args:
///     strategies: Exact strategy identities and names.
///     cores: Database core identities and display names.
///     config: Current configuration supplying canonical core order.
///
/// Returns:
///     Every metadata core in canonical order, including scoped cores not yet in the database
///     core snapshot.
pub(super) fn ordered_strategy_cores(
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

/// Build Report strategy sections in one metadata pass.
///
/// Args:
///     strategies: Exact strategy identities and names.
///     ordered_cores: Database cores already arranged by the canonical ordering policy.
///     all_label: Localized label for a core-wide choice.
///     manual_label: Localized label for manual orders.
///
/// Returns:
///     One group per known strategy/core, each starting with its core-wide choice.
pub(super) fn strategy_groups(
    strategies: &[ReportStrategy],
    ordered_cores: &[(u64, String)],
    all_label: &str,
    manual_label: &str,
) -> Vec<MoonSearchableGroup<ReportStrategyItem>> {
    let mut strategies_by_core: HashMap<u64, Vec<&ReportStrategy>> = HashMap::new();
    for strategy in strategies {
        strategies_by_core
            .entry(strategy.key.core_uid)
            .or_default()
            .push(strategy);
    }
    ordered_cores
        .iter()
        .map(|(core_uid, core_name)| {
            let mut items = Vec::new();
            items.push(strategy_item(
                ReportStrategyChoice::Core(*core_uid),
                all_label,
                core_name,
            ));
            if let Some(strategies) = strategies_by_core.get(core_uid) {
                items.extend(strategies.iter().map(|strategy| {
                    let label = if strategy.key.strategy_id == 0 {
                        manual_label
                    } else {
                        strategy.name.as_str()
                    };
                    strategy_item(ReportStrategyChoice::Exact(strategy.key), label, core_name)
                }));
            }
            MoonSearchableGroup::new(core_name.clone()).items(items)
        })
        .collect()
}

/// Build one searchable strategy row.
///
/// Args:
///     choice: Semantic filter choice.
///     row_label: Label shown inside the section.
///     core_name: Section/core label included in search and the closed trigger.
///
/// Returns:
///     A row whose search haystack retains its complete core section.
fn strategy_item(
    choice: ReportStrategyChoice,
    row_label: &str,
    core_name: &str,
) -> ReportStrategyItem {
    ReportStrategyItem {
        choice,
        row_label: row_label.to_string().into(),
        trigger_label: format!("{row_label} - {core_name}").into(),
        search_text: format!("{core_name} {row_label}").to_lowercase(),
    }
}

/// Resolve the semantic choice represented by current Report filters.
///
/// Args:
///     strategy: Optional exact strategy filter.
///     selected_cores: Explicit core filter; empty means every core.
///
/// Returns:
///     Exact strategy first, otherwise one core-wide choice when exactly one core is selected.
pub(super) fn selected_strategy_choice(
    strategy: Option<ReportStrategyKey>,
    selected_cores: &HashSet<u64>,
) -> Option<ReportStrategyChoice> {
    strategy.map(ReportStrategyChoice::Exact).or_else(|| {
        (selected_cores.len() == 1)
            .then(|| selected_cores.iter().next().copied())
            .flatten()
            .map(ReportStrategyChoice::Core)
    })
}

/// Find a choice in grouped widget order.
///
/// Args:
///     groups: Current ordered strategy sections.
///     selected: Semantic choice to locate.
///
/// Returns:
///     Section/row index for MoonUI, or `None` when metadata no longer contains the choice.
pub(super) fn strategy_choice_index(
    groups: &[MoonSearchableGroup<ReportStrategyItem>],
    selected: Option<ReportStrategyChoice>,
) -> Option<MoonComponentIndexPath> {
    let selected = selected?;
    groups.iter().enumerate().find_map(|(section, group)| {
        group
            .items
            .iter()
            .position(|item| item.choice == selected)
            .map(|row| MoonComponentIndexPath::new(row).section(section))
    })
}

/// Convert one widget choice into Report core and exact-strategy filters.
///
/// Args:
///     choice: Selected row, or `None` for the global unfiltered state.
///
/// Returns:
///     Explicit core set and optional exact strategy.
pub(super) fn filters_for_strategy_choice(
    choice: Option<ReportStrategyChoice>,
) -> (HashSet<u64>, Option<ReportStrategyKey>) {
    match choice {
        Some(choice @ ReportStrategyChoice::Core(_)) => (HashSet::from([choice.core_uid()]), None),
        Some(choice @ ReportStrategyChoice::Exact(strategy)) => {
            (HashSet::from([choice.core_uid()]), Some(strategy))
        }
        None => (HashSet::new(), None),
    }
}

#[cfg(test)]
mod tests;
