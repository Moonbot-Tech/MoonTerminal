//! Откреп-вкладки чартов: жизненный цикл их ОС-окон (создание/восстановление/репин,
//! персист геометрии и масштаба) и хост-вид окна `DetachedChartHost`. Вынесено из
//! `chart_tabs` как отдельная подсистема выносных окон — сама полоска вкладок про неё
//! знает лишь через несколько `pub(super)`-методов, дёргаемых из event/observe путей.

use gpui::*;
use moon_ui::{MoonBackgroundPolicy, Root};

use super::detached_host::DetachedChartHost;
use super::{AddChartStack, ChartTabs, Tab, chart_pane_label, coin_search};
use crate::chart_persist::{self, StackLayoutMode, StackOrientation};
use moon_core::config::ChartBucket;
use moon_core::session::CoreId;

impl ChartTabs {
    /// Дабл-клик по чарту AddToChart-вкладки → открыть монету на Main + переключиться.
    /// Собрать откреплённые окна чартов ЭТОЙ группы: восстановить (разминимизировать), показать
    /// и каскадом вернуть на первичный монитор. Спасение, если окна свёрнуты/спрятаны/уехали за
    /// экран (они независимы и не ходят за Main). Кнопка в полосе вкладок Main-окна группы.
    pub(super) fn gather_windows(&mut self, cx: &mut Context<Self>) {
        let group = self.group.clone();
        let handles: Vec<_> = self
            .backend
            .read(cx)
            .detached_chart_windows
            .iter()
            .filter(|(g, _)| *g == group)
            .map(|(_, h)| *h)
            .collect();
        for (i, handle) in handles.into_iter().enumerate() {
            let _ = handle.update(cx, |_, window, _| {
                crate::windowing::reset_window_onscreen(window, i);
                window.activate_window();
            });
        }
    }

    /// Отцепить AddToChart/Custom-вкладку в отдельное ОС-окно (убрать из стрипа). Кастомная
    /// вкладка живёт в `self.custom`, обычная — в `self.add`; обе используют `AddChartStack`.
    pub(super) fn detach(&mut self, tab: Tab, cx: &mut Context<Self>) {
        let (n, bucket, is_custom) = match tab.clone() {
            Tab::Add(n, b) => (n, b, false),
            Tab::Custom(n, b) => (n, b, true),
            Tab::Main => return,
        };
        let from = if is_custom { &self.custom } else { &self.add };
        let Some(pos) = from
            .iter()
            .position(|(num, c, _)| *num == n && *c == bucket)
        else {
            return;
        };
        let panel = from[pos].2.clone();
        // Геометрия: сохранённая (если уже откреплялась) или дефолт-каскад.
        let geom = self
            .spec_geom(cx, n, &bucket)
            .unwrap_or(chart_persist::WinGeom {
                x: 200,
                y: 160,
                w: 900,
                h: 620,
            });
        if !self.open_chart_window(n, panel.clone(), bucket.clone(), geom, false, cx) {
            return;
        }
        // Откреплённая вкладка держит свой спрос на стаканы (окно видимо) → снимаем suspend-гейт.
        panel.update(cx, |p, pcx| {
            p.set_orderbook_suspended(false, pcx);
            p.set_scene_visible(false, pcx);
        });
        if is_custom {
            self.custom
                .retain(|(num, c, _)| !(*num == n && *c == bucket));
        } else {
            self.add.remove(pos);
        }
        self.detached.push((n, bucket.clone(), panel));
        if self.active == tab {
            self.active = Tab::Main;
            self.sync_seen_for_active(cx);
            self.sync_active_scale(cx);
            self.sync_inactive_chart_visibility(cx);
            self.persist_scales(cx);
        }
        // Пометить вкладку откреплённой в charts.json (восстановится окном на след. запуске).
        self.upsert_spec(cx, n, &bucket, |s| s.detached = Some(geom));
        moon_core::detect_diag::line(&format!(
            "[detach] n={n} bucket={bucket:?} → detached=Some({},{},{},{})",
            geom.x, geom.y, geom.w, geom.h
        ));
        cx.notify();
    }

