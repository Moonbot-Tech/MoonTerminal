//! Stage `order_cancel_lag`: place a real limit order below the market, cancel it, and measure how
//! long the chart keeps showing the old state.
//!
//! Opt-in, because it sends real trading commands. What it measures is the whole chain
//! `cancel_order` → incoming orders/server log → `OrderLineStore` → chart userdata → GPU prepare →
//! chart present, and it goes red when the cancellation reached the store but the picture lagged
//! behind it. The order is priced well below the market so the test measures display, not a fill.

use std::collections::HashSet;

use moon_core::feed::CoreLogLine;
use moon_core::market::MarketQuantityUnit;
use moon_core::session::order_lines::OrderCloseReason;
use moon_core::util::now_unix_ms_i64;

use crate::Backend;

use gpui::Context;

use crate::firetest::Runtime;
use crate::firetest::logging::firetest_info;
use crate::firetest::plan::StageStep;

/// Stage `order_cancel_lag`.
pub(in crate::firetest) fn run(
    runtime: &mut Runtime,
    backend: &mut Backend,
    _cx: &mut Context<Backend>,
) -> StageStep {
    match runtime.tick_order_cancel_lag(backend) {
        Ok(true) => StageStep::Next,
        Ok(false) => StageStep::Stay,
        Err(error) => StageStep::Fail(error),
    }
}

/// Where the scenario currently is: waiting for the placed order, or for the cancelled one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OrderCancelStep {
    WaitOrder,
    WaitClosed,
}

/// The in-flight order-latency run: what was placed, and every timestamp collected so far.
pub(in crate::firetest) struct OrderCancelRun {
    core: u64,
    market: String,
    before_uids: HashSet<u64>,
    price: f64,
    size: f64,
    place_submit_ms: i64,
    uid: Option<u64>,
    order_seen_ms: Option<i64>,
    cancel_submit_ms: Option<i64>,
    closed_store_ms: Option<i64>,
    closed_order_lines_rev: Option<u64>,
    closed_reason: Option<OrderCloseReason>,
    server_log: Option<CoreLogLine>,
    step: OrderCancelStep,
}

impl Runtime {
    /// Place the test order and capture the pre-existing order uids to tell it apart from the rest.
    fn start_order_cancel_lag(&self, backend: &Backend) -> Result<OrderCancelRun, String> {
        let group = self
            .opened_group
            .as_ref()
            .ok_or_else(|| "order_cancel_lag has no opened chart group".to_string())?;
        let (core, market) = backend.main_chart_target(group).ok_or_else(|| {
            format!("order_cancel_lag has no main chart target for group={group}")
        })?;
        let latest_price = backend
            .session
            .market_source()
            .latest_price(core, &market)
            .map_err(|reason| {
                format!("order_cancel_lag has no live-correct latest price for {market}: {reason}")
            })?;
        let price = (latest_price as f64 * self.config.order_cancel_price_mult).max(1e-8);
        // `order_cancel_quote_size` is a QUOTE amount, and what one unit of size means depends on
        // the market: dividing by price gives a coin quantity, which is right on a linear market
        // and wrong on a coin-margined one, where the exchange counts contracts of fixed value.
        // This script places a REAL order, so it asks the market rather than assuming.
        let quote_size = self.config.order_cancel_quote_size;
        let size_override = match self.config.order_cancel_size {
            Some(size) => Some(size),
            None => match quote_size {
                None => None,
                Some(quote) => {
                    match backend
                        .session
                        .market_source()
                        .order_size_rules(core, &market)
                        .map(|rules| rules.unit)
                    {
                        // Both units take a COIN amount on the wire; see `manual_order_size_base`.
                        Some(MarketQuantityUnit::Contracts(_) | MarketQuantityUnit::Coins) => {
                            Some(quote / price)
                        }
                        None => {
                            return Err(format!(
                                "order_cancel_lag cannot size {market}: its quantity unit has not \
                                 been reported yet"
                            ));
                        }
                    }
                }
            },
        };
        let terms = backend
            .manual_order_terms(core, &market, price, size_override)
            .ok_or_else(|| {
                format!(
                    "order_cancel_lag core={core} has no complete local terms or valid order size"
                )
            })?;
        let size = terms.size_base;
        if !(size.is_finite() && size > 0.0) {
            return Err(format!("order_cancel_lag invalid order size {size}"));
        }
        let before_uids: HashSet<u64> = backend
            .session
            .store()
            .core(core)
            .map(|core| core.orders.iter().map(|order| order.uid).collect())
            .unwrap_or_default();
        let feed_log_enabled = backend
            .config
            .servers
            .iter()
            .find(|server| server.id == core)
            .is_some_and(|server| server.feed.log);
        if !feed_log_enabled {
            firetest_info(&format!(
                "[firetest] order_cancel_lag warning core={core} feed.log=false server_log_metrics=missing"
            ));
        }
        let place_submit_ms = now_unix_ms_i64();
        backend
            .session
            .place_order(core, market.clone(), false, price, size, None, terms.exit)
            .map_err(|error| format!("order_cancel_lag place order failed: {error:#}"))?;
        firetest_info(&format!(
            "[firetest] order_cancel_lag place core={core} market={market} price={price:.8} size={size:.8} quote_size={} latest_price={latest_price:.8}",
            opt_f64(self.config.order_cancel_quote_size)
        ));
        Ok(OrderCancelRun {
            core,
            market,
            before_uids,
            price,
            size,
            place_submit_ms,
            uid: None,
            order_seen_ms: None,
            cancel_submit_ms: None,
            closed_store_ms: None,
            closed_order_lines_rev: None,
            closed_reason: None,
            server_log: None,
            step: OrderCancelStep::WaitOrder,
        })
    }

