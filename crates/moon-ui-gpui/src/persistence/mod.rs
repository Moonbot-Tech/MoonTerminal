//! File-backed layout persistence: chart-tab specs, Classic dock states, shared Auto dock
//! topology, table column layouts, and the stable panel-identity map used to reconcile the others.

pub(crate) mod auto_dock_persist;
pub(crate) mod chart_persist;
pub(crate) mod coordinator;
pub(crate) mod dock_persist;
pub(crate) mod panel_meta;
pub(crate) mod table_persist;
pub(crate) mod window_state_persist;
