//! Which subject the bottom CPU / memory / ping chart follows.
//!
//! The chart draws ONE subject: a picked core's own rings, else a server's machine aggregate.
//! Three selection events move it, and the latest one wins:
//!
//! - a server heading clicked in the Servers view charts that machine's aggregate;
//! - a core picked anywhere in the panel — a Flat row or a Servers chip — charts that core;
//! - the Auto rail switching to a core charts that core, because the panel's scope just
//!   became that core and a chart left on the previous subject would describe a core the
//!   panel no longer shows.
//!
//! The rail is followed on CHANGE only: the panel resamples on every telemetry tick, and
//! re-applying an unchanged rail pick on each one would undo a click made after it.

use moon_core::session::CoreId;

use super::model::ServerKey;

/// The chart's current subject plus the last Auto rail pick it consumed.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(super) struct ChartFollow {
    /// Server whose machine aggregate the chart shows when no core is picked. `None` falls back
    /// to the first addressed server. The Servers view also highlights this heading.
    server: Option<ServerKey>,
    /// Core charted on its own rings. Wins over `server` while the core is in scope.
    core: Option<CoreId>,
    /// Auto rail core already applied, so an unchanged rail never re-takes the chart.
    rail_seen: Option<CoreId>,
}

impl ChartFollow {
    /// Server the chart follows when no core is picked.
    pub(super) fn server(&self) -> Option<ServerKey> {
        self.server
    }

    /// Core the chart follows, when one is picked.
    pub(super) fn core(&self) -> Option<CoreId> {
        self.core
    }

    /// Chart one server's machine aggregate, dropping any picked core.
    ///
    /// Args:
    ///     key: The clicked server heading.
    ///
    /// Returns:
    ///     Whether the subject changed and the chart must repaint.
    pub(super) fn pick_server(&mut self, key: ServerKey) -> bool {
        let changed = self.server != Some(key) || self.core.is_some();
        self.server = Some(key);
        self.core = None;
        changed
    }

    /// Chart one core's own rings.
    ///
    /// Args:
    ///     core: The picked core.
    ///     server: The server that core sits on, when the panel knows it. Kept so the Servers
    ///         view highlights the right heading; `None` keeps the previous one.
    ///
    /// Returns:
    ///     Whether the subject changed and the chart must repaint.
    pub(super) fn pick_core(&mut self, core: CoreId, server: Option<ServerKey>) -> bool {
        let server = server.or(self.server);
        let changed = self.core != Some(core) || self.server != server;
        self.core = Some(core);
        self.server = server;
        changed
    }

    /// Follow the Auto rail's selected core when the rail moved.
    ///
    /// A rail that went back to Overview (`None`) leaves the subject alone: Overview names no
    /// core, so the last pick is still the newest thing the operator chose.
    ///
    /// Args:
    ///     rail: The group's Auto rail core, or `None` in Overview and in Classic mode.
    ///     server: The server that core sits on, when the panel knows it.
    ///
    /// Returns:
    ///     Whether the subject changed and the chart must repaint.
    pub(super) fn follow_rail(&mut self, rail: Option<CoreId>, server: Option<ServerKey>) -> bool {
        if rail == self.rail_seen {
            return false;
        }
        self.rail_seen = rail;
        match rail {
            Some(core) => self.pick_core(core, server),
            None => false,
        }
    }
}

#[cfg(test)]
mod tests;
