//! Native application event regressions, overlapped with the existing recovery and cooldown stages.

use super::logging::firetest_info;
use crate::Backend;
use gpui::{AppContext, Context};
use moon_ui::Panel;
use std::{cell::RefCell, rc::Rc, time::Duration};

/// Completion results survive until the common FireTest verdict.
#[derive(Clone, Default)]
pub(super) struct Probe(Rc<RefCell<Vec<Result<(), String>>>>);

impl Probe {
    /// Retain each independent failure rather than letting later callbacks overwrite it.
    fn record(&self, result: Result<(), String>) {
        self.0.borrow_mut().push(result);
    }

    /// A missing callback is a failure, including a task that silently stopped.
    pub(super) fn verify(&self) -> Result<(), String> {
        let results = self.0.borrow();
        let errors: Vec<_> = results
            .iter()
            .filter_map(|result| result.as_ref().err().cloned())
            .collect();
        if !errors.is_empty() {
            return Err(errors.join("; "));
        }
        if results.len() != 3 {
            return Err(format!("application probes completed {}/3", results.len()));
        }
        firetest_info(
            "[firetest] application events: hidden status catch-up, market-only screener, foreign Report scope PASS",
        );
        Ok(())
    }
}

/// Exercise real panel entities without adding another test-only visible window.
pub(super) fn start(backend: &Backend, group: String, cx: &mut Context<Backend>) -> Probe {
    let probe = Probe::default();
    let result = probe.clone();
    let window = backend.assets_window;
    let backend = cx.entity();
    cx.defer(move |cx| {
        let Some(window) = window else {
            result.record(Err("application probes require the Assets window".into()));
            return;
        };
        let panels = window.update(cx, |_, window, cx| {
            let status = cx.new(|cx| {
                crate::panels::CoreStatusView::restored_group(
                    backend.clone(),
                    group.clone(),
                    window,
                    cx,
                )
            });
            status.update(cx, |status, cx| status.set_active(false, window, cx));
            let before_status = status.read(cx).firetest_rows_generation();
            let screener =
                cx.new(|cx| crate::screener::ScreenerView::new(backend.clone(), window, cx));
            let before_screener = screener.read(cx).firetest_prepare_sequence();
            let report =
                cx.new(|cx| crate::panels::ReportPanel::new(backend.clone(), group, window, cx));
            (status, before_status, screener, before_screener, report)
        });
        let Ok((status, before_status, screener, before_screener, report)) = panels else {
            result.record(Err("application probe window was closed".into()));
            return;
        };
        // Only the market channel is notified: a generic Backend heartbeat must not rescue it.
        let market_revision = backend.read(cx).market_data_revision();
        for _ in 0..8 {
            market_revision.update(cx, |_, cx| cx.notify());
        }
        cx.spawn(async move |cx| {
            cx.background_executor()
                .timer(Duration::from_millis(1100))
                .await;
            cx.update(|cx| {
                let status_result = window
                    .update(cx, |_, window, cx| {
                        if status.read(cx).firetest_rows_generation() != before_status {
                            return Err(
                                "hidden Core Status rebuilt its table on sampled ticks".into()
                            );
                        }
                        status.update(cx, |status, cx| status.set_active(true, window, cx));
                        if status.read(cx).firetest_rows_generation() <= before_status {
                            return Err("Core Status did not catch up on activation".into());
                        }
                        Ok(())
                    })
                    .unwrap_or_else(|error| Err(error.to_string()));
                result.record(status_result);
                let refreshes = screener.read(cx).firetest_prepare_sequence() - before_screener;
                result.record(if refreshes == 1 {
                    Ok(())
                } else {
                    Err(format!(
                        "market-only Screener burst produced {refreshes} refreshes, expected one"
                    ))
                });
                crate::panels::ReportPanel::firetest_foreign_workspace(report, cx, move |check| {
                    result.record(check)
                });
            });
        })
        .detach();
    });
    probe
}
