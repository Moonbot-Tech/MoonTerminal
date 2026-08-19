//! `ChartTabs` coin search and custom multi-coin tabs: multi-selection, creation, renaming,
//! persistence, restoration, and focus-based gating of order-book subscriptions.
//! Extracted from `mod.rs`.

use std::time::Duration;

use gpui::*;
use rust_i18n::t;

use super::common::CoinPopupHost;
use super::{AddChartStack, CUSTOM_NUM_BASE, ChartTabs, Tab, coin_search};
use crate::Backend;
use crate::persistence::chart_persist::{StackLayoutMode, StackOrientation};
use moon_core::config::ChartBucket;
use moon_core::session::CoreId;

impl ChartTabs {
    /// The cores this tab's coin field searches. Add tabs search within their bucket; Main and
    /// custom tabs search the whole group in Classic, because a custom tab can collect coins from
    /// different cores, and narrow to the selected core under Auto.
    ///
    /// Args:
    ///     b: Shared backend holding the persisted mode and validated Auto selection.
    ///
    /// Returns:
    ///     The bucket handed to the shared coin-search widget, which is also its cache key.
    fn coin_bucket(&self, b: &Backend) -> Option<ChartBucket> {
        super::coin_search_bucket(
            &self.active,
            super::auto_workspace_chart_core(b, &self.group),
        )
    }

    /// Return matches for the typed query or suggestions for an empty coin field.
    ///
    /// The empty-field branch reads only cached suggestions — the scan that fills that cache runs
    /// when the popup opens, never here. Both branches use the active tab's workspace-aware bucket.
    ///
    /// Args:
    ///     cx: Application context used to read Backend and the suggestion cache.
    ///
    /// Returns:
    ///     Query matches or cached suggestions within the active tab's search scope.
    pub(super) fn coin_results(&self, cx: &App) -> crate::controls::coin_search::CoinResults {
        use crate::controls::coin_search::{CoinResults, suggestions};

        let b = self.backend.read(cx);
        let bucket = self.coin_bucket(b);
        if !self.coin_query.trim().is_empty() {
            return CoinResults::Query(coin_search::search(
                b,
                &self.group,
                bucket.as_ref(),
                &self.coin_query,
            ));
        }
        let (recent, volatile) = suggestions(
            b,
            &self.group,
            bucket.as_ref(),
            b.coin_suggest_markets(&self.group, bucket.as_ref()),
        );
        CoinResults::Suggest { recent, volatile }
    }

    /// Open the coin dropdown, refreshing the suggestion cache the empty-field list reads.
    ///
    /// Both entry points route here — gaining focus, and clicking a field that already has it —
    /// so the expensive rebuild has exactly one home and cannot leak into a render pass.
    ///
    /// Args:
    ///     cx: ChartTabs context used to read the field, refresh suggestions, and repaint.
    ///
    /// Returns:
    ///     Nothing; the popup opens after the active scope's suggestion cache is refreshed.
    pub(super) fn open_coin_popup(&mut self, cx: &mut Context<Self>) {
        // Re-read the field before deciding what to show. Close paths clear the query MIRROR
        // without rewriting the input, so reopening on focus must resync both values or suggestions
        // can appear under text the user can still see in the field.
        self.coin_query = self.coin_input.read(cx).value().to_string();
        // Resolve through the same helper the render path uses: the bucket is the suggestion
        // cache key, so a mismatch here would refresh one entry and read another, leaving the
        // Top 24h section permanently empty.
        let bucket = self.coin_bucket(self.backend.read(cx));
        let group = self.group.clone();
        self.backend
            .update(cx, |b, _| b.refresh_coin_suggest(&group, bucket.as_ref()));
        self.coin_popup_open = true;
        cx.notify();
    }

