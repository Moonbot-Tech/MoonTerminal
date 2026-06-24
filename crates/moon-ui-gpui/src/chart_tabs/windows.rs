//! Откреп-вкладки чартов: жизненный цикл их ОС-окон (создание/восстановление/репин,
//! персист геометрии и масштаба) и хост-вид окна `DetachedChartHost`. Вынесено из
//! `chart_tabs` как отдельная подсистема выносных окон — сама полоска вкладок про неё
//! знает лишь через несколько `pub(super)`-методов, дёргаемых из event/observe путей.

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    MoonBackgroundPolicy, MoonButton, MoonButtonSize, MoonButtonVariant, MoonInputEvent,
    MoonInputState, MoonPalette, MoonWindowFrame, MoonWindowFrameControls, Root, h_flex, v_flex,
};
use rust_i18n::t;
use std::time::Duration;

use super::{AddChartStack, ChartTabs, Tab, chart_pane_label, layout_popup};
use crate::Backend;
use crate::chart_persist::{self, StackLayoutMode};
use crate::design;
use moon_core::config::ChartBucket;

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

    /// Отцепить AddToChart-вкладку в отдельное ОС-окно (убрать из стрипа).
    pub(super) fn detach(&mut self, tab: Tab, cx: &mut Context<Self>) {
        let Tab::Add(n, bucket) = tab.clone() else {
            return;
        };
        let Some(pos) = self
            .add
            .iter()
            .position(|(num, c, _)| *num == n && *c == bucket)
        else {
            return;
        };
        let (_, _, panel) = self.add[pos].clone();
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
        let (_, _, panel) = self.add.remove(pos);
        panel.update(cx, |p, pcx| p.set_scene_visible(false, pcx));
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
            .find(|s| s.group == self.group && s.num == num && s.bucket() == *bucket)
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
            if let Some(s) = b
                .chart_specs
                .iter_mut()
                .find(|s| s.group == group && s.num == num && s.bucket() == *bucket)
            {
                f(s);
            } else {
                let mut s = chart_persist::ChartTabSpec {
                    group,
                    num,
                    core: None,
                    bucket: Some(bucket.clone()),
                    scale: None,
                    detached: None,
                    layout_mode: None,
                    layout_height_fit: None,
                    layout_height_scroll: None,
                    orderbook_enabled: None,
                };
                f(&mut s);
                b.chart_specs.push(s);
            }
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
            if let Some(p) = self
                .detached
                .iter()
                .position(|(num, c, _)| *num == n && *c == bucket)
            {
                let (num, c, pnl) = self.detached.remove(p);
                self.add.push((num, c, pnl));
                self.add.sort_by_key(|(num, c, _)| (*num, c.clone()));
            }
            self.upsert_spec(cx, n, &bucket, |s| s.detached = None);
            moon_core::detect_diag::line(&format!(
                "[repin] n={n} bucket={bucket:?} → detached=None (окно закрыли/репин)"
            ));
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
                let found = specs
                    .iter()
                    .find(|s| s.group == self.group && s.num == num && s.bucket() == bucket);
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

/// Хост-вид окна откреплённой чарт-вкладки: шапка (масштаб + «закрыть все графики») + панель.
/// Сам пишет геометрию окна в charts.json (`observe_window_bounds`) и просит репин по закрытию
/// (`on_release` → `chart_repin_request`, дренит ChartTabs).
struct DetachedChartHost {
    panel: Entity<AddChartStack>,
    backend: Entity<Backend>,
    group: String,
    num: u32,
    bucket: ChartBucket,
    /// Можно ли сохранять геометрию из `observe_window_bounds`. У ВОССТАНОВЛЕННОГО окна сперва
    /// false: авто-размещение gpui на не-primary DPI читается со сдвигом ×scale, и пересохранять
    /// его НЕЛЬЗЯ (иначе позиция уезжает с каждым запуском). Армируется через ~1.5с — дальше
    /// пишем только реальные перемещения пользователя. У свежего детача — сразу true.
    persist_armed: bool,
    /// Логический размер для коррекции на ПЕРВОМ render восстановленного окна: gpui создаёт окно
    /// на primary, и `WM_DPICHANGED` при переезде на монитор с другим DPI пере-масштабирует
    /// РАЗМЕР (позиция уже верная) → форсим сохранённый логический размер один раз. None у детача.
    restore_size: Option<Size<Pixels>>,
    /// Кнопку окна из таскбара убираем `ITaskbarList::DeleteTab` на первых рендерах (когда окно
    /// уже показано и кнопка создана). Окно при этом остаётся обычным independent → FancyZones его
    /// видит. Несколько тиков — подстраховка от гонки «кнопка ещё не появилась».
    taskbar_hide_ticks: u8,
    /// In-scene попап настроек раскладки этой вкладки (кнопка ⚙). Не отдельное ОС-окно:
    /// chart text теперь лежит ниже обычной GPUI scene.
    layout_popup_open: bool,
    /// Был ли курсор внутри popup-а. Уход после первого входа закрывает popup и коммитит ввод.
    layout_popup_hovered: bool,
    /// Поле высоты режима Fit.
    layout_fit_input: Entity<MoonInputState>,
    /// Поле высоты режима Scroll.
    layout_scroll_input: Entity<MoonInputState>,
}

impl DetachedChartHost {
    fn new(
        panel: Entity<AddChartStack>,
        backend: Entity<Backend>,
        group: String,
        num: u32,
        bucket: ChartBucket,
        restored: bool,
        restore_size: Option<Size<Pixels>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // Геометрия окна (causal bounds event) → charts.json («то же место» при загрузке).
        cx.observe_window_bounds(window, |this, window, cx| {
            this.persist_geometry(window, cx);
        })
        .detach();
        // Восстановленное окно не пишет стартовые bounds сразу: на не-primary DPI GPUI/Win32
        // могут прислать временную позицию/размер со scale-сдвигом. Это нельзя сохранять, иначе
        // окно будет уезжать на каждом запуске. Через короткое окно стабилизации снова разрешаем
        // обычный persist пользовательских move/resize. Свежий detach сохраняет геометрию сразу.
        if restored {
            cx.spawn(async move |this, cx| {
                let executor = cx.update(|cx| cx.background_executor().clone());
                executor.timer(Duration::from_millis(1500)).await;
                let _ = cx.update(|cx| {
                    this.update(cx, |this, _cx| {
                        this.persist_armed = true;
                        moon_core::detect_diag::line(&format!(
                            "[geom] n={} bucket={:?} persist armed after restore settle",
                            this.num, this.bucket
                        ));
                    })
                    .is_ok()
                });
            })
            .detach();
        }
        // Закрытие окна → репин в стрип (дренит ChartTabs). На выходе приложения запрос не
        // обработается → спека остаётся откреплённой → окно восстановится на след. запуске.
        let (g, n, c) = (group.clone(), num, bucket.clone());
        cx.on_release(move |this, app| {
            this.backend.update(app, |b, cx| {
                b.chart_repin_request.push((g.clone(), n, c.clone()));
                cx.notify();
            });
        })
        .detach();
        // Восстановить сохранённую раскладку + флаг стакана вкладки из charts.json в панель.
        let (group2, num2, bucket2) = (group.clone(), num, bucket.clone());
        let saved = backend.read(cx).chart_specs.iter().find_map(|s| {
            (s.group == group2 && s.num == num2 && s.bucket() == bucket2).then(|| {
                (
                    s.layout_mode,
                    s.layout_height_fit,
                    s.layout_height_scroll,
                    s.orderbook_enabled,
                )
            })
        });
        if let Some((m, hf, hs, ob)) = saved {
            if m.is_some() || hf.is_some() || hs.is_some() {
                panel.update(cx, |p, pcx| p.set_layout(m, hf, hs, pcx));
            }
            if ob.is_some() {
                panel.update(cx, |p, pcx| p.set_orderbook_enabled(ob, pcx));
            }
        }
        let layout_fit_input = cx.new(|cx| MoonInputState::new(window, cx));
        let layout_scroll_input = cx.new(|cx| MoonInputState::new(window, cx));
        cx.subscribe(
            &layout_fit_input,
            |this, _input, ev: &MoonInputEvent, cx| {
                if this.layout_popup_open
                    && matches!(ev, MoonInputEvent::Blur | MoonInputEvent::PressEnter { .. })
                {
                    this.commit_layout_popup(cx);
                }
            },
        )
        .detach();
        cx.subscribe(
            &layout_scroll_input,
            |this, _input, ev: &MoonInputEvent, cx| {
                if this.layout_popup_open
                    && matches!(ev, MoonInputEvent::Blur | MoonInputEvent::PressEnter { .. })
                {
                    this.commit_layout_popup(cx);
                }
            },
        )
        .detach();
        Self {
            panel,
            backend,
            group,
            num,
            bucket,
            persist_armed: !restored,
            restore_size,
            taskbar_hide_ticks: 8,
            layout_popup_open: false,
            layout_popup_hovered: false,
            layout_fit_input,
            layout_scroll_input,
        }
    }

    /// Текущая per-tab раскладка панели этого окна: `(mode, height_fit, height_scroll)`.
    fn panel_layout(&self, cx: &App) -> (Option<StackLayoutMode>, Option<u16>, Option<u16>) {
        let p = self.panel.read(cx);
        (
            p.layout_mode(),
            p.layout_height_fit(),
            p.layout_height_scroll(),
        )
    }

    /// Открыть/закрыть in-scene popup раскладки этой вкладки.
    fn toggle_layout_popup(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.layout_popup_open {
            self.close_layout_popup(true, cx);
        } else {
            self.seed_layout_popup_inputs(window, cx);
            self.layout_popup_open = true;
            self.layout_popup_hovered = false;
            cx.notify();
        }
    }

    fn seed_layout_popup_inputs(&self, window: &mut Window, cx: &mut Context<Self>) {
        // Эффективные значения вместо пустоты при None (Fit→0, Scroll→дефолт) — иначе после
        // рестарта поля высоты пустые, без цифр.
        let (_, hf, hs) = self.panel_layout(cx);
        let fit = hf.unwrap_or(0).to_string();
        let scroll = hs
            .unwrap_or(super::stack::DEFAULT_SCROLL_HEIGHT)
            .to_string();
        self.layout_fit_input
            .update(cx, |input, c| input.set_value(fit, window, c));
        self.layout_scroll_input
            .update(cx, |input, c| input.set_value(scroll, window, c));
    }

    fn read_layout_height(&self, mode: StackLayoutMode, cx: &App) -> Option<u16> {
        let (_, fit_fallback, scroll_fallback) = self.panel_layout(cx);
        let (input, fallback) = match mode {
            StackLayoutMode::Fit => (&self.layout_fit_input, fit_fallback),
            StackLayoutMode::Scroll => (&self.layout_scroll_input, scroll_fallback),
        };
        let value = input.read(cx).value().to_string();
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return None;
        }
        trimmed
            .parse::<u16>()
            .ok()
            .map(|raw| layout_popup::clamp_height(mode, raw))
            .or(fallback)
    }

    fn commit_layout_popup(&mut self, cx: &mut Context<Self>) {
        let (mode, _, _) = self.panel_layout(cx);
        let hf = self.read_layout_height(StackLayoutMode::Fit, cx);
        let hs = self.read_layout_height(StackLayoutMode::Scroll, cx);
        self.apply_layout(Some(mode.unwrap_or(StackLayoutMode::Fit)), hf, hs, cx);
    }

    fn close_layout_popup(&mut self, commit: bool, cx: &mut Context<Self>) {
        if !self.layout_popup_open {
            return;
        }
        if commit {
            self.commit_layout_popup(cx);
        }
        self.layout_popup_open = false;
        self.layout_popup_hovered = false;
        cx.notify();
    }

    fn apply_layout_to_all_charts(
        &mut self,
        mode: Option<StackLayoutMode>,
        height_fit: Option<u16>,
        height_scroll: Option<u16>,
        cx: &mut Context<Self>,
    ) {
        let group = self.group.clone();
        // Копируем ВСЕ настройки этого окна: + масштаб + галку стакана.
        let scale = self.panel.read(cx).scale();
        let orderbook = Some(self.panel.read(cx).orderbook_enabled().unwrap_or(true));
        self.backend.update(cx, |bk, bcx| {
            bk.chart_apply_all.push(crate::ChartApplyAll {
                group,
                include_main: false,
                mode,
                height_fit,
                height_scroll,
                scale,
                orderbook,
            });
            bcx.notify();
        });
    }

    /// Применить раскладку к панели вкладки и сохранить в charts.json.
    fn apply_layout(
        &mut self,
        mode: Option<StackLayoutMode>,
        height_fit: Option<u16>,
        height_scroll: Option<u16>,
        cx: &mut Context<Self>,
    ) {
        self.panel
            .update(cx, |p, c| p.set_layout(mode, height_fit, height_scroll, c));
        let (group, num, bucket) = (self.group.clone(), self.num, self.bucket.clone());
        self.backend.update(cx, |bk, _| {
            if let Some(s) = bk
                .chart_specs
                .iter_mut()
                .find(|s| s.group == group && s.num == num && s.bucket() == bucket)
            {
                s.layout_mode = mode;
                s.layout_height_fit = height_fit;
                s.layout_height_scroll = height_scroll;
            } else {
                bk.chart_specs.push(chart_persist::ChartTabSpec {
                    group,
                    num,
                    core: None,
                    bucket: Some(bucket),
                    scale: None,
                    detached: None,
                    layout_mode: mode,
                    layout_height_fit: height_fit,
                    layout_height_scroll: height_scroll,
                    orderbook_enabled: None,
                });
            }
            bk.chart_specs_dirty = true;
        });
        cx.notify();
    }

    /// Вкл/выкл стакан этой вкладки + persist + пересбор набора рынков, которым нужен стакан.
    fn apply_orderbook(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.panel
            .update(cx, |p, c| p.set_orderbook_enabled(Some(enabled), c));
        let (group, num, bucket) = (self.group.clone(), self.num, self.bucket.clone());
        self.backend.update(cx, |bk, _| {
            if let Some(s) = bk
                .chart_specs
                .iter_mut()
                .find(|s| s.group == group && s.num == num && s.bucket() == bucket)
            {
                s.orderbook_enabled = Some(enabled);
            } else {
                bk.chart_specs.push(chart_persist::ChartTabSpec {
                    group,
                    num,
                    core: None,
                    bucket: Some(bucket),
                    scale: None,
                    detached: None,
                    layout_mode: None,
                    layout_height_fit: None,
                    layout_height_scroll: None,
                    orderbook_enabled: Some(enabled),
                });
            }
            bk.chart_specs_dirty = true;
            bk.rebuild_orderbook_wanted();
        });
        cx.notify();
    }

    fn persist_geometry(&mut self, window: &Window, cx: &mut Context<Self>) {
        // У восстановленного окна сохранение задержано до `persist_armed`: не даём стартовому
        // авто-размещению GPUI/Win32 перезаписать сохранённую позицию DPI-мусором.
        if !self.persist_armed {
            return;
        }
        let Some((x, y, w, h)) = crate::windowing::window_geom(window) else {
            moon_core::detect_diag::line(&format!(
                "[geom] n={} НЕ Windowed → геометрия не сохранена",
                self.num
            ));
            return;
        };
        let geom = chart_persist::WinGeom { x, y, w, h };
        let (group, num, bucket) = (self.group.clone(), self.num, self.bucket.clone());
        let found = self.backend.update(cx, |bk, _| {
            if let Some(s) = bk
                .chart_specs
                .iter_mut()
                .find(|s| s.group == group && s.num == num && s.bucket() == bucket)
            {
                let cur = s.detached.map(|g| (g.x, g.y, g.w, g.h));
                if cur != Some((geom.x, geom.y, geom.w, geom.h)) {
                    s.detached = Some(geom);
                    bk.chart_specs_dirty = true;
                }
                true
            } else {
                false
            }
        });
        moon_core::detect_diag::line(&format!(
            "[geom] n={num} bucket={bucket:?} → x={} y={} w={} h={} (spec_found={found})",
            geom.x, geom.y, geom.w, geom.h
        ));
    }
}