    /// Открыть ОС-окно откреп-вкладки (общий код detach и восстановления при загрузке). Панель
    /// держим в `detached` (ingest наполняет её по num/core); `gpu_canvas` переезжает вместе
    /// с GPUI scene окна.
    /// Хост (`DetachedChartHost`) сам пишет геометрию и просит репин по закрытию. Окно трекаем
    /// по группе (закрытие окна группы закроет его — main.rs on_window_closed).
    fn open_chart_window(
        &mut self,
        n: u32,
        panel: Entity<AddChartStack>,
        bucket: ChartBucket,
        geom: chart_persist::WinGeom,
        restored: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        // КРИТИЧНО для мультимонитора: без display_id окно создаётся на PRIMARY, и если
        // сохранённые bounds вне primary — gpui откатывается на default_bounds() (центр + дефолт-
        // размер). Поэтому ищем монитор, СОДЕРЖАЩИЙ сохранённую точку, и передаём его display_id —
        // тогда bounds валидны для него и окно встаёт точно (см. retrieve_window_placement).
        let origin = point(px(geom.x as f32), px(geom.y as f32));
        let display_id = cx
            .displays()
            .into_iter()
            .find(|d| d.bounds().contains(&origin))
            .map(|d| d.id());
        let mut opts = crate::windowing::detached_chart_window_options(
            format!(
                "MoonTerminal — {}",
                chart_pane_label(&self.backend, &self.group, n, &bucket, cx)
            ),
            WindowBounds::Windowed(Bounds {
                origin,
                size: size(px(geom.w as f32), px(geom.h as f32)),
            }),
            display_id,
        );
        // Цвет clear окна — из темы (фон чарта). Тело окна прозрачное (own-pass UnderScene нельзя
        // перекрывать), поэтому подложку под/между чартами даёт именно clear; без этого он белый.
        let bg = self.theme.bg;
        opts.window_clear_color = Some(gpui::rgb(
            ((bg[0] as u32) << 16) | ((bg[1] as u32) << 8) | bg[2] as u32,
        ));
        let backend = self.backend.clone();
        let group = self.group.clone();
        // Для восстановленного окна — сохранённый логический размер, чтобы скорректировать
        // DPICHANGED-сжатие на первом render (см. DetachedChartHost.restore_size).
        let restore_size = restored.then(|| size(px(geom.w as f32), px(geom.h as f32)));
        let host_bucket = bucket.clone();
        let opened = cx.open_window(opts, move |window, cx| {
            crate::windowing::configure_chart_clear_color(window, cx);
            let host = cx.new(|cx| {
                DetachedChartHost::new(
                    panel,
                    backend,
                    group,
                    n,
                    host_bucket,
                    restored,
                    restore_size,
                    window,
                    cx,
                )
            });
            cx.new(|cx| Root::new(host, window, cx).background_policy(MoonBackgroundPolicy::NoFill))
        });
        if let Ok(handle) = opened {
            let group = self.group.clone();
            self.backend.update(cx, |b, _| {
                b.detached_chart_windows.push((group, handle));
            });
            true
        } else {
            log::warn!(
                "failed to open detached chart window for group={} n={} bucket={:?}",
                self.group,
                n,
                bucket
            );
            false
        }
    }

    /// Геометрия сохранённого откреп-окна вкладки (если есть в charts.json).
    fn spec_geom(
        &self,
        cx: &App,
        num: u32,
        bucket: &ChartBucket,
    ) -> Option<chart_persist::WinGeom> {
        self.backend
            .read(cx)
            .chart_specs
            .iter()
            .find(|s| s.matches(&self.group, num, bucket))
            .and_then(|s| s.detached)
    }

    /// Найти/создать спеку вкладки (group/num/bucket), применить мутатор, пометить dirty.
    pub(super) fn upsert_spec(
        &self,
        cx: &mut Context<Self>,
        num: u32,
        bucket: &ChartBucket,
        f: impl FnOnce(&mut chart_persist::ChartTabSpec),
    ) {
        let group = self.group.clone();
        self.backend.update(cx, |b, _| {
            chart_persist::upsert(&mut b.chart_specs, &group, num, bucket, f);
            b.chart_specs_dirty = true;
        });
    }

