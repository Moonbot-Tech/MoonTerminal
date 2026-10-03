//! Live actions that waited out a lost connection are dropped, not delivered late.
//!
//! A core's command queue outlives a disconnect, and MoonProto defers what it is handed until the
//! next Ready. Desired state — settings, strategy edits, subscriptions — is right to arrive late. A
//! trading action is not: a Stop pressed during an outage, or an order priced off a chart minutes
//! ago, would fire whenever the core came back, possibly an hour later. So a live action is
//! delivered only while the connection is operational, only if it was queued after the connection
//! last became operational, and only while it is younger than [`LIVE_TTL`]. Anything else is
//! logged and dropped. Manual orders that already passed into the settings sequence are dropped
//! there when the connection is lost (`ClientSettingsSequence::drop_orders_of_lost_connection`);
//! a temporary ban that passed is desired state from then on, within the drift the sequence
//! already tolerates for it (`TEMP_BLACKLIST_DRIFT`). Run switches and Cancel all also
//! refuse a core that is not connected before queueing (`session::run_dispatch`); the other live
//! actions rely on this gate.

use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant};

use crate::feed::{CoreCmd, QueuedCmd};

#[cfg(test)]
mod tests;

/// The longest a live action may wait in the queue and still be delivered.
pub(super) const LIVE_TTL: Duration = Duration::from_secs(15);

/// Longest command description a drop log line carries.
const LOG_DETAIL_CHARS: usize = 160;

/// Why a live action is not delivered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Stale {
    /// The connection is not operational now.
    NotReady,
    /// It was queued before the connection last became operational: it waited out an outage.
    BeforeReady,
    /// It waited longer than [`LIVE_TTL`].
    TooOld,
}

impl Stale {
    /// Log wording of the reason.
    fn text(self) -> &'static str {
        match self {
            Stale::NotReady => "the core is not connected",
            Stale::BeforeReady => "it was queued before the connection came back",
            Stale::TooOld => "it waited longer than the live-action limit",
        }
    }
}

/// Whether a late delivery of `cmd` does harm: a trading or account action, as opposed to desired
/// state that is right to arrive late.
///
/// Exhaustive on purpose: a new command must be classified here, not fall into a default.
pub(super) fn is_live_action(cmd: &CoreCmd) -> bool {
    match cmd {
        // Checkbox edits alone are configuration; Start/Stop is the engine itself.
        CoreCmd::StrategiesAction { start_stop, .. } => start_stop.is_some(),
        CoreCmd::TransferAsset { .. }
        | CoreCmd::ConvertDust
        | CoreCmd::PlaceOrder { .. }
        | CoreCmd::PlacePendingOrder { .. }
        | CoreCmd::MoveOrder { .. }
        | CoreCmd::CancelOrder { .. }
        | CoreCmd::SetOrderStop { .. }
        | CoreCmd::MoveOrderStopPrice { .. }
        | CoreCmd::UpdateOrderStopsForm { .. }
        | CoreCmd::SetHedgeMode(_)
        | CoreCmd::SetLeverage { .. }
        | CoreCmd::PanicSellMarket { .. }
        | CoreCmd::TurnOrderPanicSell { .. }
        | CoreCmd::MarketSellPosition { .. }
        | CoreCmd::LimitClosePosition { .. }
        | CoreCmd::MarketSellToken { .. }
        | CoreCmd::LimitSellToken { .. }
        | CoreCmd::CancelMarketBuys { .. }
        | CoreCmd::JoinSells { .. }
        | CoreCmd::SplitOrder { .. }
        | CoreCmd::SplitOrderForMarket { .. }
        | CoreCmd::ShiftOrdersPercent { .. }
        | CoreCmd::MoveOrdersToPrice { .. }
        | CoreCmd::SellsToZone { .. }
        | CoreCmd::RestartNow
        | CoreCmd::UpdateVersion { .. }
        | CoreCmd::SetAutoDetect(_)
        | CoreCmd::CancelAllOrders
        // Irreversible one-shots: a late one wipes what accumulated since the press.
        | CoreCmd::ResetProfit(_)
        | CoreCmd::ClearProblems
        // A ban's duration counts from delivery, so a late one outlasts what was asked for.
        | CoreCmd::SetTempBlacklist { .. } => true,
        CoreCmd::SetMarket { .. }
        | CoreCmd::EditStrategyFields { .. }
        | CoreCmd::DeleteStrategy { .. }
        | CoreCmd::DeleteStrategyIfUnchanged { .. }
        | CoreCmd::DeleteFolder { .. }
        | CoreCmd::DeleteEmptyFolder { .. }
        | CoreCmd::CreateStrategies { .. }
        | CoreCmd::RestoreStrategy { .. }
        | CoreCmd::MoveStrategies { .. }
        | CoreCmd::AddFolder { .. }
        | CoreCmd::RemoveFolder { .. }
        | CoreCmd::ReorderStrategies { .. }
        | CoreCmd::RefreshTransferAssets
        | CoreCmd::SetReportRowsDeleted { .. }
        | CoreCmd::RequestReportTraces { .. }
        | CoreCmd::EditClientSettings(_)
        | CoreCmd::EditCoreConfig { .. }
        | CoreCmd::RefreshSharedConfig
        | CoreCmd::SetFavMarket { .. }
        | CoreCmd::SyncGroupExit(_)
        | CoreCmd::SetBlacklist { .. }
        | CoreCmd::SetExcludeBlacklistedDelta(_)
        | CoreCmd::SetDeltasByTrades(_)
        | CoreCmd::ChartAlertUpsert { .. }
        | CoreCmd::ChartAlertDelete { .. }
        | CoreCmd::SetChartText { .. }
        | CoreCmd::TestProblem { .. }
        | CoreCmd::RefreshProblems
        | CoreCmd::Telegram(_) => false,
    }
}

