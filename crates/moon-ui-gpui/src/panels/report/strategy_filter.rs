//! Grouped, searchable multi-strategy choices for the Report filter.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use gpui::{AnyElement, App, IntoElement, SharedString, Task, Window};
use moon_core::config::AppConfig;
use moon_core::db::{ReportStrategy, ReportStrategyKey};
use moon_ui::{
    MoonComponentIndexPath, MoonSearchableListChange, MoonSearchableListDelegate,
    MoonSearchableListItem,
};

use crate::core_order::{CoreOrder, OrderedCores};

/// Shared retained search text used when Report replaces strategy metadata.
pub(super) type ReportStrategySearch = Rc<RefCell<String>>;

/// One semantic row exposed by the Report strategy filter.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ReportStrategyChoice {
    /// Restore the global implicit All state.
    All,
    /// Toggle every currently available strategy belonging to one core.
    Core(u64),
    /// Toggle one exact strategy identity on one core.
    Exact(ReportStrategyKey),
}

/// One grouped strategy row with a domain-aware search haystack.
#[derive(Clone)]
pub(super) struct ReportStrategyItem {
    choice: ReportStrategyChoice,
    row_label: SharedString,
    search_text: String,
}

impl MoonSearchableListItem for ReportStrategyItem {
    type Value = ReportStrategyChoice;

    /// Return the row label used by the default list renderer.
    ///
    /// Returns:
    ///     The label without the surrounding core section title.
    fn title(&self) -> SharedString {
        self.row_label.clone()
    }

    /// Render the row label because the section already identifies its core.
    ///
    /// Args:
    ///     _: Window and application contexts unused by the plain-text row.
    ///
    /// Returns:
    ///     The row label as a GPUI element.
    fn render(&self, _: &mut Window, _: &mut App) -> impl IntoElement {
        self.row_label.clone()
    }

    /// Return the semantic strategy choice represented by this row.
    ///
    /// Returns:
    ///     The stable global, core-wide, or exact choice identity.
    fn value(&self) -> &Self::Value {
        &self.choice
    }

    /// Match both strategy and core names so a core query retains its complete section.
    ///
    /// Args:
    ///     query: Case-insensitive strategy or core substring.
    ///
    /// Returns:
    ///     Whether the pre-normalized search haystack contains the query.
    fn matches(&self, query: &str) -> bool {
        self.matches_normalized(&query.to_lowercase())
    }
}

impl ReportStrategyItem {
    /// Match a query that the caller already normalized once for the whole search pass.
    ///
    /// Args:
    ///     normalized_query: Lowercase strategy or core substring.
    ///
    /// Returns:
    ///     Whether the cached lowercase haystack contains the query.
    fn matches_normalized(&self, normalized_query: &str) -> bool {
        self.search_text.contains(normalized_query)
    }
}

/// One optional-title group retained by the Report-specific searchable delegate.
#[derive(Clone)]
pub(super) struct ReportStrategyGroup {
    title: Option<SharedString>,
    items: Vec<ReportStrategyItem>,
}

/// One filtered section mapped back to immutable catalog rows.
struct ReportStrategyMatchGroup {
    source_section: usize,
    /// `None` means every source row is visible without allocating an identity map.
    source_rows: Option<Vec<usize>>,
}

/// Immutable grouped rows and availability indices shared by delegate replacements.
pub(super) struct ReportStrategyCatalog {
    groups: Vec<ReportStrategyGroup>,
    available: HashSet<ReportStrategyKey>,
    available_by_core: HashMap<u64, Vec<MoonComponentIndexPath>>,
}

impl ReportStrategyCatalog {
    /// Find every selected exact key in full grouped widget order.
    ///
    /// Args:
    ///     selected: Canonical exact set, or `None` for implicit All.
    ///
    /// Returns:
    ///     Full-list indices for selected exact rows; action rows are never stored as selections.
    pub(super) fn selected_indices(
        &self,
        selected: Option<&HashSet<ReportStrategyKey>>,
    ) -> Vec<MoonComponentIndexPath> {
        strategy_choice_indices(&self.groups, selected)
    }
}

