//! `ChartTabs`: ингест AddToChart-детектов (создание/наполнение вкладок ядра) и поиск
//! активного стека по `(num, bucket)`. Вынесено из `mod.rs`.

use gpui::*;

use super::{AddChartStack, ChartTabs};
use moon_core::config::ChartBucket;
use moon_core::session::CoreId;

impl ChartTabs {
    /// Ингест AddToChart-детектов (add_to_chart>0) → создать/наполнить вкладку.
    /// Ключ вкладки — `ChartBucket` ядра (своё ядро / общая / именованная связка),
    /// резолвится из конфига ядра + глоб. `charts_split_by_core`.
    /// БЕЗ авто-перехода: active не трогаем (порт «не уводить на чарт при детекте»).
    pub(super) fn ingest(&mut self, cx: &mut Context<Self>) {
        let (split, fresh, cursors): (
            bool,
            Vec<(u32, CoreId, ChartBucket, String, f64)>,
            Vec<(CoreId, u64)>,
        ) = {
            let b = self.backend.read(cx);
            let split = b.config.charts_split_by_core;
            let mut fresh = Vec::new();
            let mut cursors = Vec::new();
            for s in b
                .session
                .sessions()
                .iter()
                .filter(|s| s.group == self.group)
            {
                let id = s.id;
                let Some(d) = b.session.store().core(id) else {
                    continue;
                };
                // Bucket ядра — из его конфига (связка) + глоб. split. Нет конфига → своя вкладка.
                let bucket = b
                    .config
                    .servers
                    .iter()
                    .find(|sv| sv.id == id)
                    .map(|sv| sv.chart_bucket(split))
                    .unwrap_or(ChartBucket::Core(id));
                let last = self.add_seq.get(&id).copied().unwrap_or(0);
                let mut mx = last;
                for det in &d.detects {
                    if det.seq <= last {
                        continue;
                    }
                    mx = mx.max(det.seq);
                    if det.add_to_chart > 0 {
                        let ttl = (det.keep_in_chart_secs.max(1) as f64) * 1000.0;
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
            (split, fresh, cursors)
        };
        for (id, mx) in cursors {
            self.add_seq.insert(id, mx);
        }
        if fresh.is_empty() {
            return;
        }
        // detect-diag: AddToChart-детекты дошли до UI этой группы. fresh — сколько монет
        // на добавление в этом проходе. (env MOON_DETECT_DIAG, off by default.)
        moon_core::detect_diag::line(&format!(
            "[ingest] group={} split={split} fresh={} existing_tabs={}",
            self.group,
            fresh.len(),
            self.add.len()
        ));
        let (epoch, theme, backend) = (self.epoch, self.theme.clone(), self.backend.clone());
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
                tab.update(cx, |p, pcx| p.add_coin(core, &market, ttl, pcx));
            } else {
                let panel = cx.new(|_| {
                    AddChartStack::new(backend.clone(), n, bucket.clone(), epoch, theme.clone())
                });
                // Восстановить сохранённый масштаб и раскладку этой вкладки (charts.json).
                let (
                    saved_scale,
                    saved_layout,
                    saved_orderbook,
                    saved_show_zone,
                    saved_auto_pin,
                    saved_action_pos,
                    saved_axis_pos,
                    saved_time_axis,
                ) = {
                    let specs = &self.backend.read(cx).chart_specs;
                    let spec = specs
                        .iter()
                        .find(|s| s.matches(&self.group, n, &bucket));
                    (
                        spec.and_then(|s| s.scale),
                        spec.map_or((None, None, None), |s| {
                            (s.layout_mode, s.layout_height_fit, s.layout_height_scroll)
                        }),
                        spec.and_then(|s| s.orderbook_enabled),
                        spec.and_then(|s| s.show_zone),
                        spec.and_then(|s| s.auto_pin),
                        spec.map_or((None, None), |s| (s.cancel_buy_pos, s.panic_sell_pos)),
                        spec.and_then(|s| s.price_axis_pos),
                        spec.and_then(|s| s.time_axis_visible),
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
                if saved_action_pos.0.is_some() || saved_action_pos.1.is_some() {
                    panel.update(cx, |p, pcx| {
                        p.set_action_btn_pos(saved_action_pos.0, saved_action_pos.1, pcx)
                    });
                }
                if saved_axis_pos.is_some() {
                    panel.update(cx, |p, pcx| p.set_price_axis_pos(saved_axis_pos, pcx));
                }
                if saved_time_axis.is_some() {
                    panel.update(cx, |p, pcx| p.set_time_axis_visible(saved_time_axis, pcx));
                }
                panel.update(cx, |p, pcx| p.add_coin(core, &market, ttl, pcx));
                self.add.push((n, bucket.clone(), panel));
                // Порядок вкладок: по (номер, bucket) — как egui sort_by_key.
                self.add.sort_by_key(|(num, c, _)| (*num, c.clone()));
                moon_core::detect_diag::line(&format!(
                    "[ingest] NEW tab n={n} bucket={bucket:?} (total_tabs={})",
                    self.add.len()
                ));
                // active НЕ меняем — не уводим пользователя на новую вкладку.
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
