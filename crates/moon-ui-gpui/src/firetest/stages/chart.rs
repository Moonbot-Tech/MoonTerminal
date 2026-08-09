//! Stage `open_chart`: put a real live chart on screen for every later stage to measure.
//!
//! The chart is opened through the same atomic `open_on_main` path the UI uses, on the first core
//! whose server, group and window are all actually active — not by constructing a chart directly.
//! A run that opened a synthetic chart would measure an empty own-pass and pass for the wrong reason.

use gpui::Context;

use crate::Backend;

use crate::firetest::Runtime;
use crate::firetest::logging::firetest_info;
use crate::firetest::plan::StageStep;

/// Stage `open_chart`: keep asking until a core is ready, or until the stage times out.
pub(in crate::firetest) fn open_chart(
    runtime: &mut Runtime,
    backend: &mut Backend,
    cx: &mut Context<Backend>,
) -> StageStep {
    if runtime.try_open_chart(backend, cx) {
        StageStep::Next
    } else {
        runtime.wait_log("waiting for active visible core/window");
        StageStep::Stay
    }
}

/// Stage `wait_chart_probe`: hold until the chart reports the bounds the storm will aim at.
pub(in crate::firetest) fn wait_probe(
    runtime: &mut Runtime,
    _backend: &mut Backend,
    _cx: &mut Context<Backend>,
) -> StageStep {
    if runtime.probe.is_some() {
        StageStep::Next
    } else {
        runtime.wait_log("waiting for chart bounds probe");
        StageStep::Stay
    }
}

impl Runtime {
    /// Request the configured market on the first eligible core, returning whether the request was
    /// placed. `false` means no active visible core/window is ready yet and the caller should keep
    /// waiting until its timeout.
    pub(in crate::firetest) fn try_open_chart(
        &mut self,
        backend: &mut Backend,
        cx: &mut Context<Backend>,
    ) -> bool {
        if backend.open_main_request.is_pending() || backend.group_windows.is_empty() {
            return false;
        }

        let target_market = self.config.market.trim();
        let candidate = backend.config.servers.iter().find_map(|server| {
            backend
                .workspace_core_availability(&server.group, server.id)
                .is_available()
                .then(|| (server.id, server.group.clone(), server.name.clone()))
        });
        let Some((core, group, name)) = candidate else {
            return false;
        };

        let market = target_market.to_string();
        backend.open_on_main((core, market.clone()), false);
        backend.follow = true;
        self.opened_group = Some(group.clone());
        firetest_info(&format!(
            "[firetest] open chart: core={core} group={group} name={name} market={market}"
        ));
        cx.notify();
        true
    }
}