/// Grouped delegate that keeps canonical selections while its visible search result changes.
pub(super) struct ReportStrategyDelegate {
    catalog: Rc<ReportStrategyCatalog>,
    matched_groups: Vec<ReportStrategyMatchGroup>,
    selected: HashSet<ReportStrategyKey>,
    selected_available: usize,
    selected_available_by_core: HashMap<u64, usize>,
    search: ReportStrategySearch,
}

impl ReportStrategyDelegate {
    /// Create the shared retained search state for one Report panel.
    ///
    /// Returns:
    ///     An initially empty search string shared by every replacement delegate.
    pub(super) fn search_state() -> ReportStrategySearch {
        Rc::new(RefCell::new(String::new()))
    }

    /// Build immutable row and availability indices once for filtered/unfiltered delegates.
    ///
    /// Args:
    ///     groups: Full global/core/exact row hierarchy.
    ///     available: Exact keys confirmed by the latest metadata refresh.
    ///
    /// Returns:
    ///     Shared catalog that avoids repeated full-list scans and deep clones during sync.
    pub(super) fn catalog(
        groups: Vec<ReportStrategyGroup>,
        available: HashSet<ReportStrategyKey>,
    ) -> Rc<ReportStrategyCatalog> {
        let mut available_by_core: HashMap<u64, Vec<MoonComponentIndexPath>> = HashMap::new();
        for (section, group) in groups.iter().enumerate() {
            for (row, item) in group.items.iter().enumerate() {
                if let ReportStrategyChoice::Exact(key) = item.choice
                    && available.contains(&key)
                {
                    available_by_core
                        .entry(key.core_uid)
                        .or_default()
                        .push(MoonComponentIndexPath::new(row).section(section));
                }
            }
        }
        Rc::new(ReportStrategyCatalog {
            groups,
            available,
            available_by_core,
        })
    }

    /// Build a delegate and apply the retained query to its initial visible groups.
    ///
    /// Args:
    ///     catalog: Shared full row hierarchy and availability indices.
    ///     selected: Canonical exact selection, or `None` for implicit All.
    ///     search: Retained query shared with future delegate replacements.
    ///
    /// Returns:
    ///     A grouped delegate whose first render agrees with the visible search input.
    pub(super) fn new(
        catalog: Rc<ReportStrategyCatalog>,
        selected: Option<&HashSet<ReportStrategyKey>>,
        search: ReportStrategySearch,
    ) -> Self {
        Self::with_initial_query(catalog, selected, search, true)
    }

    /// Build a temporarily unfiltered delegate for value-to-index selection synchronization.
    ///
    /// The panel installs this delegate, synchronizes every canonical exact key, and then replaces
    /// it with [`Self::new`]. This lets off-search selections survive metadata and core changes.
    ///
    /// Args:
    ///     catalog: Shared full row hierarchy and availability indices.
    ///     selected: Canonical exact selection, or `None` for implicit All.
    ///     search: Retained query shared with the filtered replacement.
    ///
    /// Returns:
    ///     A delegate exposing every row regardless of the retained query.
    pub(super) fn unfiltered(
        catalog: Rc<ReportStrategyCatalog>,
        selected: Option<&HashSet<ReportStrategyKey>>,
        search: ReportStrategySearch,
    ) -> Self {
        Self::with_initial_query(catalog, selected, search, false)
    }

