//! The cores the expert window lists, the selection over them, and what differs between the
//! selected ones.
//!
//! GPUI-free: the roster is built from the backend and compared, and the selection is the shared
//! [`RowSelection`] every table-like panel runs. The left column in [`super::sidebar`] only draws
//! what this module decides, so every rule about WHICH core the page shows and WHICH cores OK
//! reaches is testable without a window.
//!
//! Two cores matter at any moment. The ANCHOR is the core whose page is drawn — the row the user
//! last clicked while it is still selected, else the first selected row in list order. The TARGETS
//! are every selected core; OK writes the staged changes to all of them. The anchor is always a
//! target, so the values on screen are always among the values sent.

use std::hash::{Hash, Hasher};

use gpui::SharedString;

use moon_core::feed::CoreConfig;
use moon_core::session::CoreId;

use crate::Backend;
use crate::controls::row_selection::RowSelection;
use crate::controls::{core_menu_sections, venue_section_label};
use crate::core_order::CoreOrder;

/// One core row of the list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RosterRow {
    pub(super) core: CoreId,
    /// Shared, because the list clones it into a text element on every frame.
    pub(super) name: SharedString,
    /// Whether the store holds a LIVE page for this core — the one thing that decides whether an
    /// OK can reach it. A row without one is still listed and selectable, so the user sees the
    /// core they meant rather than a gap, and the footer counts it as skipped.
    pub(super) has_page: bool,
}

/// One exchange section of the list, in the canonical order every core menu uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RosterSection {
    pub(super) label: SharedString,
    pub(super) rows: Vec<RosterRow>,
}

/// Every core the terminal currently runs, grouped by venue.
///
/// ALL of them, not the opening group's: this window exists to change a parameter across the
/// fleet, and a group is a display scope rather than a boundary a settings write respects. The
/// group still decides which core is selected when the window opens.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(super) struct CoreRoster {
    pub(super) sections: Vec<RosterSection>,
    /// Core ids in drawn order, the shape [`RowSelection`] wants for ranges and pruning.
    order: Vec<Option<CoreId>>,
    /// Hash of every input the list was built from; see [`Self::key`].
    key: u64,
}

impl CoreRoster {
    /// Hash of everything the list is built from — each session's id, name, venue and page state,
    /// plus the sort mode — so a sync can tell "nothing changed" without building the list.
    ///
    /// The list is rebuilt on a backend notification, which fires a few times a second on a
    /// large fleet; building it means a rank map, two sorts and a name clone per core, only to be
    /// compared and dropped. Hashing the inputs walks the same sessions with no allocation, and a
    /// collision costs one missed rebuild until the next change — a hazard the 64-bit key makes
    /// negligible against the one it prevents.
    pub(super) fn key(b: &Backend) -> u64 {
        let store = b.session.store();
        let venues = b.session.core_venues();
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        b.config.core_sort.hash(&mut hasher);
        for session in b.session.sessions() {
            session.id.hash(&mut hasher);
            session.name.hash(&mut hasher);
            venues.get(&session.id).map(|v| v.id).hash(&mut hasher);
            store
                .core(session.id)
                .is_some_and(|d| d.live_core_config().is_some())
                .hash(&mut hasher);
        }
        hasher.finish()
    }

    /// Build the roster from the live sessions.
    ///
    /// Args:
    ///     b: Application state the sessions and their pages are read from.
    ///     key: The [`Self::key`] this build answers to, remembered so the next sync can skip.
    pub(super) fn build(b: &Backend, key: u64) -> Self {
        // Into a plain `Vec`, because the names are taken out of it below and the ordered
        // newtype lends its rows read-only.
        let mut cores: Vec<(CoreId, String)> = CoreOrder::new(&b.config)
            .from_sessions(b.session.sessions(), |_| true)
            .into_iter()
            .collect();
        let venues = b.session.core_venues();
        let store = b.session.store();
        let mut order = Vec::with_capacity(cores.len());
        // The sections borrow the ordered list, so they are resolved to positions first and the
        // names are then TAKEN out of it rather than cloned a second time.
        let sections: Vec<(SharedString, Vec<usize>)> = core_menu_sections(&cores, venues)
            .into_iter()
            .map(|(venue, members)| {
                let positions = members
                    .iter()
                    .map(|(core, _)| {
                        cores
                            .iter()
                            .position(|(id, _)| id == core)
                            .expect("a section member comes from the list it was built from")
                    })
                    .collect();
                (SharedString::from(venue_section_label(venue)), positions)
            })
            .collect();
        let sections = sections
            .into_iter()
            .map(|(label, positions)| RosterSection {
                label,
                rows: positions
                    .into_iter()
                    .map(|position| {
                        let (core, name) = &mut cores[position];
                        order.push(Some(*core));
                        RosterRow {
                            core: *core,
                            name: SharedString::from(std::mem::take(name)),
                            has_page: store
                                .core(*core)
                                .is_some_and(|d| d.live_core_config().is_some()),
                        }
                    })
                    .collect(),
            })
            .collect();
        Self {
            sections,
            order,
            key,
        }
    }

