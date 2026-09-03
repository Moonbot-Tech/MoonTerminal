//! Process-lifetime browsing snapshot for the Strategies tool window.
//!
//! Survives close and reopen of the window while this process is running. Deliberately not
//! serialized: a full application restart still opens the window from construction defaults.

use super::*;

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
    /// Selected folder or core root.
    pub(crate) selected_folder: Option<(CoreId, String)>,
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
            selected_folder: view.selected_folder.clone(),
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