    /// Дренаж репина откреп-вкладок: хост закрыли (пользователь) → панель detached→add, спека
    /// → НЕ откреплена. Зовётся из backend observe. (На выходе приложения запрос не обработается → спека
    /// остаётся откреплённой → окно восстановится на след. запуске — как у detached.rs.)
    pub(super) fn drain_chart_repin(&mut self, cx: &mut Context<Self>) {
        // На выходе из приложения НЕ репиним: закрытие откреп-окон при quit не должно сбрасывать
        // detached (иначе окна не восстановятся). Финальный сейв уже сделан в on_app_quit.
        if self.backend.read(cx).quitting {
            return;
        }
        let group = self.group.clone();
        let reqs: Vec<(u32, ChartBucket)> = self.backend.update(cx, |b, _| {
            let mut out = Vec::new();
            b.chart_repin_request.retain(|(g, n, c)| {
                if *g == group {
                    out.push((*n, c.clone()));
                    false
                } else {
                    true
                }
            });
            out
        });
        for (n, bucket) in reqs {
            // Кастомная вкладка возвращается в стрип как Custom (по наличию custom_coins/label
            // в спеке), обычная — как Add.
            let (is_custom, custom_label) = {
                let specs = &self.backend.read(cx).chart_specs;
                let spec = specs.iter().find(|s| s.matches(&self.group, n, &bucket));
                (
                    spec.is_some_and(|s| s.custom_coins.is_some()),
                    spec.and_then(|s| s.custom_label.clone()),
                )
            };
            if let Some(p) = self
                .detached
                .iter()
                .position(|(num, c, _)| *num == n && *c == bucket)
            {
                let (num, c, pnl) = self.detached.remove(p);
                if is_custom {
                    self.custom.push((num, c, pnl));
                    if let Some(label) = custom_label {
                        self.custom_labels.entry(n).or_insert(label);
                    }
                } else {
                    self.add.push((num, c, pnl));
                    self.add.sort_by_key(|(num, c, _)| (*num, c.clone()));
                }
            }
            self.upsert_spec(cx, n, &bucket, |s| s.detached = None);
            moon_core::detect_diag::line(&format!(
                "[repin] n={n} bucket={bucket:?} custom={is_custom} → detached=None (окно закрыли/репин)"
            ));
            // Вернулась в стрип неактивной (active=Main) → запустить 5с-гейт стаканов для кастома.
            if is_custom {
                self.refresh_orderbook_gates(cx);
            }
            cx.notify();
        }
    }

    /// Сохранить масштаб каждой вкладки в charts.json (upsert при изменении). Main = num 0.
    pub(super) fn persist_scales(&self, cx: &mut Context<Self>) {
        let mut items: Vec<(u32, ChartBucket, Option<f32>)> =
            vec![(0, ChartBucket::Shared, self.main.read(cx).scale())];
        for (n, c, p) in &self.add {
            items.push((*n, c.clone(), p.read(cx).scale()));
        }
        for (n, c, p) in &self.detached {
            items.push((*n, c.clone(), p.read(cx).scale()));
        }
        for (num, bucket, scale) in items {
            let (cur, exists) = {
                let specs = &self.backend.read(cx).chart_specs;
                let found = specs.iter().find(|s| s.matches(&self.group, num, &bucket));
                (found.and_then(|s| s.scale), found.is_some())
            };
            if cur != scale && (scale.is_some() || exists) {
                self.upsert_spec(cx, num, &bucket, move |s| s.scale = scale);
            }
        }
    }

