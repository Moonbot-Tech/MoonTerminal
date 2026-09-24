//! Opening the trade-detail window from a Report row.
//!
//! The row carries only DISPLAY cells; resolving it into a replica record is this module's job,
//! the read and the window are the shared opener's (`trade_window::open_record`) — the same one
//! the tuner's deal table opens through, so the two lists cannot disagree about what a trade is.

use gpui::*;
use rust_i18n::t;

use super::{ReportPanel, columns, selection};
use crate::trade_window::open_record::{RecordKey, RecordTarget, open_trade_record};

impl ReportPanel {
    /// Open the dedicated window for the trade on one row, if that row can be resolved.
    ///
    /// Silent when the row cannot be resolved — a double-click has nowhere to put a reason, which
    /// is precisely why the row MENU carries the same action with a disabled arm that states one.
    ///
    /// Args:
    ///     row: Visible row index.
    ///     cx: Panel context.
    pub(super) fn open_trade_detail(&mut self, row: usize, cx: &mut Context<Self>) {
        let Some(target) = self.trade_detail_target(row, cx) else {
            return;
        };
        self.open_trade_detail_target(target, cx);
    }

    /// Open the window for a target resolved EARLIER.
    ///
    /// The row-menu path resolves at menu-build time and calls this, so the action cannot drift
    /// onto a different trade if the table is republished while the menu is open.
    ///
    /// Args:
    ///     target: The already-resolved row.
    ///     cx: Panel context.
    pub(super) fn open_trade_detail_target(
        &mut self,
        target: RecordTarget,
        cx: &mut Context<Self>,
    ) {
        // These stamps render replicated columns, so they follow the report axis rather than the
        // header clock -- the same split the Report grid makes for the very same two values.
        let axis = self.report_axis();
        open_trade_record(&self.backend, axis, target, cx);
    }

    /// Resolve one row into everything the window needs, or nothing.
    ///
    /// A `None` here is also what the row menu renders its disabled arm from: the market resolves
    /// against the core's LIVE catalog, so an offline core stops here — the same boundary the
    /// existing coin cell already stops at.
    ///
    /// Args:
    ///     row: Visible row index.
    ///     cx: Panel context.
    ///
    /// Returns:
    ///     The resolved target, or `None`.
    pub(super) fn trade_detail_target(&self, row: usize, cx: &App) -> Option<RecordTarget> {
        let data = self.data.data()?;
        let core = data.core_uids.get(row).copied()?;
        let record_id = match data.row_keys.get(row)?.as_ref()? {
            selection::ReportRowKey::Replicated { rec_id, .. } => *rec_id,
            selection::ReportRowKey::Legacy { db_id, .. } => *db_id,
        };
        let values = data.rows.get(row)?;
        let coin = self
            .cols
            .iter()
            .position(|col| col == "coin")
            .and_then(|ix| values.get(ix))
            .map(columns::value_to_string)
            .filter(|coin| !coin.is_empty())?;
        // Reused rather than copied: this is the coin-to-market rule including the folded-token
        // catalog lookup, and a second spelling of it would open charts on markets that exist
        // nowhere.
        let market = columns::resolve_market(self.backend.read(cx), core, &coin)?;
        Some(RecordTarget {
            core,
            coin,
            record: RecordKey::RecordId(record_id),
            market,
            filter: (*data.filter).clone(),
        })
    }
}

/// Label of the row-menu entry that opens this window.
///
/// Returns:
///     Localized menu label.
pub(super) fn menu_label() -> String {
    t!("trade_window.open").to_string()
}
