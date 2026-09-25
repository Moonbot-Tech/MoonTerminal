//! Process-lifetime UI state shared across windows that may be destroyed and recreated.
//!
//! This container deliberately has no serialization. Feature-specific state stays beside the
//! feature that owns its invariants, while [`UiSessionState`] gives [`crate::Backend`] one
//! extensible place to retain those snapshots until the application exits.

use std::time::SystemTime;

use crate::analytics::AnalyticsSessionState;
use crate::strategies::StrategiesSessionState;

/// UI snapshots that survive view and window replacement but reset on application restart.
#[derive(Default)]
pub(crate) struct UiSessionState {
    /// Last restorable state of the Analytics tool window.
    pub(crate) analytics: AnalyticsSessionState,
    /// Last restorable Strategies browsing state, or `None` until this process snapshots one.
    pub(crate) strategies: Option<StrategiesSessionState>,
    /// Wall clock when the Strategies window last closed, or `None` while it is open.
    ///
    /// Stamped from the view's release and cleared on the next open, so time spent open
    /// does not count. Auto uses it to drop tree expansion after fifteen minutes closed.
    /// Not serialized: a restart has no snapshot to restore.
    pub(crate) strategies_closed_at: Option<SystemTime>,
}