    /// Восстановить отложенные откреп-окна (charts.json). Открывать ОС-окна В render НЕЛЬЗЯ
    /// (рушит element-арену gpui: «ArenaRef after Arena was cleared»). Вызов идёт из
    /// конструктора ChartTabs, а фактическое открытие откладываем через `cx.defer`.
    pub(super) fn restore_detached(&mut self, cx: &mut Context<Self>) {
        if self.restore_pending.is_empty() {
            return;
        }
        let pending = std::mem::take(&mut self.restore_pending);
        let this = cx.entity();
        cx.defer(move |app| {
            this.update(app, |this, cx| {
                let (epoch, theme) = (this.epoch, this.theme.clone());
                // Откреп-чарты всегда independent: owned-связь поднимает Main при клике
                // по графику на мультимониторе. Taskbar скрывается policy + Windows fallback.
                for (n, bucket, geom, scale) in pending {
                    let backend = this.backend.clone();
                    let panel = cx.new(|_| {
                        AddChartStack::new(backend, n, bucket.clone(), epoch, theme.clone())
                    });
                    if scale.is_some() {
                        panel.update(cx, |p, pcx| p.set_scale(scale, pcx));
                    }
                    // Откреплённая КАСТОМНАЯ вкладка: ингест её не наполняет → заливаем тикеры из
                    // спека (с раскладкой/ориентацией/пином) прямо сейчас, как при создании.
                    #[allow(clippy::type_complexity)]
                    let custom: Option<(
                        Vec<(CoreId, String)>,
                        Option<String>,
                        (Option<StackLayoutMode>, Option<u16>, Option<u16>),
                        Option<StackOrientation>,
                        Option<bool>,
                        Option<bool>,
                        Option<bool>,
                        Option<(CoreId, String)>,
                        bool,
                        Option<chart_persist::PriceAxisPos>,
                        Option<bool>,
                    )> = {
                        let specs = &this.backend.read(cx).chart_specs;
                        specs
                            .iter()
                            .find(|s| s.matches(&this.group, n, &bucket))
                            .and_then(|s| {
                                s.custom_coins.clone().map(|coins| {
                                    (
                                        coins,
                                        s.custom_label.clone(),
                                        (
                                            s.layout_mode,
                                            s.layout_height_fit,
                                            s.layout_height_scroll,
                                        ),
                                        s.layout_orientation,
                                        s.orderbook_enabled,
                                        s.show_zone,
                                        s.auto_pin,
                                        s.compare_anchor.clone(),
                                        s.compare_orderbook_only,
                                        s.price_axis_pos,
                                        s.time_axis_visible,
                                    )
                                })
                            })
                    };
                    if let Some((
                        coins,
                        label,
                        layout,
                        orientation,
                        ob,
                        sz,
                        ap,
                        anchor,
                        broom,
                        axis_pos,
                        time_axis,
                    )) = custom
                    {
                        panel.update(cx, |s, c| {
                            s.set_hold_vacated(false);
                            s.set_orientation(
                                Some(orientation.unwrap_or(StackOrientation::Horizontal)),
                                c,
                            );
                            s.set_layout(layout.0, layout.1, layout.2, c);
                            if let Some(v) = ob {
                                s.set_orderbook_enabled(Some(v), c);
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
                            for (core, market) in &coins {
                                s.add_coin(*core, market, coin_search::MANUAL_COIN_TTL_MS, c);
                            }
                            s.pin_all(c);
                        });
                        if anchor.is_some() || broom {
                            panel.update(cx, |s, c| s.restore_compare(anchor.clone(), broom, c));
                        }
                        if let Some(label) = label {
                            this.custom_labels.insert(n, label);
                        }
                        this.next_custom_num = this.next_custom_num.max(n + 1);
                        // Подписка ChartTabs на состав: пока окно открыто — персистит хост, но
                        // после репина в стрип эта подписка снова обслуживает кастомную вкладку.
                        this.watch_custom_stack(n, &bucket, &panel, cx);
                    }
                    if this.open_chart_window(n, panel.clone(), bucket.clone(), geom, true, cx) {
                        panel.update(cx, |p, pcx| p.set_scene_visible(false, pcx));
                        this.detached.push((n, bucket, panel));
                    }
                }
                cx.notify();
            });
        });
    }
}
