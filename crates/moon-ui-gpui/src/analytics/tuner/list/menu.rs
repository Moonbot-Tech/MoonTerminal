//! Right-click menu of a strategy row: report-only cleanup and full strategy purge, plus the gate
//! deciding whether each action can run.
//!
//! MoonUI's Root owns the open menu (`open_fitted_moon_context_menu`), as it does for the strategies
//! tree and the report row.

use gpui::{Context, Pixels, Point, Window};
use moon_ui::{MoonContextMenuWindowExt as _, MoonMenuItem, MoonTone, MoonWindowExt as _};
use rust_i18n::t;

use super::super::super::AnalyticsView;
use super::super::super::purge::{PurgeMode, PurgeTarget};
use super::super::parse_strat_key;

/// Whether a strategy row may be purged, and why not when it may not.
///
/// Every refusal carries its own reason because the menu renders the item disabled rather than
/// hiding it: an action that silently vanishes is indistinguishable from one that never existed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::analytics) enum PurgeGate {
    /// The row resolves to a live strategy on a connected, report-replicating core.
    Allowed { core_uid: u64, sid: u64 },
    /// `strategyid = 0` — manual orders and unattributed liquidations, not a strategy.
    Manual,
    /// The strategy is already gone from its core; only its trades would be left to delete.
    AlreadyDeleted,
    /// The core does not replicate its report, so a soft-delete would never be echoed back and the
    /// first step could never confirm.
    NoReportFeed,
    /// The core is not connected, or no longer carries this strategy.
    Offline,
    /// The core is live, but it sits outside the focused Auto group, which owns write authority.
    ///
    /// Distinct from [`PurgeGate::Offline`] because the core is perfectly reachable: on
    /// Auto+Overview the unpinned core filter routinely puts such a row on screen, and calling it
    /// "not connected" would send the user off diagnosing a connection that is fine.
    OutOfWorkspace,
}

impl PurgeGate {
    /// Locale key naming the refusal, or `None` when the action is allowed.
    pub(in crate::analytics) fn reason_key(self) -> Option<&'static str> {
        match self {
            PurgeGate::Allowed { .. } => None,
            PurgeGate::Manual => Some("analytics.purge.gate_manual"),
            PurgeGate::AlreadyDeleted => Some("analytics.purge.gate_deleted"),
            PurgeGate::NoReportFeed => Some("analytics.purge.gate_no_report_feed"),
            PurgeGate::Offline => Some("analytics.purge.gate_offline"),
            PurgeGate::OutOfWorkspace => Some("analytics.purge.gate_out_of_workspace"),
        }
    }
}

/// Classify one strategy row against the live core state.
///
/// Pure: report replication, strategy liveness, and workspace membership arrive as closures so
/// the decision can be unit-tested without a window, a backend, or a core session.
///
/// `alive` is the aggregate's liveness marker (0 deleted, 1 disabled, 2 enabled). `None` means no
/// strategy database is attached, which says nothing about core readiness or current placement.
///
/// Refusals are resolved in this order: invalid target, manual strategy, a deleted strategy for a
/// complete purge, disabled report replication, offline core, a missing live strategy for a
/// complete purge, then workspace membership. Asking `in_workspace` last makes an offline core
/// outside the group report the actionable connection state first and keeps every refusal reason
/// and its precedence in one place.
///
/// Args:
///     key: Strategy row key in `strategyid@core_uid` form.
///     alive: Liveness marker carried by the row's aggregate.
///     mode: Report-only cleanup or the complete strategy purge.
///     replicates: Whether that core replicates its report (`ServerConfig.feed.reports`).
///     ready: Whether that core can receive and confirm commands.
///     carries: Whether the connected core still carries this strategy id.
///     in_workspace: Whether the focused Auto group holds write authority over that core.
///
/// Returns:
///     The exact target when allowed, else the reason it is not.
pub(in crate::analytics) fn purge_gate(
    key: &str,
    alive: Option<i64>,
    mode: PurgeMode,
    replicates: impl Fn(u64) -> bool,
    ready: impl Fn(u64) -> bool,
    carries: impl Fn(u64, u64) -> bool,
    in_workspace: impl Fn(u64) -> bool,
) -> PurgeGate {
    let Some((strategy_id, Some(core_uid))) = parse_strat_key(key) else {
        // A legacy key without a core cannot address a soft-delete, which is per core.
        return PurgeGate::Offline;
    };
    if strategy_id == 0 {
        return PurgeGate::Manual;
    }
    if mode == PurgeMode::Whole && alive == Some(0) {
        return PurgeGate::AlreadyDeleted;
    }
    if !replicates(core_uid) {
        return PurgeGate::NoReportFeed;
    }
    let sid = strategy_id as u64;
    if !ready(core_uid) {
        return PurgeGate::Offline;
    }
    if mode == PurgeMode::Whole && !carries(core_uid, sid) {
        return PurgeGate::Offline;
    }
    if !in_workspace(core_uid) {
        return PurgeGate::OutOfWorkspace;
    }
    PurgeGate::Allowed { core_uid, sid }
}

/// Minimum fitted width preserving the menu's existing footprint for short translations.
const MENU_MIN_W: f32 = 340.0;
/// Maximum fitted width before an anomalously long disabled reason truncates inside the viewport.
const MENU_MAX_W: f32 = 560.0;

