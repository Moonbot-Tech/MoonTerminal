//! Guarded automation admission at the same serialization point as terminal strategy edits.

use anyhow::{Context, Result, ensure};
use serde_json::json;

use super::{LocalStratEdits, MoonClient, StrategyPlacementGuard, rebuild_sync};
use crate::automation::{Action, Receipt, Submission};

/// Validate current authorization and preview before dispatch; queue success is never execution proof.
pub(super) fn execute(
    client: &MoonClient,
    core: u64,
    request: &Submission,
    placements: &mut StrategyPlacementGuard,
    local_edits: &mut LocalStratEdits,
) -> Result<Receipt> {
    request.admitted()?;
    match &request.action {
        Action::StrategyDelete { id } => {
            let snap = client.snapshot().context("no live snapshot")?;
            let (mut effective, _) = super::overlay_pending_edits(
                snap.strats(),
                placements.pending_order(snap.strats().last_modified()),
            );
            crate::automation::apply_strategy(
                &mut effective,
                snap.strats().strategy_schema(),
                &request.action,
                &request.expected,
                crate::util::now_unix_ms_i64() as u64,
            )?;
            client.strategies().delete(*id, "")?;
            Ok(json!({"status":"submitted","strategy_id":id.to_string(),"execution_confirmed":false}).into())
        }
        Action::StrategyEdit { .. } | Action::StrategyCreate { .. } => {
            let apply_orders = matches!(
                &request.action,
                Action::StrategyEdit {
                    apply_to_orders: true,
                    ..
                }
            );
            let mut outcome = Err(anyhow::anyhow!("strategy snapshot unavailable"));
            let queued = rebuild_sync(
                client,
                core,
                "automation",
                placements,
                None,
                apply_orders,
                |full, schema, now| {
                    outcome = crate::automation::apply_strategy(
                        full,
                        schema,
                        &request.action,
                        &request.expected,
                        now,
                    );
                    usize::from(outcome.is_ok())
                },
            );
            let preview = outcome?;
            ensure!(queued, "ActiveLib rejected the strategy submission");
            let id = crate::automation::ids::parse(&preview["id"])?;
            local_edits.mark(id);
            Ok(json!({"status":"submitted","strategy_id":id.to_string(),"execution_confirmed":false,"expected":preview["sent"]}).into())
        }
        _ => {
            ensure!(
                crate::automation::preview(client, &request.action)? == request.expected,
                "state changed since preview"
            );
            crate::automation::execute(client, &request.action, &request.expected)
        }
    }
}