    /// Open the selected coin on the ACTIVE tab: Main → fullscreen chart; Add/Custom → its stack.
    pub(super) fn open_coin_on_active(
        &mut self,
        core: CoreId,
        market: String,
        cx: &mut Context<Self>,
    ) {
        match self.active.clone() {
            Tab::Main => self.main.update(cx, |m, c| {
                m.open_or_focus(core, market, crate::backend::ChartHistoryScope::Default, c)
            }),
            Tab::Add(..) | Tab::Custom(..) => {
                if let Some(panel) = self.active_stack() {
                    panel.update(cx, |p, c| {
                        p.add_coin(core, &market, coin_search::MANUAL_COIN_TTL_MS, c)
                    });
                }
            }
        }
        // Re-persist tickers after changing a custom tab's composition.
        if self.active_is_custom() {
            self.persist_custom_active(cx);
        }
        self.sync_main_chart_target(cx);
        cx.notify();
    }

    /// Toggle a coin through its dropdown checkbox, accumulating a selection for Open in new tab.
    /// Selection survives query changes, so BTC and ETH can be selected in separate searches.
    pub(super) fn toggle_coin_selected(
        &mut self,
        core: CoreId,
        market: String,
        cx: &mut Context<Self>,
    ) {
        let key = (core, market);
        if !self.coin_selected.remove(&key) {
            self.coin_selected.insert(key);
        }
        cx.notify();
    }

    /// Create a custom tab from selected coins that remain within the active search scope.
    ///
    /// Its charts start pinned in horizontal orientation, focus moves to the new tab, and its
    /// tickers, name, and layout are persisted.
    ///
    /// Args:
    ///     cx: ChartTabs context used to prune the selection, build the tab, and persist it.
    ///
    /// Returns:
    ///     Nothing; an empty in-scope selection leaves the tab set unchanged.
    pub(super) fn open_selected_in_new_tab(&mut self, cx: &mut Context<Self>) {
        // Backstop the scope prune at the moment of use: the accumulated selection outlives any
        // number of rail moves, and a tab must never be built from a core the search no longer
        // covers.
        let scope_core = super::auto_workspace_chart_core(self.backend.read(cx), &self.group);
        super::prune_coin_selection_to_scope(&mut self.coin_selected, scope_core);
        if self.coin_selected.is_empty() {
            return;
        }
        let coins: Vec<(CoreId, String)> = self.coin_selected.iter().cloned().collect();
        let num = self.next_custom_num;
        self.next_custom_num += 1;
        let label = t!("chart.tab.custom", n = num - CUSTOM_NUM_BASE + 1).to_string();
        let bucket = ChartBucket::Shared;
        let stack = cx.new(|_| {
            AddChartStack::new(
                self.backend.clone(),
                self.group.clone(),
                num,
                bucket.clone(),
                self.epoch,
                self.theme.clone(),
            )
        });
        // Custom tabs default to horizontal orientation and do not retain empty slots.
        stack.update(cx, |s, c| {
            s.set_hold_vacated(false);
            s.set_orientation(Some(StackOrientation::Horizontal), c);
        });
        for (core, market) in &coins {
            stack.update(cx, |s, c| {
                s.add_coin(*core, market, coin_search::MANUAL_COIN_TTL_MS, c)
            });
            // The bulk path does not pass through `open_coin_on_active`, so it records its own
            // recents; without this, coins opened here would never appear in the suggestion list.
            self.backend
                .update(cx, |b, _| b.push_recent_coin(*core, market));
        }
        // Pin charts immediately to protect them from TTL closure.
        stack.update(cx, |s, c| s.pin_all(c));
        self.custom.push((num, bucket.clone(), stack.clone()));
        self.custom_labels.insert(num, label.clone());
        self.active = Tab::Custom(num, bucket.clone());
        self.persist_custom(cx, num, &bucket, &coins, &label);
        // Watch composition and re-persist whenever a chart is closed or added.
        self.watch_custom_stack(num, &bucket, &stack, cx);
        // Clear the selection, field, and popup.
        self.coin_selected.clear();
        self.coin_query.clear();
        self.coin_popup_open = false;
        self.sync_active_scale(cx);
        self.sync_inactive_chart_visibility(cx);
        self.refresh_orderbook_gates(cx);
        self.sync_main_chart_target(cx);
        cx.notify();
    }