impl AnalyticsView {
    /// Classify a row using live backend state and the current action authority.
    ///
    /// Args:
    ///     key: Strategy row key in `strategyid@core_uid` form.
    ///     alive: Strategy liveness marker from the row aggregate.
    ///     mode: Report-only cleanup or the complete strategy purge.
    ///     cx: Application context used to read live core and workspace state.
    ///
    /// Returns:
    ///     An allowed exact target only when its core is live and action-authorized.
    pub(in crate::analytics) fn strategy_purge_gate(
        &self,
        key: &str,
        alive: Option<i64>,
        mode: PurgeMode,
        cx: &gpui::App,
    ) -> PurgeGate {
        let backend = self.backend.read(cx);
        let action_cores = self.action_core_ids();
        purge_gate(
            key,
            alive,
            mode,
            |core_uid| {
                // `ServerConfig.id == uid` since schema v11, so the row's core_uid indexes config
                // directly. An unknown core replicates nothing we could wait on.
                backend
                    .config
                    .servers
                    .iter()
                    .find(|server| server.id == core_uid)
                    .is_some_and(|server| server.feed.reports)
            },
            |core_uid| {
                // `CoreStore` keeps the pre-outage strategy snapshot across a disconnect, so
                // readiness is checked separately from strategy presence: report-only cleanup
                // remains valid after the strategy itself has disappeared.
                backend
                    .session
                    .store()
                    .core(core_uid)
                    .is_some_and(|core| core.status == moon_core::feed::ConnStatus::Ready)
            },
            |core_uid, sid| {
                backend
                    .session
                    .store()
                    .core(core_uid)
                    .is_some_and(|core| core.strategies.iter().any(|strategy| strategy.id == sid))
            },
            // Classic holds no group, so it authorizes every core.
            |core_uid| action_cores.is_none_or(|cores| cores.contains(&core_uid)),
        )
    }

    /// Open the strategy row's context menu at `pos`.
    ///
    /// The click does NOT move the selection: making the right-clicked row the anchor would
    /// invalidate the tuner, reset the time grid and trigger a reload as a side effect of merely
    /// opening a menu. The menu and its dialog name the strategy explicitly instead.
    ///
    /// Args:
    ///     key: Strategy row key in `strategyid@core_uid` form.
    ///     name: Display name already resolved by the row.
    ///     core_name: Core label already resolved by the row.
    ///     alive: Liveness marker carried by the row's aggregate.
    ///     period_trades: Trades the row counts for the selected period.
    ///     pos: Mouse position the menu opens at.
    ///     window: Analytics owner window.
    ///     cx: Analytics context.
    #[allow(clippy::too_many_arguments)]
    pub(in crate::analytics) fn open_strategy_row_menu(
        &mut self,
        key: String,
        name: String,
        core_name: String,
        alive: Option<i64>,
        period_trades: i64,
        pos: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let gate = self.strategy_purge_gate(&key, alive, PurgeMode::Whole, cx);
        let label = t!("analytics.purge.menu").to_string();
        let (whole_name, whole_core_name) = (name.clone(), core_name.clone());
        let item = match gate {
            PurgeGate::Allowed { core_uid, sid } => {
                let view = cx.entity();
                MoonMenuItem::with_key("an-purge-strategy", label)
                    .tone(MoonTone::Danger)
                    .on_click(move |_, window, app| {
                        window.close_context_menu(app);
                        let (name, core_name) = (whole_name.clone(), whole_core_name.clone());
                        view.update(app, |this, cx| {
                            this.open_purge_dialog(
                                PurgeMode::Whole,
                                PurgeTarget::new(core_uid, sid, name, core_name, period_trades),
                                window,
                                cx,
                            );
                        });
                    })
            }
            refused => {
                // The greyed item still names the action AND why it is unavailable; hiding it
                // would leave the user hunting for a feature that is simply gated right now.
                let reason = refused
                    .reason_key()
                    .unwrap_or("analytics.purge.gate_offline");
                MoonMenuItem::with_key("an-purge-strategy", format!("{label} — {}", t!(reason)))
                    .disabled(true)
            }
        };
        let rows_gate = self.strategy_purge_gate(&key, alive, PurgeMode::RowsOnly, cx);
        let rows_label = t!("analytics.purge.rows.menu").to_string();
        let rows_item = match rows_gate {
            PurgeGate::Allowed { core_uid, sid } => {
                let view = cx.entity();
                MoonMenuItem::with_key("an-purge-report-rows", rows_label)
                    .tone(MoonTone::Danger)
                    .on_click(move |_, window, app| {
                        window.close_context_menu(app);
                        let (name, core_name) = (name.clone(), core_name.clone());
                        view.update(app, |this, cx| {
                            this.open_purge_dialog(
                                PurgeMode::RowsOnly,
                                PurgeTarget::new(core_uid, sid, name, core_name, period_trades),
                                window,
                                cx,
                            );
                        });
                    })
            }
            refused => {
                let reason = refused
                    .reason_key()
                    .unwrap_or("analytics.purge.gate_offline");
                MoonMenuItem::with_key(
                    "an-purge-report-rows",
                    format!("{rows_label} — {}", t!(reason)),
                )
                .disabled(true)
            }
        };
        // No `cx.notify()`: the menu is Root-owned and repaints itself, while a notify here would
        // rebuild the whole Analytics tree — this entity has none of the stacked repaint throttles
        // the dock panels sit behind.
        window.open_fitted_moon_context_menu(
            cx,
            "an-strat-row-menu",
            pos,
            vec![rows_item, MoonMenuItem::separator(), item],
            MENU_MIN_W,
            MENU_MAX_W,
        );
    }
}

#[cfg(test)]
mod tests;
