//! Native application event regressions, overlapped with the existing recovery and cooldown stages.

use super::logging::firetest_info;
use crate::Backend;
use gpui::{AppContext, AsyncApp, Context, Entity};
use moon_ui::Panel;
use std::{
    cell::RefCell,
    rc::Rc,
    time::{Duration, Instant},
};

/// Completion results survive until the common FireTest verdict.
#[derive(Clone, Default)]
pub(super) struct Probe(Rc<RefCell<Vec<Result<(), String>>>>);

impl Probe {
    /// Retain each independent failure rather than letting later callbacks overwrite it.
    fn record(&self, result: Result<(), String>) {
        self.0.borrow_mut().push(result);
    }

    /// Let the common stage await outstanding callbacks, but report an existing failure at once.
    pub(super) fn is_pending(&self) -> bool {
        let results = self.0.borrow();
        results.len() < 4 && results.iter().all(Result::is_ok)
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
        if results.len() != 4 {
            return Err(format!("application probes completed {}/4", results.len()));
        }
        firetest_info(
            "[firetest] application events: hidden status catch-up, market/catalog-only screener, foreign Report scope PASS",
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
        // Core tests cover SDK events -> FeedMsg -> drain with no open charts. This checks the
        // corresponding native consumer: a generic Backend heartbeat must not rescue it.
        let market_revision = backend.read(cx).market_data_revision();
        for _ in 0..8 {
            market_revision.update(cx, |_, cx| cx.notify());
        }
        cx.spawn(async move |cx| {
            wait_for_screener_refresh(&screener, before_screener, cx).await;
            let before_catalog = cx.update(|cx| {
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
                let before_catalog = screener.read(cx).firetest_prepare_sequence();
                let catalog = backend.read(cx).market_catalog_revision();
                catalog.update(cx, |_, cx| cx.notify());
                before_catalog
            });
            wait_for_screener_refresh(&screener, before_catalog, cx).await;
            cx.update(|cx| {
                let refreshes = screener.read(cx).firetest_prepare_sequence() - before_catalog;
                result.record(if refreshes == 1 {
                    Ok(())
                } else {
                    Err(format!(
                        "catalog-only Screener produced {refreshes} refreshes, expected one"
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

/// Wait for the consumer, not elapsed time since notification: GPU recovery can delay the
/// first poll of its one-second timer. Missing delivery still fails the exact-count check.
async fn wait_for_screener_refresh(
    screener: &Entity<crate::screener::ScreenerView>,
    before: u64,
    cx: &mut AsyncApp,
) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while cx.update(|cx| screener.read(cx).firetest_prepare_sequence() == before)
        && Instant::now() < deadline
    {
        cx.background_executor()
            .timer(Duration::from_millis(50))
            .await;
    }
}
