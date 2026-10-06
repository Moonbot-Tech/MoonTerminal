//! Validation of report alive maps against the pending catch-up.

use super::*;

/// What to do with an arriving report alive map.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AliveAction {
    /// Route the map and checkpoint for atomic application by the writer.
    Apply(ReportSyncCheckpoint),
    /// The core serves another report database: wipe the replica and sync from zero.
    Wipe,
    /// Drop the map, naming why for the log.
    Ignore(&'static str),
}

/// Decide what an arriving alive map may do, given the catch-up this feed asked it to reconcile.
///
/// Every rejection here protects the checkpoint. A map is authoritative over
/// `1..=covered_up_to`, so applying one that does not describe the catch-up whose checkpoint is
/// about to be stored would hide live rows and then record the damage as reconciled. The ticket
/// match matters because a second `SyncComplete` replaces the pending pair — and the library
/// likewise replaces its active request — so a late map from the previous pass must be dropped,
/// not applied against the newer checkpoint. A matching `DatabaseRecreated` result bypasses the
/// epoch and coverage comparison because its purpose is to report that those values cannot agree.
pub(super) fn alive_map_action(
    pending: Option<&(ReportAliveMapTicket, ReportSyncComplete)>,
    ticket: ReportAliveMapTicket,
    epoch: i32,
    covered_up_to: i64,
    outcome: ReportAliveMapOutcome,
) -> AliveAction {
    let Some((wanted, done)) = pending else {
        return AliveAction::Ignore("карта не запрашивалась");
    };
    if *wanted != ticket {
        return AliveAction::Ignore("карта от другого запроса");
    }
    if outcome == ReportAliveMapOutcome::DatabaseRecreated {
        return AliveAction::Wipe;
    }
    if epoch != done.epoch || covered_up_to != done.max_rec_id {
        return AliveAction::Ignore("карта описывает другой catch-up");
    }
    AliveAction::Apply(done.checkpoint())
}
