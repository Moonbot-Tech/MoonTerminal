//! Core collection and cross-core activity summaries.

use super::*;

impl Default for CoreData {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Default)]
pub struct CoreStore {
    cores: HashMap<CoreId, CoreData>,
}

impl CoreStore {
    pub fn ensure(&mut self, id: CoreId) {
        self.cores.entry(id).or_default();
    }

    /// Remove account data for a core whose server was removed from configuration.
    /// The session lifecycle separately removes its feed handle, market client, and coordination
    /// state.
    pub fn remove(&mut self, id: CoreId) {
        self.cores.remove(&id);
    }

    pub fn core(&self, id: CoreId) -> Option<&CoreData> {
        self.cores.get(&id)
    }

    pub fn core_mut(&mut self, id: CoreId) -> Option<&mut CoreData> {
        self.cores.get_mut(&id)
    }

    /// Iterate over owned snapshots of every core's status for Settings badges.
    pub fn statuses(&self) -> impl Iterator<Item = (CoreId, ConnStatus)> + '_ {
        self.cores.iter().map(|(id, d)| (*id, d.status.clone()))
    }

    /// Iterate over core ids and data for chart-alert reconciliation and similar consumers.
    pub fn cores(&self) -> impl Iterator<Item = (CoreId, &CoreData)> + '_ {
        self.cores.iter().map(|(id, d)| (*id, d))
    }

    /// Return the combined chart-alert revision across all cores.
    ///
    /// This cheaply detects whether any server-owned alert set changed and gates remote-figure
    /// reconciliation.
    pub fn chart_alerts_activity(&self) -> u64 {
        self.cores
            .values()
            .fold(0u64, |a, c| a.wrapping_add(c.chart_alerts_rev))
    }

    /// Return the combined log revision across all cores.
    ///
    /// This cheaply detects new log lines on any core so the application can request a frame for
    /// windows whose Log tab is active.
    pub fn log_activity(&self) -> u64 {
        self.cores
            .values()
            .fold(0u64, |a, c| a.wrapping_add(c.log_rev))
    }
}