/// Why a live action queued at `queued_at` must not be delivered now, or `None` to deliver it.
///
/// Args:
///     queued_at: When the action entered the queue.
///     ready_since: When the current connection last became operational; `None` while it is not.
///     now: The current instant.
pub(super) fn staleness(
    queued_at: Instant,
    ready_since: Option<Instant>,
    now: Instant,
) -> Option<Stale> {
    let Some(since) = ready_since else {
        return Some(Stale::NotReady);
    };
    if queued_at < since {
        return Some(Stale::BeforeReady);
    }
    (now.saturating_duration_since(queued_at) > LIVE_TTL).then_some(Stale::TooOld)
}

/// Take the next command worth delivering, dropping stale live actions with a log line.
///
/// A stale strategies action loses only its Start/Stop: the checkbox edits it carries are desired
/// state and are still delivered.
///
/// Args:
///     cmd_rx: The core's command queue.
///     ready_since: When the current connection last became operational; `None` while it is not.
///     server_id: Core id, for the log.
///
/// Returns:
///     The next command, or the queue's own `Empty`/`Disconnected`.
pub(super) fn recv_fresh(
    cmd_rx: &Receiver<QueuedCmd>,
    ready_since: Option<Instant>,
    server_id: u64,
) -> Result<CoreCmd, TryRecvError> {
    loop {
        let QueuedCmd { at, cmd } = cmd_rx.try_recv()?;
        if !is_live_action(&cmd) {
            return Ok(cmd);
        }
        let Some(reason) = staleness(at, ready_since, Instant::now()) else {
            return Ok(cmd);
        };
        let detail: String = format!("{cmd:?}").chars().take(LOG_DETAIL_CHARS).collect();
        log::warn!(
            "core {}: live command dropped, {}: {detail}",
            crate::feed::core_label(server_id),
            reason.text()
        );
        match cmd {
            CoreCmd::StrategiesAction { checks, .. } if !checks.is_empty() => {
                return Ok(CoreCmd::StrategiesAction {
                    checks,
                    start_stop: None,
                });
            }
            _ => {}
        }
    }
}
