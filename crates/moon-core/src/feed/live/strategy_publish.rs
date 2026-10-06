//! Strategy publication signatures and delivery acknowledgements.

use super::*;

/// Return whether one normalized strategy generation still needs database delivery.
pub(super) fn strategy_db_export_due(
    schema_ready: bool,
    schema_revision: u64,
    strategy_signature: u64,
    delivered: Option<(u64, u64)>,
) -> bool {
    schema_ready && Some((schema_revision, strategy_signature)) != delivered
}

/// Apply one writer acknowledgement without consuming a failed strategy generation.
pub(super) fn apply_strategy_delivery_ack(
    generation: (u64, u64),
    committed: bool,
    delivered: &mut Option<(u64, u64)>,
    retry_due: &mut bool,
    initial: &mut bool,
) {
    *retry_due = !committed;
    if committed {
        *delivered = Some(generation);
        *initial = false;
    }
}

/// Fold the currently open strategy-edit set into a change signature.
///
/// Kept SEPARATE from the confirmed-snapshot fold in the strategies block below: submitting or
/// resolving an edit changes no core-confirmed `StrategySnapshot`, so folding this into that
/// signature would make that signature the only thing keeping the strategy-edit carrier alive —
/// a later refactor of the confirmed-snapshot fold would then kill this feature silently, with
/// nothing left to notice.
pub(super) fn strategy_edit_sig<'a>(
    edits: impl Iterator<Item = (u64, &'a moonproto::state::StrategyEdit)>,
) -> u64 {
    let mut sig = 0u64;
    for (id, edit) in edits {
        sig = sig
            .wrapping_mul(1099511628211)
            .wrapping_add(id)
            .wrapping_add(edit.status() as u64)
            .wrapping_add(edit.submitted_at().unix_millis() as u64);
    }
    sig
}

/// Log the desired-vs-echo revision for one resolved strategy edit, then queue it as a resolved
/// note for the next `FeedMsg::StrategyEdits` publish.
///
/// The desired side comes from `desired_cache`, populated at `EditSubmitted` time, because
/// moonproto removes the edit from its map in the same step that resolves it: by the time this
/// runs, the live state has nothing left to read for the desired half. The log line exists to
/// settle, on the first live run, whether the Delphi core renumbers `strategy_ver` when it
/// adjusts or supersedes a submitted edit. An Adjusted line also names the fields that differ,
/// as `name: sent -> saved`, because the revision pair alone stays equal when the core rewrites
/// a value.
pub(super) fn record_strategy_edit_resolution(
    echo_snap: Option<&MoonStateSnapshot>,
    desired_cache: &mut std::collections::HashMap<u64, moonproto::StrategySnapshot>,
    strategy_ids: &[u64],
    result: StrategyEditResult,
    core_id: u64,
    notes: &mut Vec<StrategyEditResolution>,
) {
    for &id in strategy_ids {
        let desired = desired_cache.remove(&id);
        let echo_rev = echo_snap
            .and_then(|snap| snap.strats().snapshot(id))
            .map(|strategy| (strategy.strategy_ver, strategy.last_date));
        let desired_rev = desired
            .as_ref()
            .map(|strategy| (strategy.strategy_ver, strategy.last_date));
        let changes = if result == StrategyEditResult::Adjusted {
            adjusted_field_changes(echo_snap, desired.as_ref(), id)
        } else {
            Vec::new()
        };
        if result == StrategyEditResult::Adjusted {
            log::info!(
                "core {} strategy {id} edit Adjusted: desired(ver,last_date)={desired_rev:?} echo(ver,last_date)={echo_rev:?}{}",
                crate::feed::core_label(core_id),
                super::strategies::format_adjustment_log(&changes),
            );
        } else {
            log::info!(
                "core {} strategy {id} edit {result:?}: desired(ver,last_date)={desired_rev:?} echo(ver,last_date)={echo_rev:?}",
                crate::feed::core_label(core_id)
            );
        }
        notes.push(StrategyEditResolution {
            id,
            result,
            changes,
        });
    }
}

/// Field differences for one Adjusted id, or nothing when either snapshot is already gone.
pub(super) fn adjusted_field_changes(
    echo_snap: Option<&MoonStateSnapshot>,
    desired: Option<&moonproto::StrategySnapshot>,
    id: u64,
) -> Vec<super::StrategyFieldChange> {
    let (Some(desired), Some(snap)) = (desired, echo_snap) else {
        return Vec::new();
    };
    let Some(echo) = snap.strats().snapshot(id).cloned() else {
        return Vec::new();
    };
    super::strategies::strategy_field_changes(snap.strats().strategy_schema(), desired, &echo)
}
