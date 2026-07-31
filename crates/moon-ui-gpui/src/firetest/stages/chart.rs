//! Stage `open_chart`: put a real live chart on screen for every later stage to measure.
//!
//! The chart is opened through the same `open_request` path the UI uses, on the first core whose
//! server, group and window are all actually active — not by constructing a chart directly. A run
//! that opened a synthetic chart would measure an empty own-pass and pass for the wrong reason.

use gpui::Context;

use crate::Backend;

use crate::firetest::Runtime;
use crate::firetest::logging::firetest_info;

impl Runtime {
    /// Request the configured market on the first eligible core, returning whether the request was
    /// placed. `false` means no active visible core/window is ready yet and the caller should keep
    /// waiting until its timeout.
    pub(in crate::firetest) fn try_open_chart(
        &mut self,
        backend: &mut Backend,
        cx: &mut Context<Backend>,
    ) -> bool {
        if backend.open_request.is_some() || backend.group_windows.is_empty() {
            return false;
        }

        let target_market = self.config.market.trim();
        let candidate = backend.config.servers.iter().find_map(|server| {
            let session_exists = backend
                .session
                .sessions()
                .iter()
                .any(|session| session.id == server.id && session.group == server.group);
            (server.active
                && server.show_window
                && backend.config.group(&server.group).active
                && backend.group_windows.contains_key(&server.group)
                && session_exists)
                .then(|| (server.id, server.group.clone(), server.name.clone()))
        });
        let Some((core, group, name)) = candidate else {
            return false;
        };

        let market = target_market.to_string();
        backend.open_request = Some((core, market.clone()));
        backend.open_request_rev = backend.open_request_rev.wrapping_add(1);
        backend.open_request_activate = false;
        backend.follow = true;
        self.opened_group = Some(group.clone());
        firetest_info(&format!(
            "[firetest] open chart: core={core} group={group} name={name} market={market}"
        ));
        cx.notify();
        true
    }
}
