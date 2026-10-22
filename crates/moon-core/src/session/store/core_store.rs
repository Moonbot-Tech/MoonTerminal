//! Core collection and cross-core activity summaries.

use super::*;

impl Default for CoreData {
    fn default() -> Self {
        Self::new()
    }
}

/// Retained cores sharing one report-offset invalidation epoch.
#[derive(Default)]
pub struct CoreStore {
    cores: HashMap<CoreId, CoreData>,
    time_offset_epoch: Arc<AtomicU64>,
    /// Each core's detect counter, kept past [`Self::remove`] on purpose: a core removed and
    /// restored under the same id continues numbering after what the UI cursors already hold; see
    /// `CoreData::detect_seq`. One `u64` per id ever seen.
    detect_seqs: HashMap<CoreId, Arc<AtomicU64>>,
}

impl CoreStore {
    /// Insert a core with the fleet epoch so every offset adoption invalidates the axis, and with
    /// its own detect counter so its detect numbers never restart.
    pub fn ensure(&mut self, id: CoreId) {
        self.cores.entry(id).or_insert_with(|| {
            let mut core = CoreData::new();
            core.time_offset_epoch = self.time_offset_epoch.clone();
            core.detect_seq = self.detect_seqs.entry(id).or_default().clone();
            core
        });
    }

    /// Return the fleet revision advanced by changed offset snapshots and core removals.
    pub fn time_offset_epoch(&self) -> u64 {
        self.time_offset_epoch.load(Ordering::Relaxed)
    }

    /// Remove account data for a core whose server was removed from configuration.
    /// Invalidate measured axes even when configuration still retains the deactivated core.
    /// The session lifecycle separately removes its feed handle, market client, and coordination state.
    pub fn remove(&mut self, id: CoreId) {
        self.cores.remove(&id);
        self.time_offset_epoch.fetch_add(1, Ordering::Relaxed);
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
