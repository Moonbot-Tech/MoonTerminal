//! Backend notification batching and Main request publication.

use super::ChartHistoryScope;
use crate::Backend;
use gpui::Context;
use moon_core::session::CoreId;
use std::time::Duration;
use std::time::Instant;

impl Backend {
    pub(crate) fn mark_backend_dirty(&mut self, cx: &mut Context<Self>) {
        self.backend_dirty_since_notify = true;
        self.flush_backend_notify(cx);
    }

    pub(crate) fn flush_backend_notify(&mut self, cx: &mut Context<Self>) {
        if !self.backend_dirty_since_notify {
            return;
        }
        let due = self
            .last_backend_notify
            .is_none_or(|last| last.elapsed() >= Duration::from_millis(250));
        if !due {
            return;
        }
        self.backend_dirty_since_notify = false;
        self.last_backend_notify = Some(Instant::now());
        crate::diag::bump(&crate::diag::BACKEND_NOTIFY);
        cx.notify();
    }

    /// Queue the first configured market for the render diagnostic once its owner is available.
    ///
    /// Args:
    ///     cx: Backend context used to notify diagnostic observers after the request is queued.
    ///
    /// Returns:
    ///     Nothing. With no group window it remains pending; once a window exists it either queues
    ///     the first eligible market or finishes with a diagnostic warning when none exists.
    pub(crate) fn maybe_diag_open_first_market(&mut self, cx: &mut Context<Self>) {
        if !self.diag_open_first_market
            || self.diag_open_done
            || self.open_main_request.is_pending()
        {
            return;
        }
        if self.group_windows.is_empty() {
            return;
        }

        let candidate = self.config.servers.iter().find_map(|server| {
            let market = server.market.trim();
            (self
                .workspace_core_availability(&server.group, server.id)
                .is_available()
                && !market.is_empty()
                && self.group_windows.contains_key(&server.group))
            .then(|| (server.id, market.to_string()))
        });

        let Some((core, market)) = candidate else {
            self.diag_open_done = true;
            log::warn!("diag auto-open: no available server with default market");
            return;
        };

        self.diag_open_done = true;
        self.open_on_main((core, market.clone()), false);
        if std::env::var_os("MOON_RENDER_DIAG_PAUSE_AFTER_OPEN").is_some() {
            self.follow = false;
            self.follow_persistent = true;
        }
        log::info!(
            "diag auto-open: core={} market={market}",
            moon_core::feed::core_label(core)
        );
        cx.notify();
    }

    /// Request opening `target` on its group's Main chart as one atomic identity.
    ///
    /// `activate` raises and focuses the Main window: the chart double-click and the Alerts coin
    /// click pass `true`, while table, detect, log, and screener navigation pass `false` to open
    /// the market without stealing focus from the current window.
    ///
    /// Args:
    ///     target: Live core and canonical market to address atomically.
    ///     activate: Whether ChartTabs should raise the owning group window after opening.
    ///
    /// Returns:
    ///     Nothing; a target without a live session is ignored.
    pub(crate) fn open_on_main(&mut self, target: (CoreId, String), activate: bool) {
        self.queue_open_on_main(target, ChartHistoryScope::Default, None, activate);
    }

    /// Resolve and queue one Main request with immutable optional producer authority.
    pub(super) fn queue_open_on_main(
        &mut self,
        target: (CoreId, String),
        history: ChartHistoryScope,
        authority_group: Option<String>,
        activate: bool,
    ) {
        let Some(group) = self
            .session
            .sessions()
            .iter()
            .find(|session| session.id == target.0)
            .map(|session| session.group.clone())
        else {
            return;
        };
        self.open_main_request
            .request(target, history, group, authority_group, activate);
    }

    #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
    pub(crate) fn take_diag_open_10_btc(&mut self) -> bool {
        if !self.diag_open_10_btc || self.diag_open_10_btc_done {
            return false;
        }
        // Debug perf windows only need a live core id/group, not the main group window.
        // On headless Linux/X11 the main window can exist while the bookkeeping gate is
        // still false during early startup, which made MOON_RENDER_DIAG_OPEN_10_BTC
        // silently do nothing and broke automated perf runs.
        if self.session.sessions().is_empty() {
            return false;
        }
        if crate::diagnostics::debug_window::debug_chart_target(self).is_none() {
            return false;
        }
        self.diag_open_10_btc_done = true;
        true
    }
}