    /// Build a delegate with either retained-query or full initial visibility.
    ///
    /// Args:
    ///     catalog: Shared full row hierarchy and availability indices.
    ///     selected: Canonical exact selection, or `None` for implicit All.
    ///     search: Shared retained search text.
    ///     apply_query: Whether the first visible snapshot applies `search`.
    ///
    /// Returns:
    ///     A configured Report strategy delegate.
    fn with_initial_query(
        catalog: Rc<ReportStrategyCatalog>,
        selected: Option<&HashSet<ReportStrategyKey>>,
        search: ReportStrategySearch,
        apply_query: bool,
    ) -> Self {
        let selected = selected.cloned().unwrap_or_default();
        let mut selected_available_by_core = HashMap::new();
        let mut selected_available = 0;
        for key in &selected {
            if catalog.available.contains(key) {
                selected_available += 1;
                *selected_available_by_core.entry(key.core_uid).or_insert(0) += 1;
            }
        }
        let mut delegate = Self {
            catalog,
            matched_groups: Vec::new(),
            selected,
            selected_available,
            selected_available_by_core,
            search,
        };
        if apply_query {
            let query = delegate.search.borrow().clone();
            delegate.apply_search(&query);
        } else {
            delegate.show_all_rows();
        }
        delegate
    }

    /// Expose every catalog section without allocating per-row mappings.
    ///
    /// Returns:
    ///     Nothing; the visible mapping references all immutable source rows.
    fn show_all_rows(&mut self) {
        self.matched_groups = (0..self.catalog.groups.len())
            .map(|source_section| ReportStrategyMatchGroup {
                source_section,
                source_rows: None,
            })
            .collect();
    }

    /// Rebuild the visible groups from the full hierarchy.
    ///
    /// Args:
    ///     query: Case-insensitive strategy/core substring query.
    ///
    /// Returns:
    ///     Nothing; empty groups disappear and the global All row follows normal matching.
    fn apply_search(&mut self, query: &str) {
        if query.is_empty() {
            self.show_all_rows();
            return;
        }
        let normalized_query = query.to_lowercase();
        self.matched_groups = self
            .catalog
            .groups
            .iter()
            .enumerate()
            .filter_map(|(source_section, group)| {
                let source_rows = group
                    .items
                    .iter()
                    .enumerate()
                    .filter_map(|(row, item)| {
                        item.matches_normalized(&normalized_query).then_some(row)
                    })
                    .collect::<Vec<_>>();
                (!source_rows.is_empty()).then_some(ReportStrategyMatchGroup {
                    source_section,
                    source_rows: Some(source_rows),
                })
            })
            .collect();
    }

    /// Map one filtered row to its immutable catalog index.
    ///
    /// Args:
    ///     index: Visible section and row.
    ///
    /// Returns:
    ///     The corresponding full catalog index when the visible row still exists.
    fn source_index(&self, index: MoonComponentIndexPath) -> Option<MoonComponentIndexPath> {
        let matched = self.matched_groups.get(index.section)?;
        let row = match &matched.source_rows {
            Some(rows) => *rows.get(index.row)?,
            None => index.row,
        };
        self.catalog
            .groups
            .get(matched.source_section)?
            .items
            .get(row)?;
        Some(MoonComponentIndexPath::new(row).section(matched.source_section))
    }

    /// Return one immutable catalog item.
    ///
    /// Args:
    ///     index: Full catalog section and row.
    ///
    /// Returns:
    ///     The exact stored item when the index is valid.
    fn catalog_item(&self, index: MoonComponentIndexPath) -> Option<&ReportStrategyItem> {
        self.catalog.groups.get(index.section)?.items.get(index.row)
    }

    /// Return every currently available exact index belonging to one core.
    ///
    /// Args:
    ///     core_uid: Core-wide row being toggled.
    ///
    /// Returns:
    ///     Full-list indices for exact members of the core.
    fn available_core_indices(&self, core_uid: u64) -> &[MoonComponentIndexPath] {
        self.catalog
            .available_by_core
            .get(&core_uid)
            .map_or(&[], Vec::as_slice)
    }

