//! Рефкаунт рынков/стаканов в backend и time-based таймеры панели чарта: TTL авто-закрытия
//! AddToChart-графиков и авто-возврат в live после пана. Вынесено из `chart.rs`.

use std::collections::HashSet;
use std::time::Duration;

use gpui::*;

use moon_chart::paint::now_unix_ms;
use moon_core::session::CoreId;

use super::ChartPanel;

impl ChartPanel {
    pub(super) fn retain_market_ref(&mut self, core: CoreId, market: &str, cx: &mut App) {
        self.sync_market_ref_epoch(cx);
        if self.registered_markets.insert((core, market.to_string())) {
            self.backend.update(cx, |b, _| {
                b.retain_chart_market(core, market);
            });
        }
        self.sync_orderbook_refs(cx);
    }

    pub(super) fn release_market_ref(&mut self, core: CoreId, market: &str, cx: &mut App) {
        self.sync_market_ref_epoch(cx);
        if self.registered_markets.remove(&(core, market.to_string())) {
            self.backend.update(cx, |b, _| {
                b.release_chart_market(core, market);
            });
        }
        self.sync_orderbook_refs(cx);
    }

    pub(super) fn release_market_refs_except(&mut self, keep: Option<(CoreId, &str)>, cx: &mut App) {
        self.sync_market_ref_epoch(cx);
        let keep = keep.map(|(core, market)| (core, market.to_string()));
        let old = std::mem::take(&mut self.registered_markets);
        for (core, market) in old {
            if keep.as_ref().is_some_and(|k| k.0 == core && k.1 == market) {
                self.registered_markets.insert((core, market));
            } else {
                self.backend.update(cx, |b, _| {
                    b.release_chart_market(core, &market);
                });
            }
        }
        self.sync_orderbook_refs(cx);
    }

    pub(super) fn release_all_market_refs(&mut self, cx: &mut App) {
        self.sync_market_ref_epoch(cx);
        let old = std::mem::take(&mut self.registered_markets);
        for (core, market) in old {
            self.backend.update(cx, |b, _| {
                b.release_chart_market(core, &market);
            });
        }
        self.sync_orderbook_refs(cx);
    }

    fn sync_market_ref_epoch(&mut self, cx: &mut App) {
        let epoch = self.backend.read(cx).chart_market_refs_epoch;
        if self.market_ref_epoch != epoch {
            self.registered_markets.clear();
            self.registered_orderbook.clear();
            self.market_ref_epoch = epoch;
        }
    }

    /// Привести orderbook-ref backend к состоянию «рынки этой панели, если стакан включён».
    /// Зовётся после любых изменений рынков и при переключении стакана. Без borrow самого view.
    pub(super) fn sync_orderbook_refs(&mut self, cx: &mut App) {
        let want: HashSet<(CoreId, String)> = if self.orderbook_enabled {
            self.registered_markets.clone()
        } else {
            HashSet::new()
        };
        let to_release: Vec<(CoreId, String)> = self
            .registered_orderbook
            .difference(&want)
            .cloned()
            .collect();
        for (core, market) in to_release {
            self.registered_orderbook.remove(&(core, market.clone()));
            self.backend
                .update(cx, |b, _| b.release_chart_orderbook(core, &market));
        }
        let to_add: Vec<(CoreId, String)> = want
            .difference(&self.registered_orderbook)
            .cloned()
            .collect();
        for (core, market) in to_add {
            self.registered_orderbook.insert((core, market.clone()));
            self.backend
                .update(cx, |b, _| b.retain_chart_orderbook(core, &market));
        }
    }

    fn next_ttl_delay(&self, now_ms: f64) -> Option<Duration> {
        self.chart
            .next_ttl_deadline_ms()
            .map(|deadline| Duration::from_millis((deadline - now_ms).max(1.0).ceil() as u64))
    }

    pub(super) fn arm_ttl_timer(&mut self, cx: &mut Context<Self>) {
        if self.ttl_timer_armed {
            return;
        }
        let Some(delay) = self.next_ttl_delay(now_unix_ms()) else {
            return;
        };
        self.ttl_timer_armed = true;
        cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            executor.timer(delay).await;
            let _ = cx.update(|cx| {
                this.update(cx, |this, cx| {
                    this.ttl_timer_armed = false;
                    let removed = this.chart.prune_ttl(now_unix_ms());
                    if !removed.is_empty() {
                        this.view_dirty = true;
                        for (core, market) in removed {
                            this.release_market_ref(core, &market, cx);
                        }
                        crate::diag::bump(&crate::diag::CHART_TTL_NOTIFY);
                        cx.notify();
                    }
                    this.arm_ttl_timer(cx);
                })
                .is_ok()
            });
        })
        .detach();
    }

    fn next_auto_live_delay(&self, now_ms: f64) -> Option<Duration> {
        self.chart
            .next_auto_live_deadline_ms()
            .map(|deadline| Duration::from_millis((deadline - now_ms).max(1.0).ceil() as u64))
    }

    /// One-shot таймер авто-возврата в live (П.9): мирроринг `arm_ttl_timer`. Армится из
    /// `mark_input_changed` после пана; по срабатыванию двигает live и пере-армится на
    /// следующий дедлайн (или гаснет, если возвращать больше нечего).
    pub(super) fn arm_auto_live_timer(&mut self, cx: &mut Context<Self>) {
        if self.auto_live_timer_armed {
            return;
        }
        let Some(delay) = self.next_auto_live_delay(now_unix_ms()) else {
            return;
        };
        self.auto_live_timer_armed = true;
        cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            executor.timer(delay).await;
            let _ = cx.update(|cx| {
                this.update(cx, |this, cx| {
                    this.auto_live_timer_armed = false;
                    if this.chart.tick_auto_live(now_unix_ms()) {
                        this.view_dirty = true;
                        let follow = this.chart.follow();
                        this.backend.update(cx, |b, bcx| {
                            if b.follow != follow {
                                b.follow = follow;
                                bcx.notify();
                            }
                        });
                        cx.notify();
                    }
                    this.arm_auto_live_timer(cx);
                })
                .is_ok()
            });
        })
        .detach();
    }
}