    /// Handle a detection right-click by opening the coin in a NEW custom comparison tab.
    /// The detection coin is the anchor; the SAME coin (exact market name) is added from other
    /// group cores without duplicate exchanges, deduplicating by the core's market-data provider
    /// as the screener does and taking the first core in session order. The tab receives the coin
    /// name, horizontal orientation (the only orientation supporting comparison), an anchor lock,
    /// and broom mode, so neighbors show only their order books. Right-clicking the same coin again
    /// focuses the existing tab by name instead of creating a duplicate.
    pub(super) fn open_compare_tab(
        &mut self,
        core: CoreId,
        market: String,
        cx: &mut Context<Self>,
    ) {
        // The tab is named after the coin as the CORE names it, so a Hyperliquid spot index does
        // not become a tab called `@156`. The DISPLAY spelling, without a contract tail: a tab is
        // per coin, and its charts may well be several expiries of it.
        let label = self
            .backend
            .read(cx)
            .session
            .market_source()
            .market_label(core, &market)
            .display_coin()
            .to_string();
        // If a tab already has this coin's name, switch to it. Compared through the shared match
        // key so a tab saved under an older spelling still counts as the same coin instead of
        // silently gaining a duplicate.
        let key = moon_core::symbol::coin_match_key(&label);
        if let Some((n, b)) = self
            .custom
            .iter()
            .find(|(n, _, _)| moon_core::symbol::coin_match_key(&self.custom_label(*n)) == key)
            .map(|(n, b, _)| (*n, b.clone()))
        {
            self.active = Tab::Custom(n, b);
            self.sync_active_scale(cx);
            self.sync_inactive_chart_visibility(cx);
            self.refresh_orderbook_gates(cx);
            self.sync_main_chart_target(cx);
            cx.notify();
            return;
        }
        // Collect the same exact market from other group cores, at most one core per exchange; the
        // anchor provider is already taken. Skip cores without a provider (no market snapshot),
        // because their coin availability cannot be checked.
        let coins: Vec<(CoreId, String)> = {
            let b = self.backend.read(cx);
            let ms = b.session.market_source();
            let mut used = std::collections::HashSet::new();
            used.insert(ms.provider_of(core));
            let mut out = vec![(core, market.clone())];
            for s in b
                .session
                .sessions()
                .iter()
                .filter(|s| s.group == self.group)
            {
                if s.id == core {
                    continue;
                }
                let provider = ms.provider_of(s.id);
                if provider.is_none() || used.contains(&provider) {
                    continue;
                }
                if ms
                    .search_markets(s.id, &market, coin_search::COIN_SEARCH_LIMIT)
                    .iter()
                    .any(|m| m == &market)
                {
                    used.insert(provider);
                    out.push((s.id, market.clone()));
                }
            }
            out
        };
        let num = self.next_custom_num;
        self.next_custom_num += 1;
        let bucket = ChartBucket::Shared;
        let stack = cx.new(|_| {
            AddChartStack::new(
                self.backend.clone(),
                self.group.clone(),
                num,
                bucket.clone(),
                self.epoch,
                self.theme.clone(),
            )
        });
        let anchor = (core, market);
        stack.update(cx, |s, c| {
            s.set_hold_vacated(false);
            s.set_orientation(Some(StackOrientation::Horizontal), c);
            // The anchor is added first, so it is already on the left.
            for (core, market) in &coins {
                s.add_coin(*core, market, coin_search::MANUAL_COIN_TTL_MS, c);
            }
            s.pin_all(c);
            // Lock the anchor and enable broom mode so neighbors show only their order books.
            s.restore_compare(Some(anchor.clone()), true, c);
        });
        self.custom.push((num, bucket.clone(), stack.clone()));
        self.custom_labels.insert(num, label.clone());
        self.active = Tab::Custom(num, bucket.clone());
        self.persist_custom(cx, num, &bucket, &coins, &label);
        self.upsert_spec(cx, num, &bucket, move |s| {
            s.compare_anchor = Some(anchor);
            s.compare_orderbook_only = true;
        });
        self.watch_custom_stack(num, &bucket, &stack, cx);
        self.sync_active_scale(cx);
        self.sync_inactive_chart_visibility(cx);
        self.refresh_orderbook_gates(cx);
        self.sync_main_chart_target(cx);
        cx.notify();
    }