    /// Advances the opt-in order-cancel latency scenario.
    ///
    /// Returns `Ok(true)` when the scenario is disabled or complete, `Ok(false)` while it
    /// is waiting for state, and an error when setup, cancellation, rendering, or latency
    /// validation fails.
    pub(in crate::firetest) fn tick_order_cancel_lag(
        &mut self,
        backend: &mut Backend,
    ) -> Result<bool, String> {
        if !self.config.order_cancel_lag {
            firetest_info(
                "[firetest] order_cancel_lag skipped (set MOON_FIRETEST_ORDER_CANCEL=1 to enable real order test)",
            );
            return Ok(true);
        }

        let mut run = match self.order_cancel.take() {
            Some(run) => run,
            None => {
                self.order_cancel = Some(self.start_order_cancel_lag(backend)?);
                return Ok(false);
            }
        };
        match run.step {
            OrderCancelStep::WaitOrder => {
                let Some(core) = backend.session.store().core(run.core) else {
                    return Err(format!("order_cancel_lag core={} disappeared", run.core));
                };
                let found = core
                    .orders
                    .iter()
                    .filter(|order| {
                        order.market == run.market
                            && !run.before_uids.contains(&order.uid)
                            && !order.is_short
                            && !order.job_is_done
                            && (order.buy_price - run.price).abs()
                                <= run.price.abs().mul_add(0.03, 1e-8)
                    })
                    .max_by_key(|order| order.uid)
                    .map(|order| order.uid);
                let Some(uid) = found else {
                    self.wait_log("order_cancel_lag waiting for placed order snapshot");
                    self.order_cancel = Some(run);
                    return Ok(false);
                };
                let now = now_unix_ms_i64();
                backend
                    .session
                    .cancel_order(run.core, uid)
                    .map_err(|error| {
                        format!("order_cancel_lag cancel order {uid} failed: {error:#}")
                    })?;
                run.uid = Some(uid);
                run.order_seen_ms = Some(now);
                run.cancel_submit_ms = Some(now_unix_ms_i64());
                run.step = OrderCancelStep::WaitClosed;
                firetest_info(&format!(
                    "[firetest] order_cancel_lag cancel uid={uid} place_to_seen_ms={} core={} market={}",
                    now - run.place_submit_ms,
                    run.core,
                    run.market
                ));
            }
            OrderCancelStep::WaitClosed => {
                let uid = run
                    .uid
                    .ok_or_else(|| "order_cancel_lag waiting closed without uid".to_string())?;
                let Some(core) = backend.session.store().core(run.core) else {
                    return Err(format!("order_cancel_lag core={} disappeared", run.core));
                };
                if run.server_log.is_none() {
                    run.server_log = find_order_cancel_log(
                        core.raw_server_log_snapshot(300),
                        uid,
                        run.cancel_submit_ms.unwrap_or_default(),
                    );
                }
                if run.closed_store_ms.is_none() {
                    if let Some(state) = core.order_lines.order_state(uid) {
                        if let (Some(closed_store_ms), Some(closed_rev)) =
                            (state.closed_store_ms, state.closed_rev)
                        {
                            let closed_ms_i64 = closed_store_ms.round() as i64;
                            run.closed_store_ms = Some(closed_ms_i64);
                            run.closed_order_lines_rev = Some(closed_rev);
                            run.closed_reason = state.closed_reason;
                            firetest_info(&format!(
                                "[firetest] order_cancel_lag closed uid={uid} order_lines_rev={} reason={:?} cancel_to_order_lines_ms={}",
                                closed_rev,
                                state.closed_reason,
                                closed_ms_i64 - run.cancel_submit_ms.unwrap_or(closed_ms_i64)
                            ));
                        }
                    }
                }
                let Some(closed_rev) = run.closed_order_lines_rev else {
                    self.wait_log("order_cancel_lag waiting for cancelled order snapshot");
                    self.order_cancel = Some(run);
                    return Ok(false);
                };
                let Some(group) = self.opened_group.as_ref() else {
                    return Err("order_cancel_lag has no opened group".into());
                };
                #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
                let probe = backend
                    .debug_main_chart_handles
                    .get(group)
                    .and_then(|chart| chart.order_render_probe(run.core, &run.market));
                #[cfg(not(any(debug_assertions, moon_profile_debug, feature = "debug-tools")))]
                let probe: Option<crate::chartdx::OrderRenderProbe> = {
                    let _ = (backend, group);
                    None
                };
                let Some(probe) = probe else {
                    self.wait_log("order_cancel_lag waiting for chart order render probe");
                    self.order_cancel = Some(run);
                    return Ok(false);
                };
                if probe.gpu_rev != closed_rev {
                    self.wait_log("order_cancel_lag waiting for chart GPU userdata revision");
                    self.order_cancel = Some(run);
                    return Ok(false);
                }
                if probe.present_rev != closed_rev {
                    self.wait_log(
                        "order_cancel_lag waiting for chart present after order revision",
                    );
                    self.order_cancel = Some(run);
                    return Ok(false);
                }
                if run.closed_reason != Some(OrderCloseReason::Cancel) {
                    return Err(format!(
                        "order_cancel_lag uid={uid} closed with {:?}, expected explicit Cancel",
                        run.closed_reason
                    ));
                }

                let closed_store_ms = run.closed_store_ms.unwrap_or_default() as f64;
                let display_lag_ms = (probe.present_ms - closed_store_ms).max(0.0);
                let sync_to_gpu_ms = (probe.gpu_ms - probe.order_lines_sync_ms).max(0.0);
                let gpu_to_present_ms = (probe.present_ms - probe.gpu_ms).max(0.0);
                let cancel_to_chart_ms =
                    (probe.present_ms - run.cancel_submit_ms.unwrap_or_default() as f64).max(0.0);
                let server_log = run.server_log.as_ref();
                let server_to_recv_ms = server_log.map(|line| line.recv_ms - line.time_ms);
                let log_recv_to_chart_ms =
                    server_log.map(|line| (probe.gpu_ms - line.recv_ms as f64).max(0.0));
                firetest_info(&format!(
                    "[firetest] order_cancel_lag result uid={uid} core={} market={} price={:.8} size={} closed_order_lines_rev={} probe_order_lines_rev={} display_lag_ms={display_lag_ms:.1} sync_to_gpu_ms={sync_to_gpu_ms:.1} gpu_to_present_ms={gpu_to_present_ms:.1} cancel_to_visible_ms={cancel_to_chart_ms:.1} server_to_recv_ms={} log_recv_to_chart_ms={} server_log={}",
                    run.core,
                    run.market,
                    run.price,
                    run.size,
                    closed_rev,
                    probe.order_lines_rev,
                    opt_i64(server_to_recv_ms),
                    opt_f64(log_recv_to_chart_ms),
                    server_log
                        .map(|line| crate::display_text::flatten_lines(&line.msg))
                        .unwrap_or_else(|| "missing".to_string())
                ));
                if display_lag_ms > self.config.order_cancel_max_display_lag_ms {
                    return Err(format!(
                        "order_cancel_lag display_lag_ms {display_lag_ms:.1} > {:.1}",
                        self.config.order_cancel_max_display_lag_ms
                    ));
                }
                return Ok(true);
            }
        }
        self.order_cancel = Some(run);
        Ok(false)
    }
}

/// The most recent server log line that plausibly reports this cancellation.
///
/// Matched by uid or by a cancel wording in either language, and bounded to lines received around
/// the cancel submission so an older cancellation of another order cannot be picked up.
fn find_order_cancel_log(lines: Vec<CoreLogLine>, uid: u64, since_ms: i64) -> Option<CoreLogLine> {
    let uid_text = uid.to_string();
    lines.into_iter().rev().find(|line| {
        if line.recv_ms < since_ms.saturating_sub(500) {
            return false;
        }
        let msg = line.msg.to_ascii_lowercase();
        line.msg.contains(&uid_text) || msg.contains("cancel") || msg.contains("отмен")
    })
}

/// Format an optional integer for this stage's log line, with an explicit `NA` for absent.
fn opt_i64(value: Option<i64>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "NA".to_string())
}

/// Format an optional float for this stage's log line, with an explicit `NA` for absent.
fn opt_f64(value: Option<f64>) -> String {
    value
        .map(|value| format!("{value:.1}"))
        .unwrap_or_else(|| "NA".to_string())
}
