//! Process-lifetime browsing snapshot for the Strategies tool window.
//!
//! Survives close and reopen of the window while this process is running. Deliberately not
//! serialized: a full application restart still opens the window from construction defaults.
//! In Auto, a close of at least fifteen minutes drops expansion before that restore; see
//! [`strategies_reopen_collapses`].

use std::time::{Duration, SystemTime};

use moon_core::config::WorkspaceMode;
use moon_core::session::CoreId;

use super::*;

/// How long a closed Strategies window must stay shut, in Auto, before the next open
/// drops tree expansion. Fixed: there is no setting.
pub(super) const STRATEGIES_IDLE_COLLAPSE: Duration = Duration::from_secs(15 * 60);

/// Restorable Strategies browsing state for the current process only.
#[derive(Clone, Default)]
pub(crate) struct StrategiesSessionState {
    /// Cores the user left expanded in the tree, by hand.
    ///
    /// `StrategiesView::rail_expanded_core` — the Auto rail's live seed — is deliberately absent
    /// from this snapshot: capturing it would let a rail seed outlive the window that received it
    /// and reappear as if the user had expanded that core themselves, in another scope or window.
    pub(crate) expanded_cores: HashSet<CoreId>,
    /// Folders the user left expanded, keyed by core and slash-separated path.
    pub(crate) expanded_folders: HashSet<(CoreId, String)>,
    /// Cores whose Deleted folder the user left expanded.
    pub(crate) expanded_deleted: HashSet<CoreId>,
    /// Primary strategy selection.
    pub(crate) selected: Option<Key>,
    /// Multi-selection set.
    pub(crate) sel: HashSet<Key>,
    /// Selected folders and core roots.
    pub(crate) folder_sel: HashSet<(CoreId, String)>,
    /// The folder node last pointed at, which is the paste/create target and the keyboard cursor.
    pub(crate) folder_anchor: Option<(CoreId, String)>,
    /// Selected schema section index.
    pub(crate) selected_section: usize,
    /// Shift-range selection anchor.
    pub(crate) anchor: Option<Key>,
    /// Search box text.
    pub(crate) search: String,
    /// Kind filter ordinal, or `None` for all kinds.
    pub(crate) kind: Option<u8>,
    /// Direction filter: `None` both, `Some(true)` short, `Some(false)` long.
    pub(crate) dir: Option<bool>,
    /// Exchange section filter, or `None` for every exchange.
    pub(crate) exchange: Option<crate::core_order::ExchangeSection>,
    /// Empty UI folders that are tree structure without live strategies.
    pub(crate) ui_folders: HashSet<(CoreId, String)>,
}

impl StrategiesSessionState {
    /// Snapshot the view's user-visible browsing fields.
    ///
    /// Args:
    ///     view: Live Strategies view whose browsing state is copied.
    ///
    /// Returns:
    ///     An owned snapshot suitable for `UiSessionState`.
    pub(super) fn capture(view: &StrategiesView) -> Self {
        Self {
            expanded_cores: view.expanded_cores.clone(),
            expanded_folders: view.expanded_folders.clone(),
            expanded_deleted: view.expanded_deleted.clone(),
            selected: view.selected,
            sel: view.sel.clone(),
            folder_sel: view.folder_sel.clone(),
            folder_anchor: view.folder_anchor.clone(),
            selected_section: view.selected_section,
            anchor: view.anchor,
            search: view.filter.search.clone(),
            kind: view.filter.kind,
            dir: view.filter.dir,
            exchange: view.filter.exchange,
            ui_folders: view.ui_folders.clone(),
        }
    }
}

/// Elapsed time since the Strategies window closed, or `None` when it has not.
///
/// A backwards wall clock yields `None` so the next open keeps the snapshot instead of
/// treating the jump as a long idle gap.
///
/// Args:
///     closed_at: Wall clock stamped when the window last released, or `None` while it
///         is open or has never closed in this process.
///     now: Wall clock at the moment of the reopen.
///
/// Returns:
///     The non-negative gap, or `None` when there is no close to measure.
pub(super) fn strategies_closed_for(
    closed_at: Option<SystemTime>,
    now: SystemTime,
) -> Option<Duration> {
    closed_at.and_then(|closed| now.duration_since(closed).ok())
}

/// Whether this reopen should drop Strategies tree expansion.
///
/// Auto and a close of at least [`STRATEGIES_IDLE_COLLAPSE`] collapses. A shorter close
/// keeps the snapshot. Classic always keeps it. `None` means the window has not closed
/// in this process, which also keeps the snapshot.
///
/// Args:
///     mode: Workspace preset the singleton window is opening under.
///     closed_for: Time since the window last closed, from [`strategies_closed_for`].
///
/// Returns:
///     `true` when expansion and folder selection should be discarded.
pub(super) fn strategies_reopen_collapses(
    mode: WorkspaceMode,
    closed_for: Option<Duration>,
) -> bool {
    mode == WorkspaceMode::AutoTrading
        && closed_for.is_some_and(|elapsed| elapsed >= STRATEGIES_IDLE_COLLAPSE)
}

/// Drop expansion and the folder selection that only makes sense while those nodes are open.
///
/// Search text, kind, side, and exchange filters stay. Strategy selection stays: it is
/// not expansion, and the parameter pane can still show it. Empty UI folders stay: they
/// are tree structure, not an open/closed bit. Tree scroll is not in this snapshot; the
/// window builds a fresh tree on every open. The active-only filter lives on layout
/// prefs, not here, so this function cannot touch it.
///
/// Args:
///     state: Browsing snapshot about to be restored into a new window.
pub(super) fn collapse_strategies_expansion(state: &mut StrategiesSessionState) {
    state.expanded_cores.clear();
    state.expanded_folders.clear();
    state.expanded_deleted.clear();
    state.folder_sel.clear();
    state.folder_anchor = None;
}

/// The Auto rail overlay to open with.
///
/// A normal open seeds the selected core. An idle collapse opens with every core row
/// shut, so the overlay is `None` even when the rail has a selection. `rail_seen_core`
/// is a different field and still records that selection, so a later revision of the
/// same rail does not treat the collapse as a move and open the row again.
///
/// Args:
///     collapse: Whether [`strategies_reopen_collapses`] dropped the saved expansion.
///     rail_seed: The core `rail_seed_core` would open on an ordinary reopen.
///
/// Returns:
///     The overlay to store in `rail_expanded_core`.
pub(super) fn rail_overlay_on_open(collapse: bool, rail_seed: Option<CoreId>) -> Option<CoreId> {
    if collapse { None } else { rail_seed }
}

#[cfg(test)]
mod tests;