    /// Custom-tab label: the user-supplied name or the localized default set label.
    pub(super) fn custom_label(&self, n: u32) -> String {
        self.custom_labels
            .get(&n)
            .cloned()
            .unwrap_or_else(|| t!("chart.tab.custom", n = n - CUSTOM_NUM_BASE + 1).to_string())
    }

    /// Rename the active custom tab from the ⚙ popup's name field and persist the change.
    pub(super) fn rename_active_custom(&mut self, name: String, cx: &mut Context<Self>) {
        let name = name.trim().to_string();
        if name.is_empty() {
            return;
        }
        if let Tab::Custom(n, b) = self.active.clone() {
            self.custom_labels.insert(n, name.clone());
            self.upsert_spec(cx, n, &b, move |s| s.custom_label = Some(name));
            cx.notify();
        }
    }

    /// Write a custom-tab spec (tickers, name, and horizontal orientation) to `charts.json`.
    pub(super) fn persist_custom(
        &self,
        cx: &mut Context<Self>,
        num: u32,
        bucket: &ChartBucket,
        coins: &[(CoreId, String)],
        label: &str,
    ) {
        let coins = coins.to_vec();
        let label = label.to_string();
        self.upsert_spec(cx, num, bucket, move |s| {
            s.custom_coins = Some(coins);
            s.custom_label = Some(label);
            if s.layout_orientation.is_none() {
                s.layout_orientation = Some(StackOrientation::Horizontal);
            }
        });
    }

    /// Remove a custom-tab spec from `charts.json`; closing the tab deletes its saved state.
    pub(super) fn remove_custom_spec(&self, n: u32, cx: &mut Context<Self>) {
        let group = self.group.clone();
        self.backend.update(cx, |b, _| {
            let before = b.chart_specs.len();
            b.chart_specs
                .retain(|s| !(s.group == group && s.num == n && s.custom_coins.is_some()));
            if b.chart_specs.len() != before {
                b.chart_specs_dirty = true;
            }
        });
    }

    /// Observe custom-stack changes and re-persist tickers when its composition changes, updating
    /// `custom_coins` after a chart is closed or added on a saved tab. While the stack is detached
    /// into `self.detached`, `sync_custom_coins` writes nothing because the window host owns it;
    /// after repinning into the strip, this subscription becomes relevant again.
    pub(super) fn watch_custom_stack(
        &self,
        num: u32,
        bucket: &ChartBucket,
        stack: &Entity<AddChartStack>,
        cx: &mut Context<Self>,
    ) {
        let bk = bucket.clone();
        cx.observe(stack, move |this, _stack, cx| {
            this.sync_custom_coins(num, &bk, cx);
            // A lock click may have changed the comparison anchor, so update the group's trading
            // target. Hotkeys and `cancel_buy` address the locked anchor like fullscreen Main.
            this.sync_main_chart_target(cx);
        })
        .detach();
    }

    /// Compare the custom tab's current composition (tickers, comparison anchor, and broom mode)
    /// with saved state, rewriting the spec ONLY after a change. Otherwise the observer callback
    /// would perform a redundant write on every data tick.
    fn sync_custom_coins(&mut self, num: u32, bucket: &ChartBucket, cx: &mut Context<Self>) {
        let Some(stack) = self.add_stack(num, bucket) else {
            return;
        };
        let (coins, anchor, broom) = {
            let s = stack.read(cx);
            (s.coins(cx), s.compare_anchor(), s.compare_orderbook_only())
        };
        let changed = {
            let specs = &self.backend.read(cx).chart_specs;
            specs
                .iter()
                .find(|s| s.matches(&self.group, num, bucket))
                .map_or(true, |s| {
                    s.custom_coins.as_deref() != Some(coins.as_slice())
                        || s.compare_anchor != anchor
                        || s.compare_orderbook_only != broom
                })
        };
        if changed {
            let label = self.custom_label(num);
            self.persist_custom(cx, num, bucket, &coins, &label);
            self.upsert_spec(cx, num, bucket, move |s| {
                s.compare_anchor = anchor;
                s.compare_orderbook_only = broom;
            });
        }
    }

