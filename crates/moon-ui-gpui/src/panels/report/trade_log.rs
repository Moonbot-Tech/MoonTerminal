//! Core-log lines belonging to one report trade, read from the rotated log files on disk.
//!
//! The wire protocol carries no identifier on a log line — `MPC_LogMsg` is a timestamp and text
//! (see `moonproto::events::ui`), so nothing links a stored trade to its log through the feed. What
//! does link them is the core's own spelling: every line a trading task writes carries its task
//! number in parentheses, and the report row keeps that number in its `taskid` column. Both were
//! checked against live data: report row `taskid=17062` on core `BinF3` matches the 47 lines
//! carrying `(17062)` in `logs/2026-08-03_BinF3.log`, coin and all.
//!
//! Scope of what this can show, stated up front because each limit is visible to the user:
//!   - the core must have had log relaying on (`feed.log`), or nothing was ever written;
//!   - the day's file must still exist — file logging can be off and retention deletes old days;
//!   - a task number is unique WITHIN one core and is reused after the core restarts, and the scan
//!     narrows the search to the days the trade spans but cannot go finer: a restart inside one of
//!     those days can put another task's lines under the same number, and nothing in the line says
//!     so. Deliberately not filtered by time — the log clock and the report dates come from the
//!     same core, but a bound tight enough to exclude a restart would also drop the trade's own
//!     lead-in and tail;
//!   - two cores configured under the SAME display name write one file, so their lines mix.
//!
//! [`open_trade_log`] resolves those inputs and hands the file scan to a background thread; the
//! dialog itself lives in [`view`].

mod scan;
mod view;

use crate::panels::line_list::RowSelection;
use gpui::*;
use moon_ui::MoonVirtualListScrollHandle;

pub(super) use scan::trade_log_request;

/// What identifies one trade's log: which core wrote it, which task, and over which days.
#[derive(Clone)]
pub(super) struct TradeLogRequest {
    /// Core that ran the trade, used for the window title.
    pub(super) core_name: String,
    /// Coin as the report stored it, used for the window title.
    pub(super) coin: String,
    /// Core task number from the report's `taskid` column.
    pub(super) task_id: i64,
    /// Log-file labels to try, newest configured name first; a renamed core has files under both.
    pub(super) labels: Vec<String>,
    /// UTC days the trade spans, as `YYYY-MM-DD`.
    pub(super) dates: Vec<String>,
}

/// Outcome of the background scan, as the dialog needs to present it.
enum TradeLogState {
    /// The scan is still running.
    Loading,
    /// Lines found, oldest first. `truncated` means the cap stopped the scan early.
    Ready {
        lines: Vec<view::TradeLine>,
        truncated: bool,
    },
}

/// Live dialog state: the request, the scan result, and the row selection for copying.
struct TradeLog {
    request: TradeLogRequest,
    state: TradeLogState,
    selection: RowSelection,
    /// Character budget of the widest line, sizing the horizontal scroll area.
    widest_chars: usize,
    scroll: MoonVirtualListScrollHandle,
    /// Sideways offset of the line viewport, kept across frames.
    hscroll: ScrollHandle,
    /// Focus of the list, so its copy and select-all keys reach this view.
    focus: FocusHandle,
    /// Whether the first frame already claimed focus from the dialog.
    focused_once: bool,
    /// Keeps the background scan alive for as long as the dialog is open.
    _scan: Option<Task<()>>,
}

/// Maximum number of matching lines shown for one trade.
///
/// A trade writes tens of lines; a cap this high is only reached when the task number collides with
/// unrelated text, and reporting the truncation is what tells the user that happened.
const MAX_LINES: usize = 5000;

/// Opens the trade-log dialog and starts the background file scan.
///
/// Args:
///     request: Core, task, and days resolved from the report row.
///     window: Window hosting the dialog.
///     cx: Application context used to spawn the scan and open the dialog.
///
/// Returns:
///     Nothing; the dialog updates itself when the scan lands.
pub(super) fn open_trade_log(request: TradeLogRequest, window: &mut Window, cx: &mut App) {
    let entity = cx.new(|cx| TradeLog {
        request: request.clone(),
        state: TradeLogState::Loading,
        selection: RowSelection::default(),
        widest_chars: 0,
        scroll: MoonVirtualListScrollHandle::new(),
        hscroll: ScrollHandle::new(),
        focus: cx.focus_handle(),
        focused_once: false,
        _scan: None,
    });
    // Reading a day of a core's log is tens of megabytes off disk; it never runs on the frame
    // thread. The task is owned by the dialog state, so closing the dialog drops it — which is why
    // the future holds a WEAK handle: a strong one would close the loop (state -> task -> future ->
    // state) and neither would ever be freed.
    let scan_entity = entity.downgrade();
    let scan = cx.spawn(async move |cx| {
        let executor = cx.update(|cx| cx.background_executor().clone());
        let found = executor
            .spawn(async move { scan::scan_trade_log(&request, MAX_LINES) })
            .await;
        let Some(scan_entity) = scan_entity.upgrade() else {
            return; // The dialog is gone; nothing to publish into.
        };
        cx.update(|cx| {
            scan_entity.update(cx, |this, cx| {
                let lines = view::build_lines(found.lines);
                this.widest_chars = view::widest_chars(&lines);
                this.state = TradeLogState::Ready {
                    lines,
                    truncated: found.truncated,
                };
                cx.notify();
            });
        });
    });
    entity.update(cx, |this, _| this._scan = Some(scan));
    view::open_dialog(entity, window, cx);
}