    /// Resolve semantic row interactivity from current availability and selection.
    ///
    /// Args:
    ///     choice: Global, core-wide, or exact row identity.
    ///
    /// Returns:
    ///     Whether the row may change selection.
    fn choice_is_enabled(&self, choice: ReportStrategyChoice) -> bool {
        match choice {
            ReportStrategyChoice::Core(core_uid) => {
                !self.available_core_indices(core_uid).is_empty()
            }
            ReportStrategyChoice::Exact(key) => {
                self.catalog.available.contains(&key) || self.selected.contains(&key)
            }
            ReportStrategyChoice::All => true,
        }
    }

    /// Add one exact key to the cached selection counters.
    ///
    /// Args:
    ///     key: Exact strategy identity to add.
    ///
    /// Returns:
    ///     Whether the key was newly selected.
    fn select_key(&mut self, key: ReportStrategyKey) -> bool {
        if !self.selected.insert(key) {
            return false;
        }
        if self.catalog.available.contains(&key) {
            self.selected_available += 1;
            *self
                .selected_available_by_core
                .entry(key.core_uid)
                .or_insert(0) += 1;
        }
        true
    }

    /// Remove one exact key from the cached selection counters.
    ///
    /// Args:
    ///     key: Exact strategy identity to remove.
    ///
    /// Returns:
    ///     Whether the key was selected.
    fn deselect_key(&mut self, key: ReportStrategyKey) -> bool {
        if !self.selected.remove(&key) {
            return false;
        }
        if self.catalog.available.contains(&key) {
            self.selected_available = self.selected_available.saturating_sub(1);
            if let Some(count) = self.selected_available_by_core.get_mut(&key.core_uid) {
                *count = count.saturating_sub(1);
                if *count == 0 {
                    self.selected_available_by_core.remove(&key.core_uid);
                }
            }
        }
        true
    }

    /// Apply one exact-row atomic change by value rather than filtered index identity.
    ///
    /// Args:
    ///     selection: Mutable live widget selection.
    ///     change: Proposed select or deselect operation.
    ///     source_index: Full catalog index for the visible row.
    ///     item: Exact row resolved from the current filtered view.
    ///
    /// Returns:
    ///     Nothing; duplicate exact values remain impossible.
    fn apply_exact_change(
        &mut self,
        selection: &mut Vec<(MoonComponentIndexPath, ReportStrategyItem)>,
        change: &MoonSearchableListChange,
        source_index: MoonComponentIndexPath,
        item: &ReportStrategyItem,
    ) {
        let ReportStrategyChoice::Exact(key) = item.choice else {
            return;
        };
        match change {
            MoonSearchableListChange::Select { .. } => {
                if self.select_key(key) {
                    selection.push((source_index, item.clone()));
                }
            }
            MoonSearchableListChange::Deselect { .. } => {
                if self.deselect_key(key) {
                    selection.retain(|(_, selected)| selected.choice != item.choice);
                }
            }
        }
    }
}

impl MoonSearchableListDelegate for ReportStrategyDelegate {
    type Item = ReportStrategyItem;

    /// Return the number of visible global/core sections.
    ///
    /// Args:
    ///     _: Application context unused by retained in-memory groups.
    ///
    /// Returns:
    ///     The count of filtered sections.
    fn sections_count(&self, _: &App) -> usize {
        self.matched_groups.len()
    }

    /// Return the visible row count for one section.
    ///
    /// Args:
    ///     section: Visible section index.
    ///
    /// Returns:
    ///     The filtered row count, or zero for an invalid section.
    fn items_count(&self, section: usize) -> usize {
        let Some(matched) = self.matched_groups.get(section) else {
            return 0;
        };
        matched.source_rows.as_ref().map_or_else(
            || {
                self.catalog
                    .groups
                    .get(matched.source_section)
                    .map_or(0, |group| group.items.len())
            },
            Vec::len,
        )
    }

