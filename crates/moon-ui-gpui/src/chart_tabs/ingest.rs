//! `ChartTabs` ingestion of AddToChart detects, including creating and populating core tabs, plus
//! active-stack lookup by `(num, bucket)`. Extracted from `mod.rs`.
//!
//! The same pass feeds Main's own detect switch: with it on, every row the group's Detects feed
//! takes in — its own cut and its own gates, `panels::detect_newest_per_market` and
//! `panels::detect_feed_admits` — also opens its chart in Main's stack. One pass and one cursor for
//! both routes, so a row is never judged by two different "is it new" rules.
//!
//! What Main does NOT mirror is the feed's replay: ticking "show AddToChart detects" makes the feed
//! re-read its ring, while Main opens only what arrives from then on. A settings tick that opened a
//! screenful of charts for detects that fired minutes ago would be the switch misbehaving.

use gpui::*;

use super::{AddChartStack, ChartTabs};
use moon_core::config::ChartBucket;
use moon_core::session::CoreId;

/// How long a detect's chart lives on Main.
///
/// A strategy's `KeepInChart`, where a strategy produced the row — `0` is its way of saying
/// "forever". A row NO strategy produced (a drawn figure's alert firing; `strat_name` is empty for
/// those alone) carries a zero that nobody chose, and reading it as "forever" would leave a chart
/// up for good on the strength of a missing value: it takes its feed card's lifetime instead.
///
/// Args:
///     det: The detect row.
///     card_ttl_ms: Lifetime of the row's card in the Detects feed.
///
/// Returns:
///     The chart's lifetime in milliseconds, infinite for a strategy's `KeepInChart = 0`.
fn main_detect_ttl_ms(det: &moon_core::feed::DetectRow, card_ttl_ms: f64) -> f64 {
    if det.strat_name.is_empty() {
        card_ttl_ms
    } else {
        det.keep_in_chart_ttl_ms()
    }
}

