//! MoonTree node helpers.

use super::*;

/// Data for one tree row, looked up by node ID from `render_row` and decorators.
pub(crate) enum NodeData {
    /// Always-expanded, non-interactive heading for one canonical exchange section.
    Exchange {
        label: String,
        logo: Option<Arc<RenderImage>>,
    },
    Core {
        core: CoreId,
        label: String,
        active: usize,
        total: usize,
        open_orders: usize,
        selected: bool,
        /// Summary of covered strategies' displayed checkboxes, not of `active`/`total`.
        checked: bool,
        /// The core's global strategy engine: Some(true) running, Some(false) stopped, None when the core has not confirmed either.
        engine: Option<bool>,
    },
    Folder {
        /// What the folder holds, which decides whether its caret and checkbox are drawn at all.
        fill: FolderFill,
        core: CoreId,
        path: Vec<String>,
        label: String,
        active: usize,
        total: usize,
        /// Whether a click selected the folder for highlighting and Ctrl+C folder copying.
        selected: bool,
        /// Summary of covered strategies' displayed checkboxes, not of `active`/`total`.
        checked: bool,
        /// The core's global strategy engine: Some(true) running, Some(false) stopped, None when the core has not confirmed either.
        engine: Option<bool>,
    },
    Strategy {
        core: CoreId,
        id: u64,
        name: String,
        kind: String,
        open_orders: usize,
        server_checked: bool,
        staged: Option<bool>,
        /// Same confirmed engine state used by the core heading; unknown is not running.
        engine: Option<bool>,
        highlighted: bool,
        is_short: bool,
        /// Marked by Cut and waiting for the paste that moves it, so the row draws dimmed.
        cut: bool,
        /// The whole selection of this core when the row belongs to it, else `None` — a row
        /// outside the selection drags only its own `id`.
        ///
        /// Sharing one list across selected rows keeps large multi-selections linear, while the
        /// `None` case keeps ordinary unselected rows allocation-free.
        drag_ids: Option<Rc<[u64]>>,
    },
    /// Core's Deleted folder: strategies absent from the server and retained only in the local DB.
    /// The DB retains their folder paths for restoration, while the UI lists them flat here.
    DeletedFolder { core: CoreId, count: usize },
    DeletedStrategy {
        core: CoreId,
        id: u64,
        name: String,
        kind: String,
        is_short: bool,
        highlighted: bool,
    },
}

/// Adapter result containing tree items, the side map, expanded IDs, and visible flat order.
pub(crate) struct MoonTreeBuild {
    pub(crate) items: Vec<MoonTreeItem>,
    pub(crate) node_data: HashMap<SharedString, NodeData>,
    pub(crate) expanded_ids: Vec<SharedString>,
    pub(crate) flat: Vec<Key>,
    /// Every drawn row in draw order, cores and folders included, for keyboard navigation.
    pub(crate) nav: Vec<ops::NavNode>,
    pub(crate) searching: bool,
}
