//! Right-click menu of a strategy row: the one destructive action the Analytics table offers,
//! plus the gate deciding whether it can run at all.
//!
//! MoonUI's Root owns the open menu (`open_moon_context_menu`), as it does for the strategies
//! tree and the report row.

use gpui::{Context, Pixels, Point, Window};
use moon_ui::{MoonContextMenuWindowExt as _, MoonMenuItem, MoonTone, MoonWindowExt as _};
use rust_i18n::t;

use super::super::super::AnalyticsView;
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
        }
    }
}

/// Classify one strategy row against the live core state.
///
/// Pure: the two live facts arrive as closures so the decision can be unit-tested without a
/// window, a backend, or a core session.
///
/// `alive` is the aggregate's liveness marker (0 deleted, 1 disabled, 2 enabled). `None` means no
/// strategy database is attached, which says nothing about the core — `live` decides that.
///
/// Args:
///     key: Strategy row key in `strategyid@core_uid` form.
///     alive: Liveness marker carried by the row's aggregate.
///     replicates: Whether that core replicates its report (`ServerConfig.feed.reports`).
///     live: Whether that core is connected AND still carries this strategy id.
///
/// Returns:
///     The exact target when allowed, else the reason it is not.
pub(in crate::analytics) fn purge_gate(
    key: &str,
    alive: Option<i64>,
    replicates: impl Fn(u64) -> bool,
    live: impl Fn(u64, u64) -> bool,
) -> PurgeGate {
    let Some((strategy_id, Some(core_uid))) = parse_strat_key(key) else {
        // A legacy key without a core cannot address a soft-delete, which is per core.
        return PurgeGate::Offline;
    };
    if strategy_id == 0 {
        return PurgeGate::Manual;
    }
    if alive == Some(0) {
        return PurgeGate::AlreadyDeleted;
    }
    if !replicates(core_uid) {
        return PurgeGate::NoReportFeed;
    }
    let sid = strategy_id as u64;
    if !live(core_uid, sid) {
        return PurgeGate::Offline;
    }
    PurgeGate::Allowed { core_uid, sid }
}

/// Menu width: the label is a whole sentence, so the strategies tree's 190 would wrap it.
const MENU_W: f32 = 340.0;

impl AnalyticsView {
    /// Classify a row using the live backend state.
    ///
    /// Args:
    ///     key: Strategy row key in `strategyid@core_uid` form.
    ///     alive: Strategy liveness marker from the row aggregate.
    ///     cx: Application context used to read live core and workspace state.
    ///
    /// Returns:
    ///     An allowed exact target only when its core is live and still workspace-visible.
    pub(in crate::analytics) fn strategy_purge_gate(
        &self,
        key: &str,
        alive: Option<i64>,
        cx: &gpui::App,
    ) -> PurgeGate {
        let backend = self.backend.read(cx);
        let gate = purge_gate(
            key,
            alive,
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
            |core_uid, sid| {
                // `CoreStore` keeps the pre-outage strategy snapshot across a disconnect, so
                // "the store knows this strategy" alone would leave the action enabled on a core
                // that cannot receive a single one of the three command types.
                backend.session.store().core(core_uid).is_some_and(|core| {
                    core.status == moon_core::feed::ConnStatus::Ready
                        && core.strategies.iter().any(|strategy| strategy.id == sid)
                })
            },
        );
        match gate {
            PurgeGate::Allowed { core_uid, .. }
                if self
                    .workspace_scope
                    .as_ref()
                    .is_some_and(|scope| !scope.core_ids.contains(&core_uid)) =>
            {
                PurgeGate::Offline
            }
            gate => gate,
        }
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
        let gate = self.strategy_purge_gate(&key, alive, cx);
        let label = t!("analytics.purge.menu").to_string();
        let item = match gate {
            PurgeGate::Allowed { core_uid, sid } => {
                let view = cx.entity();
                MoonMenuItem::with_key("an-purge-strategy", label)
                    .tone(MoonTone::Danger)
                    .on_click(move |_, window, app| {
                        window.close_context_menu(app);
                        let (name, core_name) = (name.clone(), core_name.clone());
                        view.update(app, |this, cx| {
                            this.open_purge_dialog(
                                core_uid,
                                sid,
                                name,
                                core_name,
                                period_trades,
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
        // No `cx.notify()`: the menu is Root-owned and repaints itself, while a notify here would
        // rebuild the whole Analytics tree — this entity has none of the stacked repaint throttles
        // the dock panels sit behind.
        window.open_moon_context_menu(cx, "an-strat-row-menu", pos, vec![item], MENU_W);
    }
}

#[cfg(test)]
mod tests;