    /// Re-persist the active custom tab's tickers after its composition changes.
    pub(super) fn persist_custom_active(&mut self, cx: &mut Context<Self>) {
        if let Tab::Custom(n, b) = self.active.clone() {
            if let Some(stack) = self.add_stack(n, &b) {
                let coins = stack.read(cx).coins(cx);
                let label = self.custom_label(n);
                self.persist_custom(cx, n, &b, &coins, &label);
            }
        }
    }

    /// Restore custom tabs from `charts.json` specs containing `custom_coins`: create each stack,
    /// load and pin its tickers, and apply its layout, orientation, scale, and name. Restore into
    /// the strip rather than a window.
    pub(super) fn restore_custom_tabs(&mut self, cx: &mut Context<Self>) {
        #[allow(clippy::type_complexity)]
        let specs: Vec<(
            u32,
            ChartBucket,
            Vec<(CoreId, String)>,
            Option<String>,
            Option<f32>,
            (Option<StackLayoutMode>, Option<u16>, Option<u16>),
            Option<StackOrientation>,
            Option<bool>,
            Option<bool>,
            Option<bool>,
            Option<bool>,
            Option<(CoreId, String)>,
            bool,
            Option<crate::persistence::chart_persist::PriceAxisPos>,
            Option<bool>,
            Option<bool>,
            Option<bool>,
            Option<moon_core::market::CandleViewCfg>,
            Option<moon_core::config::ChartGraphicsCfg>,
        )> = {
            let all = &self.backend.read(cx).chart_specs;
            all.iter()
                .filter(|s| s.group == self.group && s.detached.is_none())
                .filter_map(|s| {
                    s.custom_coins.clone().map(|coins| {
                        (
                            s.num,
                            s.bucket(),
                            coins,
                            s.custom_label.clone(),
                            s.scale,
                            (s.layout_mode, s.layout_height_fit, s.layout_height_scroll),
                            s.layout_orientation,
                            s.orderbook_enabled,
                            s.liquidations_enabled,
                            s.show_zone,
                            s.auto_pin,
                            s.compare_anchor.clone(),
                            s.compare_orderbook_only,
                            s.price_axis_pos,
                            s.time_axis_visible,
                            s.line_labels,
                            s.cursor_labels,
                            s.candle_view,
                            s.chart_graphics,
                        )
                    })
                })
                .collect()
        };
        for (
            num,
            bucket,
            coins,
            label,
            scale,
            layout,
            orientation,
            ob,
            liq,
            sz,
            ap,
            anchor,
            broom,
            axis_pos,
            time_axis,
            line_labels,
            cursor_labels,
            candle_view,
            chart_graphics,
        ) in specs
        {
            let stack = cx.new(|_| {
                AddChartStack::new(
                    self.backend.clone(),
                    self.group.clone(),
                    num,
                    bucket.clone(),
                    self.epoch,
                    self.theme.clone(),
                )
            });
            stack.update(cx, |s, c| {
                s.set_hold_vacated(false);
                s.set_orientation(Some(orientation.unwrap_or(StackOrientation::Horizontal)), c);
                if scale.is_some() {
                    s.set_scale(scale, c);
                }
                s.set_layout(layout.0, layout.1, layout.2, c);
                if let Some(v) = ob {
                    s.set_orderbook_enabled(Some(v), c);
                }
                if let Some(v) = liq {
                    s.set_liquidations_enabled(Some(v), c);
                }
                if let Some(v) = sz {
                    s.set_show_zone(Some(v), c);
                }
                if let Some(v) = ap {
                    s.set_auto_pin(Some(v), c);
                }
                if axis_pos.is_some() {
                    s.set_price_axis_pos(axis_pos, c);
                }
                if time_axis.is_some() {
                    s.set_time_axis_visible(time_axis, c);
                }
                if line_labels.is_some() {
                    s.set_line_labels(line_labels, c);
                }
                if cursor_labels.is_some() {
                    s.set_cursor_labels(cursor_labels, c);
                }
                if candle_view.is_some() {
                    s.set_candle_view(candle_view, c);
                }
                if chart_graphics.is_some() {
                    s.set_chart_graphics(chart_graphics, c);
                }
                for (core, market) in &coins {
                    s.add_coin(*core, market, coin_search::MANUAL_COIN_TTL_MS, c);
                }
                s.pin_all(c);
            });
            // Restore comparison mode (anchor plus broom) after loading the tickers.
            if anchor.is_some() || broom {
                stack.update(cx, |s, c| s.restore_compare(anchor.clone(), broom, c));
            }
            self.watch_custom_stack(num, &bucket, &stack, cx);
            self.custom.push((num, bucket, stack));
            if let Some(label) = label {
                self.custom_labels.insert(num, label);
            }
            self.next_custom_num = self.next_custom_num.max(num + 1);
        }
        if !self.custom.is_empty() {
            self.refresh_orderbook_gates(cx);
        }
    }