impl ChartTabs {
    /// Ingest AddToChart detects with `add_to_chart > 0` by creating or populating a tab, and hand
    /// Main the rows its detect switch asks for.
    ///
    /// The tab key is the core's `ChartBucket` (per-core, shared, or named bundle), resolved from
    /// core config and global `charts_split_by_core`. This does not switch `active`, preserving the
    /// behavior that a detect must not pull the user to a chart — and Main keeps to the same rule
    /// inside its own stack (`MainChartStack::add_detect_coin`).
    ///
    /// Args:
    ///     cx: Parent context used to read Backend detects and create or update stacks.
    ///
    /// Returns:
    ///     Nothing; new detects and stack observers are incorporated in place.
    pub(super) fn ingest(&mut self, cx: &mut Context<Self>) {
        // Read before the backend borrow below: Main is a sibling entity, not part of it.
        let main_on = self.main.read(cx).main_detects_on();
        #[allow(clippy::type_complexity)]
        let (split, fresh, main_fresh, cursors): (
            bool,
            Vec<(u32, CoreId, ChartBucket, String, f64)>,
            Vec<(CoreId, String, f64)>,
            Vec<(CoreId, u64)>,
        ) = {
            let b = self.backend.read(cx);
            let split = b.config.charts_split_by_core;
            let mut fresh = Vec::new();
            let mut main_fresh = Vec::new();
            let mut cursors = Vec::new();
            // Main's route asks the FEED's question, with the feed's own inputs, so the switch
            // opens what the Detects panel shows and nothing else. Resolved once per pass, and only
            // when the switch is on: an off switch must cost this path nothing.
            let feed = main_on.then(|| {
                (
                    b.detects_view.shows_add_to_chart(&self.group),
                    b.alert_duration_s(),
                    moon_chart::paint::now_unix_ms(),
                )
            });
            for s in b
                .session
                .sessions()
                .iter()
                .filter(|s| s.group == self.group)
                .filter(|s| b.core_displayed_in_group(&self.group, s.id))
            {
                let id = s.id;
                let Some(d) = b.session.store().core(id) else {
                    continue;
                };
                // Resolve the core bucket from its configured bundle and global split setting.
                // A core without config gets its own tab.
                let bucket = b
                    .config
                    .servers
                    .iter()
                    .find(|sv| sv.id == id)
                    .map(|sv| sv.chart_bucket(split))
                    .unwrap_or(ChartBucket::Core(id));
                let last = self.add_seq.get(&id).copied().unwrap_or(0);
                let mut mx = last;
                if let Some((show_add_to_chart, figure_duration_s, now_ms)) = feed {
                    // The feed's own cut first, then its gates: only the newest row of a market
                    // is judged. Walked back to arrival order, so the stalest lands first.
                    let newest = crate::panels::detect_newest_per_market(
                        d.detects.iter().map(|det| &**det),
                        last,
                    );
                    for det in newest.into_iter().rev() {
                        if let Some(card_ttl) = crate::panels::detect_feed_admits(
                            det,
                            show_add_to_chart,
                            figure_duration_s,
                            now_ms,
                        ) {
                            main_fresh.push((
                                id,
                                det.market.clone(),
                                main_detect_ttl_ms(det, card_ttl),
                            ));
                        }
                    }
                }
                for det in &d.detects {
                    if det.seq <= last {
                        continue;
                    }
                    mx = mx.max(det.seq);
                    if det.add_to_chart > 0 {
                        // `KeepInChart = 0` means Moonbot holds the chart INDEFINITELY, so the
                        // TTL is infinite: `prune_ttl` never takes that pane and no close timer
                        // is armed for it.
                        let ttl = det.keep_in_chart_ttl_ms();
                        fresh.push((
                            det.add_to_chart,
                            id,
                            bucket.clone(),
                            det.market.clone(),
                            ttl,
                        ));
                    }
                }
                if mx != last {
                    cursors.push((id, mx));
                }
            }
            (split, fresh, main_fresh, cursors)
        };
        for (id, mx) in cursors {
            self.add_seq.insert(id, mx);
        }
        if !main_fresh.is_empty() {
            moon_core::detect_diag::line(&format!(
                "[ingest] group={} main_fresh={}",
                self.group,
                main_fresh.len()
            ));
            self.main.update(cx, |main, mcx| {
                for (core, market, ttl) in main_fresh {
                    main.add_detect_coin(core, &market, ttl, mcx);
                }
            });
        }
        if fresh.is_empty() {
            return;
        }
        // Detect diagnostics: AddToChart detects reached this group's UI. `fresh` is the number of
        // new AddToChart events processed in this pass. `channels.detect` enables this and is off
        // by default.
        moon_core::detect_diag::line(&format!(
            "[ingest] group={} split={split} fresh={} existing_tabs={}",
            self.group,
            fresh.len(),
            self.add.len()
        ));
        let (epoch, theme, backend, workspace_group) = (
            self.epoch,
            self.theme.clone(),
            self.backend.clone(),
            self.group.clone(),
        );
        for (n, core, bucket, market, ttl) in fresh {
            let in_detached = self
                .detached
                .iter()
                .any(|(num, c, _)| *num == n && *c == bucket);
            if let Some((_, _, tab)) = self
                .add
                .iter()
                .find(|(num, c, _)| *num == n && *c == bucket)
                .or_else(|| {
                    self.detached
                        .iter()
                        .find(|(num, c, _)| *num == n && *c == bucket)
                })
            {
                if in_detached {
                    moon_core::detect_diag::line(&format!(
                        "[ingest] +coin n={n} bucket={bucket:?} market={market} → DETACHED-окно"
                    ));
                }
                tab.update(cx, |p, pcx| p.add_detect_coin(core, &market, ttl, pcx));
            } else {
                let panel = cx.new(|_| {
                    AddChartStack::new(
                        backend.clone(),
                        workspace_group.clone(),
                        n,
                        bucket.clone(),
                        epoch,
                        theme.clone(),
                    )
                });
                // Restore this tab's saved per-tab display and comparison settings from charts.json.
                let (
                    saved_scale,
                    saved_layout,
                    saved_orderbook,
                    saved_show_zone,
                    saved_auto_pin,
                    saved_orientation,
                    saved_arrival_flash,
                    saved_arrival_frame,
                    saved_grid,
                    saved_max_charts,
                    saved_axis_pos,
                    saved_time_axis,
                    saved_line_labels,
                    saved_cursor_labels,
                    saved_candle_view,
                    saved_chart_graphics,
                    saved_chart_labels,
                    saved_compare,
                ) = {
                    let specs = &self.backend.read(cx).chart_specs;
                    let spec = specs.iter().find(|s| s.matches(&self.group, n, &bucket));
                    (
                        spec.and_then(|s| s.scale),
                        spec.map_or((None, None, None), |s| {
                            (s.layout_mode, s.layout_height_fit, s.layout_height_scroll)
                        }),
                        spec.and_then(|s| s.orderbook_enabled),
                        spec.and_then(|s| s.show_zone),
                        spec.and_then(|s| s.auto_pin),
                        spec.and_then(|s| s.layout_orientation),
                        spec.and_then(|s| s.arrival_flash),
                        spec.map_or((None, None), |s| (s.arrival_core_color, s.arrival_hold)),
                        spec.map_or((None, None, None), |s| {
                            (s.layout_columns, s.layout_columns_exact, s.layout_min_slot)
                        }),
                        spec.map_or((None, None), |s| (s.max_charts, s.max_charts_evict)),
                        spec.and_then(|s| s.price_axis_pos),
                        spec.and_then(|s| s.time_axis_visible),
                        spec.and_then(|s| s.line_labels),
                        spec.and_then(|s| s.cursor_labels),
                        spec.and_then(|s| s.candle_view),
                        spec.and_then(|s| s.chart_graphics),
                        spec.and_then(|s| s.chart_labels.clone()),
                        spec.map_or((None, false), |s| {
                            (s.compare_anchor.clone(), s.compare_orderbook_only)
                        }),
                    )
                };
                if saved_scale.is_some() {
                    panel.update(cx, |p, pcx| p.set_scale(saved_scale, pcx));
                }
                if saved_layout.0.is_some() || saved_layout.1.is_some() || saved_layout.2.is_some()
                {
                    panel.update(cx, |p, pcx| {
                        p.set_layout(saved_layout.0, saved_layout.1, saved_layout.2, pcx)
                    });
                }
                if saved_orderbook.is_some() {
                    panel.update(cx, |p, pcx| p.set_orderbook_enabled(saved_orderbook, pcx));
                }
                if saved_show_zone.is_some() {
                    panel.update(cx, |p, pcx| p.set_show_zone(saved_show_zone, pcx));
                }
                if saved_auto_pin.is_some() {
                    panel.update(cx, |p, pcx| p.set_auto_pin(saved_auto_pin, pcx));
                }
                if saved_orientation.is_some() {
                    panel.update(cx, |p, pcx| p.set_orientation(saved_orientation, pcx));
                }
                if saved_arrival_flash.is_some() {
                    panel.update(cx, |p, pcx| p.set_arrival_flash(saved_arrival_flash, pcx));
                }
                if saved_arrival_frame.0.is_some() || saved_arrival_frame.1.is_some() {
                    panel.update(cx, |p, pcx| {
                        p.set_arrival_frame(saved_arrival_frame.0, saved_arrival_frame.1, pcx)
                    });
                }
                if saved_grid.0.is_some() || saved_grid.1.is_some() || saved_grid.2.is_some() {
                    panel.update(cx, |p, pcx| {
                        p.set_layout_columns(saved_grid.0, saved_grid.1, saved_grid.2, pcx)
                    });
                }
                // Restored BEFORE the first chart goes in, so a tab reopened at its cap does not
                // spend one detect getting there.
                if saved_max_charts.0.is_some() || saved_max_charts.1.is_some() {
                    panel.update(cx, |p, pcx| {
                        p.set_max_charts(saved_max_charts.0, saved_max_charts.1, pcx)
                    });
                }
                if saved_axis_pos.is_some() {
                    panel.update(cx, |p, pcx| p.set_price_axis_pos(saved_axis_pos, pcx));
                }
                if saved_time_axis.is_some() {
                    panel.update(cx, |p, pcx| p.set_time_axis_visible(saved_time_axis, pcx));
                }
                if saved_line_labels.is_some() {
                    panel.update(cx, |p, pcx| p.set_line_labels(saved_line_labels, pcx));
                }
                if saved_cursor_labels.is_some() {
                    panel.update(cx, |p, pcx| p.set_cursor_labels(saved_cursor_labels, pcx));
                }
                if saved_candle_view.is_some() {
                    panel.update(cx, |p, pcx| p.set_candle_view(saved_candle_view, pcx));
                }
                if saved_chart_graphics.is_some() {
                    panel.update(cx, |p, pcx| p.set_chart_graphics(saved_chart_graphics, pcx));
                }
                if saved_chart_labels.is_some() {
                    panel.update(cx, |p, pcx| p.set_chart_labels(saved_chart_labels, pcx));
                }
                if saved_compare.0.is_some() || saved_compare.1 {
                    panel.update(cx, |p, pcx| {
                        p.restore_compare(saved_compare.0.clone(), saved_compare.1, pcx)
                    });
                }
                panel.update(cx, |p, pcx| p.add_detect_coin(core, &market, ttl, pcx));
                self.watch_regular_stack_target(&panel, cx);
                self.add.push((n, bucket.clone(), panel));
                // Order tabs by `(number, bucket)`, matching egui's `sort_by_key` behavior.
                self.add.sort_by_key(|(num, c, _)| (*num, c.clone()));
                moon_core::detect_diag::line(&format!(
                    "[ingest] NEW tab n={n} bucket={bucket:?} (total_tabs={})",
                    self.add.len()
                ));
                // Do not change `active`; a new tab must not pull the user away.
            }
        }
        self.sync_seen_for_active(cx);
        self.persist_scales(cx);
    }

    pub(super) fn add_stack(&self, n: u32, bucket: &ChartBucket) -> Option<Entity<AddChartStack>> {
        self.add
            .iter()
            .chain(self.custom.iter())
            .find(|(num, c, _)| *num == n && c == bucket)
            .map(|(_, _, p)| p.clone())
    }
}