    /// Render a core section title; the global All section intentionally has none.
    ///
    /// Args:
    ///     section: Visible section index.
    ///
    /// Returns:
    ///     The source core title, or no title for the global section or invalid input.
    #[allow(deprecated)]
    fn section(&self, section: usize) -> Option<AnyElement> {
        self.matched_groups
            .get(section)
            .and_then(|matched| self.catalog.groups.get(matched.source_section))?
            .title
            .clone()
            .map(IntoElement::into_any_element)
    }

    /// Return one visible row by filtered section and row.
    ///
    /// Args:
    ///     index: Visible section and row.
    ///
    /// Returns:
    ///     The immutable source item when the visible index is valid.
    fn item(&self, index: MoonComponentIndexPath) -> Option<&Self::Item> {
        self.catalog_item(self.source_index(index)?)
    }

    /// Find one semantic choice in the current visible result.
    ///
    /// Args:
    ///     value: Semantic value to find.
    ///
    /// Returns:
    ///     Its visible index when the current search result contains it.
    fn position<V>(&self, value: &V) -> Option<MoonComponentIndexPath>
    where
        Self::Item: MoonSearchableListItem<Value = V>,
        V: PartialEq,
    {
        self.matched_groups
            .iter()
            .enumerate()
            .find_map(|(section, matched)| {
                let group = self.catalog.groups.get(matched.source_section)?;
                match &matched.source_rows {
                    Some(source_rows) => source_rows
                        .iter()
                        .position(|source_row| {
                            group
                                .items
                                .get(*source_row)
                                .is_some_and(|item| item.value() == value)
                        })
                        .map(|row| MoonComponentIndexPath::new(row).section(section)),
                    None => group
                        .items
                        .iter()
                        .position(|item| item.value() == value)
                        .map(|row| MoonComponentIndexPath::new(row).section(section)),
                }
            })
    }

    /// Refilter rows while retaining the query for the next metadata replacement.
    ///
    /// Args:
    ///     query: Current strategy/core query.
    ///     _: Window and application contexts unused by synchronous in-memory filtering.
    ///
    /// Returns:
    ///     An already-complete task after the visible index map is rebuilt.
    fn perform_search(&mut self, query: &str, _: &mut Window, _: &mut App) -> Task<()> {
        self.search.replace(query.to_string());
        self.apply_search(query);
        Task::ready(())
    }

    /// Disable core-wide rows that have no currently available exact children.
    ///
    /// Selected stale exact rows remain enabled only until the user removes them; unavailable
    /// unselected rows cannot be reintroduced before metadata confirms them again.
    ///
    /// Args:
    ///     _: Visible index and application context unused by semantic availability checks.
    ///     item: Row whose current interactivity is requested.
    ///
    /// Returns:
    ///     Whether the row may change selection.
    fn is_item_enabled(&self, _: MoonComponentIndexPath, item: &Self::Item, _: &App) -> bool {
        self.choice_is_enabled(item.choice)
    }

    /// Check global/core action rows from their exact child set.
    ///
    /// Cached exact membership and per-core counts keep every virtual row check constant-time.
    ///
    /// Args:
    ///     _: Visible index, widget snapshot, and application context are superseded by the
    ///        delegate's synchronized exact-key cache.
    ///     item: Row whose checkbox state is requested.
    ///
    /// Returns:
    ///     Whether the global, core-wide, or exact row is checked.
    fn is_item_checked(
        &self,
        _: MoonComponentIndexPath,
        item: &Self::Item,
        _: &[(MoonComponentIndexPath, Self::Item)],
        _: &App,
    ) -> bool {
        match item.choice {
            ReportStrategyChoice::All => {
                self.selected.is_empty()
                    || (!self.catalog.available.is_empty()
                        && self.selected.len() == self.catalog.available.len()
                        && self.selected_available == self.catalog.available.len())
            }
            ReportStrategyChoice::Core(core_uid) => {
                let members = self.available_core_indices(core_uid);
                !members.is_empty()
                    && self
                        .selected_available_by_core
                        .get(&core_uid)
                        .copied()
                        .unwrap_or(0)
                        == members.len()
            }
            ReportStrategyChoice::Exact(key) => self.selected.contains(&key),
        }
    }