    /// Update custom-tab order-book gates from focus. Resume the active tab immediately; suspend
    /// inactive tabs remaining in the strip after five seconds if focus does not return. Detached
    /// tabs are absent from `self.custom`, so they are never suspended; their window maintains
    /// demand.
    pub(super) fn refresh_orderbook_gates(&mut self, cx: &mut Context<Self>) {
        let active = self.active.clone();
        let customs: Vec<(u32, ChartBucket, Entity<AddChartStack>)> = self.custom.clone();
        for (n, b, stack) in customs {
            if Tab::Custom(n, b.clone()) == active {
                // Returning to the tab cancels the pending timer and resubscribes immediately.
                *self.custom_gate_gen.entry(n).or_insert(0) += 1;
                stack.update(cx, |s, c| s.set_orderbook_suspended(false, c));
            } else {
                // Leaving the tab starts a five-second unsubscribe timer; the latest generation
                // wins.
                let want_gen = {
                    let e = self.custom_gate_gen.entry(n).or_insert(0);
                    *e += 1;
                    *e
                };
                let stack = stack.clone();
                cx.spawn(async move |this, cx| {
                    let executor = cx.update(|cx| cx.background_executor().clone());
                    executor.timer(Duration::from_secs(5)).await;
                    let _ = cx.update(|cx| {
                        this.update(cx, |this, cx| {
                            // Is the timer still current, with the tab still inactive in the strip?
                            let still = this.custom_gate_gen.get(&n) == Some(&want_gen)
                                && !matches!(&this.active, Tab::Custom(nn, _) if *nn == n)
                                && this.custom.iter().any(|(num, _, _)| *num == n);
                            if still {
                                stack.update(cx, |s, c| s.set_orderbook_suspended(true, c));
                            }
                        })
                        .ok();
                    });
                })
                .detach();
            }
        }
    }
}

/// Coin search in the tab strip. The selected coin opens on the ACTIVE tab (Main → fullscreen
/// chart; Add/Custom → its stack). Popup plumbing lives in [`super::common`].
impl CoinPopupHost for ChartTabs {
    /// Return the shared backend that supplies search state and persisted recents.
    fn coin_backend(&self) -> Entity<crate::Backend> {
        self.backend.clone()
    }

    /// Clear the coin field and close the list after selection or an outside click.
    fn clear_coin_search(&mut self, cx: &mut Context<Self>) {
        self.coin_query.clear();
        self.coin_popup_open = false;
        cx.notify();
    }
    fn open_picked_coin(&mut self, core: CoreId, market: String, cx: &mut Context<Self>) {
        self.open_coin_on_active(core, market, cx);
    }
}
