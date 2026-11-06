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

/// Preserve the resolution's own snapshots; a later live read may already describe another edit.
pub(super) fn record_strategy_edit_resolution(
    schema: Option<&moonproto::StrategySchema>,
    resolutions: &[moonproto::state::StrategyResolution],
    core_id: u64,
    notes: &mut Vec<StrategyEditResolution>,
) {
    for resolution in resolutions {
        let id = resolution.desired.strategy_id;
        let result = match resolution.outcome {
            moonproto::state::StrategyResolutionKind::Confirmed => StrategyEditResult::Confirmed,
            moonproto::state::StrategyResolutionKind::Adjusted => StrategyEditResult::Adjusted,
            moonproto::state::StrategyResolutionKind::Superseded => StrategyEditResult::Superseded,
        };
        let sent_revision = (
            resolution.desired.strategy_ver,
            resolution.desired.last_date,
        );
        let actual_revision = (resolution.actual.strategy_ver, resolution.actual.last_date);
        let changes = if result == StrategyEditResult::Adjusted {
            super::strategies::strategy_field_changes(
                schema,
                &resolution.desired,
                &resolution.actual,
            )
        } else {
            Vec::new()
        };
        log::info!(
            "core {} strategy {id} edit {result:?}: desired(ver,last_date)={sent_revision:?} echo(ver,last_date)={actual_revision:?}{}",
            crate::feed::core_label(core_id),
            super::strategies::format_adjustment_log(&changes),
        );
        notes.push(StrategyEditResolution {
            id,
            result,
            sent_revision,
            actual: resolution.actual.clone(),
            changes,
        });
    }
}
