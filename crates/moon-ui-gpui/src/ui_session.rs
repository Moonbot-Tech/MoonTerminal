//! Process-lifetime UI state shared across windows that may be destroyed and recreated.
//!
//! This container deliberately has no serialization. Feature-specific state stays beside the
//! feature that owns its invariants, while [`UiSessionState`] gives [`crate::Backend`] one
//! extensible place to retain those snapshots until the application exits.

use crate::analytics::AnalyticsSessionState;
use crate::strategies::StrategiesSessionState;

/// UI snapshots that survive view and window replacement but reset on application restart.
#[derive(Default)]
pub(crate) struct UiSessionState {
    /// Last restorable state of the Analytics tool window.
    pub(crate) analytics: AnalyticsSessionState,
    /// Last restorable Strategies browsing state, or `None` until this process snapshots one.
    pub(crate) strategies: Option<StrategiesSessionState>,
}