impl Render for DetachedChartHost {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Коррекция размера восстановленного окна (один раз): окно уже на целевом мониторе с
        // верным scale → форсим сохранённый логический размер, перебивая DPICHANGED-сжатие.
        if let Some(sz) = self.restore_size.take() {
            window.resize(sz);
        }
        // Убрать кнопку из таскбара (DeleteTab), оставив окно independent → FancyZones его видит.
        // Несколько первых рендеров — на случай, если кнопка появляется чуть позже показа окна.
        if self.taskbar_hide_ticks > 0 {
            crate::windowing::hide_window_from_taskbar(window);
            self.taskbar_hide_ticks -= 1;
        }
        let p = MoonPalette::active(cx);
        // Масштаб — СВОЙ у этой панели (по-вкладочно), правится прямо в неё.
        let scale = self.panel.read(cx).scale();
        let panel = self.panel.clone();
        let close_all_panel = self.panel.clone();
        let title = chart_pane_label(&self.backend, &self.group, self.num, &self.bucket, cx);
        let frame = MoonWindowFrame::detached_chart("detached-chart-window-frame", 0.0)
            .header_height(34.0)
            .controls(MoonWindowFrameControls::Close)
            .show_controls(design::show_custom_window_controls());
        let popup_open = self.layout_popup_open;
        let layout_popup = self.layout_popup_open.then(|| {
            let mode = self.panel_layout(cx).0.unwrap_or(StackLayoutMode::Fit);
            let orderbook_enabled = self.panel.read(cx).orderbook_enabled().unwrap_or(true);
            let pick_entity = cx.entity();
            let all_entity = cx.entity();
            let ob_entity = cx.entity();
            let hover_entity = cx.entity();
            let size = layout_popup::content_size(cx);
            div()
                .id("detached-chart-layout-popup-scene")
                .absolute()
                .right(px(6.0))
                .top(px(38.0))
                .w(size.width)
                .h(size.height)
                .on_mouse_down(MouseButton::Left, |_, _window, app| {
                    app.stop_propagation();
                })
                .on_hover(move |hovered, _window, app| {
                    hover_entity.update(app, |this, cx| {
                        if *hovered {
                            this.layout_popup_hovered = true;
                        } else if this.layout_popup_hovered {
                            this.close_layout_popup(true, cx);
                        }
                    });
                })
                .child(layout_popup::render_layout_popup(
                    "detached-chart-layout",
                    mode,
                    &self.layout_fit_input,
                    &self.layout_scroll_input,
                    orderbook_enabled,
                    p,
                    cx,
                    move |mode, app| {
                        pick_entity.update(app, |this, cx| {
                            let hf = this.read_layout_height(StackLayoutMode::Fit, cx);
                            let hs = this.read_layout_height(StackLayoutMode::Scroll, cx);
                            this.apply_layout(Some(mode), hf, hs, cx);
                        });
                    },
                    t!("chart.layout.apply_all_charts").to_string(),
                    move |app| {
                        all_entity.update(app, |this, cx| {
                            let (mode, _, _) = this.panel_layout(cx);
                            let hf = this.read_layout_height(StackLayoutMode::Fit, cx);
                            let hs = this.read_layout_height(StackLayoutMode::Scroll, cx);
                            this.apply_layout_to_all_charts(
                                Some(mode.unwrap_or(StackLayoutMode::Fit)),
                                hf,
                                hs,
                                cx,
                            );
                        });
                    },
                    move |checked, app| {
                        ob_entity.update(app, |this, cx| this.apply_orderbook(checked, cx));
                    },
                ))
        });
        let layout_dismiss = self.layout_popup_open.then(|| {
            let entity = cx.entity();
            div()
                .id("detached-chart-layout-popup-dismiss")
                .absolute()
                .inset_0()
                .on_mouse_down(MouseButton::Left, move |_, _window, app| {
                    entity.update(app, |this, cx| this.close_layout_popup(true, cx));
                    app.stop_propagation();
                })
        });
        // Шапка — ТОЛЬКО у выносных окон вкладок (в основном доке её нет): масштаб слева,
        // «закрыть все графики» справа.
        v_flex()
            .size_full()
            .relative()
            .child(
                h_flex()
                    .h(design::fit_h_px(cx, 34.0, 13.0, 10.5))
                    .w_full()
                    .items_center()
                    .gap(design::ui_px(cx, 8.0))
                    .pl(design::ui_px(cx, design::titlebar_leading_inset()))
                    .pr(design::ui_px(cx, 6.0))
                    .border_b_1()
                    .border_color(rgb(p.border))
                    .bg(rgb(p.shell_high))
                    .child(
                        frame
                            .title_cluster(title, cx)
                            .h_full()
                            .flex_1()
                            .min_w_0()
                            .items_center(),
                    )
                    .child(crate::controls::scale_dropdown_for_add_stack(
                        scale,
                        panel.clone(),
                        p,
                    ))
                    .child({
                        let entity = cx.entity();
                        div().relative().child(
                            MoonButton::new("detached-layout-settings")
                                .label("⚙")
                                .tooltip(t!("chart.layout.tip").to_string())
                                .size(MoonButtonSize::Micro)
                                .variant(if popup_open {
                                    MoonButtonVariant::Blue
                                } else {
                                    MoonButtonVariant::Ghost
                                })
                                .selected(popup_open)
                                .on_click(move |_, window, app| {
                                    entity.update(app, |this, cx| {
                                        this.toggle_layout_popup(window, cx)
                                    });
                                })
                                .render(),
                        )
                    })
                    .child(
                        MoonButton::new("detached-close-all")
                            .label("🗑")
                            .tooltip(t!("chartwin.clear").to_string())
                            .size(MoonButtonSize::Micro)
                            .variant(MoonButtonVariant::Ghost)
                            .on_click(move |_, _w, app| {
                                close_all_panel.update(app, |p, cx| p.close_all_panes(cx));
                            })
                            .render(),
                    )
                    .when(design::show_custom_window_controls(), |this| {
                        this.child(frame.visual_controls(cx))
                    }),
            )
            .child(
                div()
                    .flex_1()
                    .w_full()
                    .overflow_hidden()
                    // БЕЗ .bg(): own-pass чарта и его text layer лежат under-scene, любой
                    // непрозрачный фон тела перекроет график. Подложку под/между чартами закрывает
                    // тёмный clear окна (правка форка MoonUI), белого нет.
                    .child(self.panel.clone()),
            )
            .children(layout_dismiss)
            .children(layout_popup)
    }
}
