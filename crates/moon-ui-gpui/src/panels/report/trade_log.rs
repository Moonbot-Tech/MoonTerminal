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
//! [`open_trade_log`] revalidates group workspace authority and hands the file scan to a background
//! thread; the dialog itself lives in [`view`].

mod scan;
mod view;

use crate::Backend;
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
    /// Group-owned workspace authority captured when the row menu was built. Standalone Reports
    /// deliberately leave this absent because Analytics owns their explicit scope.
    pub(super) workspace: Option<TradeLogWorkspaceIdentity>,
}

/// Group workspace identity that must still authorize a delayed trade-log scan.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct TradeLogWorkspaceIdentity {
    /// Group whose Report produced the request.
    group: String,
    /// Row core that owns the log files.
    core: moon_core::session::CoreId,
    /// Workspace generation observed while the row menu was built.
    generation: u64,
}

impl TradeLogWorkspaceIdentity {
    /// Capture one group Report's workspace identity.
    ///
    /// Args:
    ///     group: Owning group of the Report panel.
    ///     core: Core recorded by the selected report row.
    ///     generation: Current workspace-authority generation.
    ///
    /// Returns:
    ///     Immutable identity carried through menu delay and background scanning.
    pub(super) fn new(group: String, core: moon_core::session::CoreId, generation: u64) -> Self {
        Self {
            group,
            core,
            generation,
        }
    }

    /// Check the captured generation and selected-core authority against live state.
    ///
    /// Args:
    ///     current_generation: Current workspace-authority generation.
    ///     core_allowed: Whether the live workspace still permits this core.
    ///
    /// Returns:
    ///     `true` only when neither identity component became stale.
    fn is_current(&self, current_generation: u64, core_allowed: bool) -> bool {
        self.generation == current_generation && core_allowed
    }
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
    /// Zone used to build every cached visible and copied clock.
    display_zone: chrono_tz::Tz,
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
///     backend: Shared state containing the selected display zone and its revision.
///     window: Window hosting the dialog.
///     cx: Application context used to spawn the scan and open the dialog.
///
/// Returns:
///     Nothing; stale group requests open no dialog, and a still-current dialog updates when its
///     scan lands. A scope change during the scan prevents the old-core result from publishing.
pub(super) fn open_trade_log(
    request: TradeLogRequest,
    backend: Entity<Backend>,
    window: &mut Window,
    cx: &mut App,
) {
    if !trade_log_request_is_current(&request, backend.read(cx), cx) {
        return;
    }
    let display_time_revision = backend.read(cx).display_time_revision.clone();
    let display_zone =
        crate::chrome::clock::resolved_header_clock_zone(backend.read(cx).header_clock_zone());
    let zone_backend = backend.clone();
    let dialog_request = request.clone();
    let entity = cx.new(move |cx| {
        cx.observe(
            &display_time_revision,
            move |this: &mut TradeLog, _revision, cx| {
                let zone = crate::chrome::clock::resolved_header_clock_zone(
                    zone_backend.read(cx).header_clock_zone(),
                );
                if zone == this.display_zone {
                    return;
                }
                this.display_zone = zone;
                let widest = match &mut this.state {
                    TradeLogState::Ready { lines, .. } => {
                        view::rezone_lines(lines, zone);
                        Some(view::widest_chars(lines))
                    }
                    TradeLogState::Loading => None,
                };
                if let Some(widest) = widest {
                    this.widest_chars = widest;
                }
                cx.notify();
            },
        )
        .detach();
        TradeLog {
            request: dialog_request,
            state: TradeLogState::Loading,
            display_zone,
            selection: RowSelection::default(),
            widest_chars: 0,
            scroll: MoonVirtualListScrollHandle::new(),
            hscroll: ScrollHandle::new(),
            focus: cx.focus_handle(),
            focused_once: false,
            _scan: None,
        }
    });
    // Reading a day of a core's log is tens of megabytes off disk; it never runs on the frame
    // thread. The task is owned by the dialog state, so closing the dialog drops it — which is why
    // the future holds a WEAK handle: a strong one would close the loop (state -> task -> future ->
    // state) and neither would ever be freed.
    let scan_entity = entity.downgrade();
    let completion_backend = backend.clone();
    let completion_request = request.clone();
    let scan = cx.spawn(async move |cx| {
        let executor = cx.update(|cx| cx.background_executor().clone());
        let found = executor
            .spawn(async move { scan::scan_trade_log(&request, MAX_LINES) })
            .await;
        let Some(scan_entity) = scan_entity.upgrade() else {
            return; // The dialog is gone; nothing to publish into.
        };
        cx.update(|cx| {
            if !trade_log_request_is_current(&completion_request, completion_backend.read(cx), cx) {
                return;
            }
            scan_entity.update(cx, |this, cx| {
                let lines = view::build_lines(found.lines, this.display_zone);
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

/// Revalidate a delayed group Report request while leaving standalone Reports explicitly unscoped.
///
/// Args:
///     request: Trade-log request captured from the report row.
///     backend: Live backend containing workspace mode and selected-core authority.
///     cx: Application context used to read the workspace generation entity.
///
/// Returns:
///     `true` for standalone requests or for an unchanged, still-authorized group identity.
fn trade_log_request_is_current(request: &TradeLogRequest, backend: &Backend, cx: &App) -> bool {
    let Some(workspace) = &request.workspace else {
        return true;
    };
    let revision = backend.workspace_revision();
    workspace.is_current(
        revision.read(cx).generation(),
        backend.workspace_action_allows_core(Some(&workspace.group), workspace.core),
    )
}

#[cfg(test)]
mod tests;