    /// The [`Self::key`] this roster was built from.
    pub(super) fn built_from(&self) -> u64 {
        self.key
    }

    /// Core ids in drawn order, `Some` for every row: the list draws no unselectable line.
    pub(super) fn order(&self) -> &[Option<CoreId>] {
        &self.order
    }

    /// Whether the roster lists this core.
    pub(super) fn contains(&self, core: CoreId) -> bool {
        self.order.contains(&Some(core))
    }

    /// Every row in drawn order.
    pub(super) fn rows(&self) -> impl Iterator<Item = &RosterRow> {
        self.sections.iter().flat_map(|s| s.rows.iter())
    }

    /// The selected rows in drawn order.
    pub(super) fn selected<'a>(
        &'a self,
        selection: &'a RowSelection<CoreId>,
    ) -> impl Iterator<Item = &'a RosterRow> + 'a {
        self.rows()
            .filter(move |row| selection.contains(Some(row.core)))
    }

    /// The configured name of one listed core.
    pub(super) fn name(&self, core: CoreId) -> Option<&SharedString> {
        self.rows()
            .find(|row| row.core == core)
            .map(|row| &row.name)
    }

    /// How many selected rows have a live page, and how many have not — what OK reaches and what
    /// it skips.
    pub(super) fn selected_pages(&self, selection: &RowSelection<CoreId>) -> (usize, usize) {
        self.selected(selection)
            .fold((0, 0), |(with, without), row| match row.has_page {
                true => (with + 1, without),
                false => (with, without + 1),
            })
    }
}

/// The core whose page the window draws, given the selection over a roster.
///
/// The last-clicked row while it is still selected; otherwise the first selected row in list
/// order, so Ctrl-clicking the drawn core out of the selection moves the page to a core that IS
/// still going to be written rather than leaving it on one that is not.
pub(super) fn anchor(selection: &RowSelection<CoreId>, roster: &CoreRoster) -> Option<CoreId> {
    selection
        .current()
        .or_else(|| roster.selected(selection).next().map(|row| row.core))
}

/// Every selected core in list order — what OK writes to.
pub(super) fn targets(selection: &RowSelection<CoreId>, roster: &CoreRoster) -> Vec<CoreId> {
    selection.in_order(roster.order())
}

/// Whether staged changes survive the selection moving from `before` to `after`.
///
/// Changes are made FOR the cores selected at the time. Adding a core to that selection (Ctrl,
/// Shift) widens what they are for — that is how a parameter is unified across cores — and
/// dropping some of those cores narrows it; either way a core the changes were made for is still
/// there to receive them. A plain click on another core replaces the selection wholesale, and
/// then no core they were made for is left: that is leaving the page without OK, and leaving
/// without OK saves nothing.
pub(super) fn keeps_changes(before: &[CoreId], after: &[CoreId]) -> bool {
    before.iter().any(|core| after.contains(core))
}

/// One selected core's contribution to the diff key: its page revision and whether that page is
/// live. See [`diff_key`].
pub(super) type DiffKeyEntry = (CoreId, u64, bool);

/// One core's entry of the diff key, read off the store.
fn diff_key_entry(store: &moon_core::session::CoreStore, core: CoreId) -> DiffKeyEntry {
    let entry = store.core(core);
    (
        core,
        entry.map_or(0, |d| d.core_config_rev),
        entry.is_some_and(|d| d.live_core_config().is_some()),
    )
}

/// Whether the diff the window holds was computed for exactly this selection at these page
/// revisions, checked without allocating the key.
///
/// The store bumps `core_config_rev` only when a core's projected page actually changes, and it
/// does NOT bump it when the page merely goes stale or comes back live — so the key carries the
/// live bit as well, or a core the OK will skip would stay in the comparison. This runs on every
/// backend notification, where collecting a `Vec` per target to compare and drop would be the
/// notify-hammer in miniature.
pub(super) fn diff_key_matches(
    b: &Backend,
    targets: impl Iterator<Item = CoreId>,
    key: &[DiffKeyEntry],
) -> bool {
    let store = b.session.store();
    let mut key = key.iter();
    for core in targets {
        if key.next() != Some(&diff_key_entry(store, core)) {
            return false;
        }
    }
    key.next().is_none()
}

/// The revision key the diff is computed for: every selected core with its page revision and
/// whether that page is live.
pub(super) fn diff_key(b: &Backend, targets: &[CoreId]) -> Vec<DiffKeyEntry> {
    let store = b.session.store();
    targets
        .iter()
        .map(|&core| diff_key_entry(store, core))
        .collect()
}

/// The LIVE pages of the given cores, in the given order, for the diff.
///
/// A core without a live page is left out rather than represented by a stale one: the diff is
/// about what OK would find, and OK skips such a core.
pub(super) fn live_pages<'a>(b: &'a Backend, targets: &[CoreId]) -> Vec<&'a CoreConfig> {
    let store = b.session.store();
    targets
        .iter()
        .filter_map(|&core| store.core(core)?.live_core_config())
        .collect()
}

#[cfg(test)]
mod tests;