    /// Convert global/core action clicks into the canonical exact selection.
    ///
    /// Args:
    ///     selection: Mutable widget selection stored by full catalog identity.
    ///     changes: Atomic visible-row changes proposed by MoonUI.
    ///
    /// Returns:
    ///     Nothing; cached membership is updated in the same pass as the widget vector.
    fn on_will_change(
        &mut self,
        selection: &mut Vec<(MoonComponentIndexPath, Self::Item)>,
        changes: &[MoonSearchableListChange],
    ) {
        for change in changes {
            let index = match change {
                MoonSearchableListChange::Select { index }
                | MoonSearchableListChange::Deselect { index } => *index,
            };
            let Some(source_index) = self.source_index(index) else {
                continue;
            };
            let Some(item) = self.catalog_item(source_index).cloned() else {
                continue;
            };
            match item.choice {
                ReportStrategyChoice::All => {
                    selection.clear();
                    self.selected.clear();
                    self.selected_available = 0;
                    self.selected_available_by_core.clear();
                }
                ReportStrategyChoice::Core(core_uid) => {
                    let member_indices = self.available_core_indices(core_uid).to_vec();
                    if member_indices.is_empty() {
                        continue;
                    }
                    let members = member_indices
                        .into_iter()
                        .filter_map(|member_index| {
                            let member = self.catalog_item(member_index)?.clone();
                            let ReportStrategyChoice::Exact(key) = member.choice else {
                                return None;
                            };
                            Some((member_index, member, key))
                        })
                        .collect::<Vec<_>>();
                    let all_selected = self
                        .selected_available_by_core
                        .get(&core_uid)
                        .copied()
                        .unwrap_or(0)
                        == members.len();
                    if all_selected {
                        let member_keys = members
                            .iter()
                            .map(|(_, _, key)| *key)
                            .collect::<HashSet<_>>();
                        selection.retain(|(_, selected)| match selected.choice {
                            ReportStrategyChoice::Exact(key) => !member_keys.contains(&key),
                            ReportStrategyChoice::All | ReportStrategyChoice::Core(_) => false,
                        });
                        for key in member_keys {
                            self.deselect_key(key);
                        }
                    } else {
                        for (member_index, member, key) in members {
                            if self.select_key(key) {
                                selection.push((member_index, member));
                            }
                        }
                    }
                }
                ReportStrategyChoice::Exact(_) => {
                    self.apply_exact_change(selection, change, source_index, &item);
                }
            }
        }
    }
}

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

/// Build the global All row plus one strategy section per core.
///
/// Args:
///     strategies: Exact strategy identities and display names.
///     ordered_cores: Database cores arranged by the canonical ordering policy.
///     all_label: Localized global and core-wide All-strategies label.
///     manual_label: Localized label for manual orders.
///
/// Returns:
///     A title-less global section followed by core sections with group and exact rows.
pub(super) fn strategy_groups(
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
    groups.extend(ordered_cores.iter().map(|(core_uid, core_name)| {
        let mut items = vec![strategy_item(
            ReportStrategyChoice::Core(*core_uid),
            all_label,
            core_name,
        )];
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
        ReportStrategyGroup {
            title: Some(core_name.clone().into()),
            items,
        }
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
pub(super) fn strategy_choice_indices(
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
pub(super) fn exact_strategy_selection(
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
pub(super) fn strategy_selection_is_all(
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
pub(super) fn strategy_selection_summary(
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
pub(super) fn normalized_strategy_filter_keys(
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
pub(super) fn merge_strategy_metadata(
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

#[cfg(test)]
mod tests;
