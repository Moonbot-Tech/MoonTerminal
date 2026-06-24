//! Оболочка одной группы (Shell): одно ОС-окно = header + единый `DockArea` + статус-бар.
//! Вынесено из main.rs. `Backend` живёт в крейт-руте — доступ к его приватным полям из
//! этого модуля сохраняется (правило: потомок видит приватное предка).
//!
//! Разнесено по подмодулям:
//! - [`metrics`] — попапы торговых метрик тулбара (TP/SL/Lev) и коммит правок в ядро;
//! - [`docks`] — отцепление/возврат панелей и персист геометрии ОС-окна;
//! - [`status_bar`] — нижняя строка состояния (соединение/лицензия/диагностика).

mod docks;
mod metrics;
mod status_bar;

use std::rc::Rc;
use std::time::Instant;

use gpui::*;

use moon_ui::{
    DockArea, DockEvent, DockItem, MoonBackgroundPolicy, MoonInputEvent, MoonInputState,
    MoonPalette, MoonSliderEvent, MoonSliderState, MoonWindowFrame, PanelView, v_flex,
};

use moon_core::feed::{ClientSettingsEdit, LevManageEdit};
use moon_core::session::CoreId;

use crate::chart_tabs::ChartTabs;
use crate::dock_persist::DOCK_VERSION;
use crate::panels::{AssetsView, DetectsPanel, LogPanel, OrderPanel, OrdersPanel, ReportPanel};
use crate::{Backend, controls, design, panels, terminal_chrome};

/// Оболочка одной группы (= одно ОС-окно): header + единый `DockArea` + статус.
/// Весь контент — Dock-панели (чарт=center, детекты/ордер=right, нижние вкладки=
/// bottom), перетаскиваемые/отцепляемые. Header/статус — фикс. полосы вокруг дока.
pub(crate) struct Shell {
    backend: Entity<Backend>,
    group: String,
    dock: Entity<DockArea>,
    /// Время прошлого кадра и сглаженный fps рендера — для статус-бара (как egui host).
    last_frame: Option<Instant>,
    fps: f32,
    /// Троттл observe-notify бэкенда: Shell-рендер обновляет лишь статус-бар (book/cpu/
    /// fps), его дёргать чаще ~4 Гц человеку незачем, а он тащит top-down тяжёлый Orders.
    last_notify: Option<Instant>,
    /// Прошлое виденное значение follow (Live/Пауза). Смена = клик юзера → отражаем кнопку
    /// тулбара мгновенно, мимо 250мс-троттла (иначе Live↔Пауза «залипает» до ¼с).
    last_follow: bool,
    /// Прошлое виденное значение масштаба. Это тоже клик юзера, а не фоновая телеметрия:
    /// тулбар должен менять подпись сразу, даже при троттле Shell observe.
    last_price_scale: Option<f32>,
    /// Прошлая виденная ревизия выбора размера ордера (F1-F6). Клик юзера → выбранную
    /// кнопку отражаем мгновенно, мимо 250мс-троттла (иначе selected «залипает» до ¼с).
    last_order_size_rev: u64,
    /// Handle своего ОС-окна. Нужен event/observe callbacks, где нет `&mut Window`,
    /// но нельзя переносить window-bound операции в `render()`.
    window_handle: AnyWindowHandle,
    /// Инпут инлайн-редактирования значения кнопки размера ордера (дабл-клик в тулбаре).
    /// Один на Shell, переиспользуется для любой F-кнопки.
    size_input: Entity<MoonInputState>,
    /// Что сейчас редактируется в тулбаре: `(ядро, индекс F1-F6)`. None = не редактируем.
    size_edit: Option<(CoreId, usize)>,
    /// Инпут инлайн-редактирования процента fixed-sell пресета (дабл-клик по S-кнопке) + что
    /// редактируется `(ядро, индекс S1-S6)`. По Blur/Enter шлём `SetFixedSellPct` в ядро.
    sell_input: Entity<MoonInputState>,
    sell_edit: Option<(CoreId, usize)>,
    /// Слайдер+поле попапов торговых метрик (TP/SL/Lev). Персистентны (значения переживают
    /// рендеры; при открытии попапа сидируются значением активного ядра). Коммит в ядро —
    /// подписками в `new`. Один набор на окно: одновременно открыт лишь один попап. У TP два
    /// слайдера (1..100 и 100..900) под флаг `x_tmode` — границы в рантайме не меняются.
    tp_slider_normal: Entity<MoonSliderState>,
    tp_slider_ext: Entity<MoonSliderState>,
    /// Файн-слайдер TP (суб-процент через scalp). Пересоздаётся при открытии TP-попапа с
    /// диапазоном 0..основной_TP (границы слайдера в рантайме не меняются).
    tp_fine_slider: Entity<MoonSliderState>,
    sl_slider: Entity<MoonSliderState>,
    lev_slider: Entity<MoonSliderState>,
    tp_input: Entity<MoonInputState>,
    sl_input: Entity<MoonInputState>,
    lev_input: Entity<MoonInputState>,
    /// Какой попап метрики тулбара открыт (TP/SL/Lev). Overlay рисуется поверх дока, закрытие
    /// по клику вне (dismiss-слой), уводу мыши или повторному клику по кнопке. None = закрыт.
    open_metric_popup: Option<controls::TradeMetric>,
    /// Был ли курсор уже над попапом метрики (как `layout_popup_hovered`): авто-выход по
    /// уводу мыши только после реального захода внутрь.
    metric_popup_hovered: bool,
    /// Фокус корня окна — чтобы хоткеи (`on_key_down` на корне) ловились даже когда ничего
    /// другого не сфокусировано (пустой Main). Фокусируем на старте; клик по чарту/инпуту
    /// уводит фокус туда, но F-клавиши всплывают обратно к корню.
    focus: FocusHandle,
}

impl Shell {
    pub(crate) fn new(
        backend: Entity<Backend>,
        group: String,
        focus: Option<(CoreId, String)>,
        epoch: f64,
        theme: moon_core::config::ChartTheme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let window_handle = window.window_handle();
        // Единый DockArea на окно. Панели: чарт=center, детекты+ордер=right (split),
        // нижние вкладки=bottom. Dock/TabPanel — MoonPalette, чтобы фоны управлялись
        // MoonBackgroundPolicy и не перекрывали chart UnderScene.
        let dock = cx.new(|cx| {
            DockArea::new("group-dock", Some(DOCK_VERSION), window, cx)
                .background_policy(MoonBackgroundPolicy::NoFill)
                .tab_background_policy(MoonBackgroundPolicy::NoFill)
        });
        let weak = dock.downgrade();

        // Сохранённая раскладка этой группы (совместимой версии) → восстановить через
        // DockArea::load (панели пересоздаёт PanelRegistry по panel_name+группе). Иначе
        // строим дефолтную раскладку. Порт «сохранение всего» для доков.
        let saved = backend
            .read(cx)
            .dock_states
            .get(&group)
            .filter(|s| s.version == Some(DOCK_VERSION))
            .cloned();

        if let Some(state) = saved {
            dock.update(cx, |area, cx| {
                if let Err(e) = area.load(state, window, cx) {
                    log::warn!("не восстановил раскладку доков группы {group}: {e}");
                }
            });
        } else {
            // Чарт-вкладки (Main + AddToChart-N) — свой таб-стрип (chart_tabs.rs), полный
            // контроль активной вкладки/детача. Детекты/ордер/нижние — gpui-Dock-панели.
            let charts = cx.new(|cx| {
                ChartTabs::new(
                    backend.clone(),
                    group.clone(),
                    focus,
                    epoch,
                    theme.clone(),
                    window,
                    cx,
                )
            });
            let detects = cx.new(|cx| DetectsPanel::new(backend.clone(), group.clone(), cx));
            let order = cx.new(|cx| OrderPanel::new(backend.clone(), group.clone(), cx));

            // Нижние вкладки — собираем, ПРОПУСКАЯ откреплённые (их окна откроет старт):
            // панель убрана из дока при откреплении, dock_persist хранит док без неё.
            let detached_set: std::collections::HashSet<String> = backend
                .read(cx)
                .detached
                .iter()
                .filter(|s| s.group == group)
                .map(|s| s.panel.clone())
                .collect();
            let mut bottom_tabs: Vec<Rc<dyn PanelView>> = Vec::new();
            if !detached_set.contains("Orders") {
                bottom_tabs.push(Rc::new(
                    cx.new(|cx| OrdersPanel::new(backend.clone(), group.clone(), window, cx)),
                ));
            }
            if !detached_set.contains("Assets") {
                bottom_tabs.push(Rc::new(cx.new(|cx| {
                    AssetsView::restored_group(backend.clone(), group.clone(), window, cx)
                })));
            }
            if !detached_set.contains("Log") {
                bottom_tabs.push(Rc::new(
                    cx.new(|cx| LogPanel::new(backend.clone(), group.clone(), window, cx)),
                ));
            }
            if !detached_set.contains("Report") {
                bottom_tabs.push(Rc::new(
                    cx.new(|cx| ReportPanel::new(backend.clone(), group.clone(), window, cx)),
                ));
            }

            // ВСЁ — в center-сплите: размеры панелей меняются split-handle'ами,
            // tab-docking/drag-to-edge — отдельный следующий слой док-механики.
            // Чарт-вкладки слева, детекты+ордер стопкой справа (≈220px), нижние вкладки внизу.
            // Тулбар (Размеры/Продажа/Масштаб) — отдельная фикс. полоса в Shell::render, не док.
            let chart_item = DockItem::tab(charts, &weak, window, cx);
            let right = DockItem::v_split(
                vec![
                    DockItem::tab(detects, &weak, window, cx),
                    DockItem::tab(order, &weak, window, cx),
                ],
                &weak,
                window,
                cx,
            );
            let top = DockItem::split_with_sizes(
                Axis::Horizontal,
                vec![chart_item, right],
                vec![None, Some(px(220.0))],
                &weak,
                window,
                cx,
            );
            let bottom = DockItem::tabs(bottom_tabs, &weak, window, cx);
            let center = DockItem::split_with_sizes(
                Axis::Vertical,
                vec![top, bottom],
                vec![None, Some(px(220.0))],
                &weak,
                window,
                cx,
            );

            dock.update(cx, |area, cx| area.set_center(center, window, cx));
        }

        // Header/статус-бар читают backend; но это GPUI-перерисовка top-down → тащит тяжёлый
        // Orders. Данные статуса (book/cpu/fps) меняются ≤10 Гц, человеку хватает ≤4 Гц.
        // Троттлим notify до ≥250мс (Пример 5: не будить всю сцену общим молотком на каждый тик).
        cx.observe(&backend, |this, backend, cx| {
            crate::diag::bump(&crate::diag::SHELL_OBS_FIRE);
            this.drain_order_size_edit_request(cx);
            this.drain_sell_edit_request(cx);
            this.drain_repin_requests(cx);
            let now = Instant::now();
            // Follow/Live и Scale меняются по КЛИКУ юзера — отражаем мгновенно,
            // мимо 250мс-троттла.
            // Прочее (book/cpu/fps) меняется само и человеку хватает ≤4 Гц → троттлим.
            let (follow, price_scale, order_size_rev) = {
                let b = backend.read(cx);
                (b.follow, b.price_scale, b.order_size_rev)
            };
            let follow_changed = follow != this.last_follow;
            let scale_changed = price_scale != this.last_price_scale;
            let size_changed = order_size_rev != this.last_order_size_rev;
            this.last_follow = follow;
            this.last_price_scale = price_scale;
            this.last_order_size_rev = order_size_rev;
            let due = follow_changed
                || scale_changed
                || size_changed
                || this
                    .last_notify
                    .map(|t| now.duration_since(t).as_millis() >= 250)
                    .unwrap_or(true);
            if due {
                this.last_notify = Some(now);
                crate::diag::bump(&crate::diag::SHELL_OBS_NOTIFY);
                cx.notify();
            }
        })
        .detach();

        // Любое изменение раскладки доков (drag/split/resize/detach) → дамп в backend,
        // сохранение дебаунсит дренаж-таймер (docks.json). Порт персиста раскладки.
        cx.subscribe(&dock, |this, dock, event: &DockEvent, cx| {
            match event {
                DockEvent::DetachRequested { panel_name } => {
                    this.defer_detach_panel(panel_name.to_string(), cx);
                }
                DockEvent::PanelCloseRequested { panel_name } => {
                    this.defer_restore_closed_panel(panel_name.to_string(), cx);
                }
                DockEvent::LayoutChanged => {}
            }
            let state = dock.read(cx).dump(cx);
            let group = this.group.clone();
            this.backend.update(cx, |b, _| {
                b.dock_states.insert(group, state);
                b.dock_dirty = true;
            });
        })
        .detach();

        cx.observe_window_bounds(window, |this, window, cx| {
            this.persist_group_geometry(window, cx);
        })
        .detach();

        // Инпут инлайн-редактирования размера ордера (дабл-клик по кнопке F1-F6). По Blur
        // (клик вне) или Enter — пишем значение в `ServerConfig.order_sizes` фокусного ядра
        // и сохраняем на диск (config.save). Пустой/нечисловой ввод — отмена без записи.
        let size_input = cx.new(|cx| MoonInputState::new(window, cx));
        cx.subscribe(&size_input, |this, inp, ev: &MoonInputEvent, cx| {
            if !matches!(ev, MoonInputEvent::Blur | MoonInputEvent::PressEnter { .. }) {
                return;
            }
            let Some((core, ix)) = this.size_edit.take() else {
                return;
            };
            let raw = inp.read(cx).value().to_string();
            if let Ok(v) = raw.trim().replace(',', ".").parse::<f64>() {
                if v > 0.0 && ix < 6 {
                    this.backend.update(cx, |b, bcx| {
                        let base = b.session.core_base(core).unwrap_or("").to_string();
                        let mut saved = false;
                        if let Some(s) = b.config.servers.iter_mut().find(|s| s.id == core) {
                            let mut arr = s.order_sizes.unwrap_or_else(|| {
                                moon_core::config::servers::default_order_sizes(&base)
                            });
                            arr[ix] = v;
                            s.order_sizes = Some(arr);
                            saved = true;
                        }
                        if saved {
                            if let Err(e) = b.config.save() {
                                log::warn!("save order size failed: {e}");
                            }
                        }
                        bcx.notify();
                    });
                }
            }
            cx.notify();
        })
        .detach();

        // Инпут инлайн-редактирования процента fixed-sell пресета (дабл-клик по S-кнопке). По
        // Blur/Enter шлём `SetFixedSellPct` активному ядру. Пустой/нечисловой ввод — отмена.
        let sell_input = cx.new(|cx| MoonInputState::new(window, cx));
        cx.subscribe(&sell_input, |this, inp, ev: &MoonInputEvent, cx| {
            if !matches!(ev, MoonInputEvent::Blur | MoonInputEvent::PressEnter { .. }) {
                return;
            }
            let Some((core, ix)) = this.sell_edit.take() else {
                return;
            };
            if let Ok(v) = inp.read(cx).value().trim().replace(',', ".").parse::<f64>() {
                if v >= 0.0 && ix < 6 {
                    this.backend.update(cx, |b, bcx| {
                        // Оптимистичный локальный кэш (живой дисплей) + отправка в ядро.
                        b.set_fixed_sell_pct_local(core, ix, v);
                        b.order_size_rev = b.order_size_rev.wrapping_add(1);
                        bcx.notify();
                        if let Err(error) = b.session.edit_client_settings(
                            core,
                            ClientSettingsEdit::SetFixedSellPct {
                                slot: ix + 1,
                                pct: v,
                            },
                        ) {
                            log::warn!("set fixed-sell pct failed: {error}");
                        }
                    });
                }
            }
            cx.notify();
        })
        .detach();

        // Попапы торговых метрик: слайдер (быстрый выбор) + поле (точный ввод). Границы — из
        // `controls` (по смыслу ядра). TP — два слайдера (обычный/расширенный под `x_tmode`).
        // Значение сидируется при открытии попапа (on_open_change), здесь — лишь дефолт.
        let mk_slider = |cx: &mut Context<Self>, (min, max, step): (f32, f32, f32), def: f32| {
            cx.new(|_| {
                MoonSliderState::new()
                    .min(min)
                    .max(max)
                    .step(step)
                    .default_value(def)
            })
        };
        let tp_slider_normal = mk_slider(cx, controls::TP_NORMAL, 1.0);
        let tp_slider_ext = mk_slider(cx, controls::TP_EXT, 100.0);
        // Файн-слайдер TP: фиксированный 0..2 (активен только когда верхний TP = 2).
        let tp_fine_slider = Self::make_tp_fine_slider(cx);
        let sl_slider = mk_slider(cx, controls::SL_BOUNDS, 0.0);
        let lev_slider = mk_slider(cx, controls::LEV_BOUNDS, 1.0);
        let tp_input = cx.new(|cx| MoonInputState::new(window, cx));
        let sl_input = cx.new(|cx| MoonInputState::new(window, cx));
        let lev_input = cx.new(|cx| MoonInputState::new(window, cx));

        // Слайдеры: на каждое изменение шлём правку активному ядру И обновляем поле попапа
        // (живой numeric-фидбэк). moonproto коалесит pending settings → драг не штормит провод.
        cx.subscribe(&tp_slider_normal, |this, _e, ev: &MoonSliderEvent, cx| {
            if let MoonSliderEvent::Change(v) = ev {
                let v = v.end();
                this.commit_client_edit(
                    ClientSettingsEdit::TakeProfit {
                        pct: v as f64,
                        extended: false,
                    },
                    cx,
                );
                this.live_set_field(this.tp_input.clone(), controls::fmt_field2(v), cx);
                // Верхний дошёл до минимума (2) → нижний (файн) становится активным и равным 2.
                if v <= controls::TP_FINE_MAX {
                    this.defer_set_slider(this.tp_fine_slider.clone(), controls::TP_FINE_MAX, cx);
                }
            }
        })
        .detach();
        cx.subscribe(&tp_slider_ext, |this, _e, ev: &MoonSliderEvent, cx| {
            if let MoonSliderEvent::Change(v) = ev {
                let v = v.end();
                this.commit_client_edit(
                    ClientSettingsEdit::TakeProfit {
                        pct: v as f64,
                        extended: true,
                    },
                    cx,
                );
                this.live_set_field(this.tp_input.clone(), controls::fmt_field2(v), cx);
            }
        })
        .detach();
        cx.subscribe(&sl_slider, |this, _e, ev: &MoonSliderEvent, cx| {
            if let MoonSliderEvent::Change(v) = ev {
                let v = v.end();
                this.commit_client_edit(ClientSettingsEdit::StopLossPct(v), cx);
                this.live_set_field(this.sl_input.clone(), controls::fmt_field2_signed(v), cx);
            }
        })
        .detach();
        cx.subscribe(&lev_slider, |this, _e, ev: &MoonSliderEvent, cx| {
            if let MoonSliderEvent::Change(v) = ev {
                let v = v.end();
                this.commit_lev_edit(LevManageEdit::FixLev(v as i32), cx);
                this.live_set_field(this.lev_input.clone(), format!("{}", v as i32), cx);
            }
        })
        .detach();

        // Поля ввода: коммит по Blur/Enter (точное значение). Пустой/нечисловой ввод — игнор.
        // TP читает текущий режим x_tmode активного ядра, чтобы отправить правку в тот же диапазон.
        cx.subscribe(&tp_input, |this, inp, ev: &MoonInputEvent, cx| {
            if !matches!(ev, MoonInputEvent::Blur | MoonInputEvent::PressEnter { .. }) {
                return;
            }
            if let Ok(v) = inp.read(cx).value().trim().replace(',', ".").parse::<f64>() {
                let extended = this.active_tp_extended(cx);
                this.commit_client_edit(ClientSettingsEdit::TakeProfit { pct: v, extended }, cx);
            }
        })
        .detach();
        cx.subscribe(&sl_input, |this, inp, ev: &MoonInputEvent, cx| {
            if !matches!(ev, MoonInputEvent::Blur | MoonInputEvent::PressEnter { .. }) {
                return;
            }
            if let Ok(v) = inp.read(cx).value().trim().replace(',', ".").parse::<f32>() {
                this.commit_client_edit(ClientSettingsEdit::StopLossPct(v), cx);
            }
        })
        .detach();
        cx.subscribe(&lev_input, |this, inp, ev: &MoonInputEvent, cx| {
            if !matches!(ev, MoonInputEvent::Blur | MoonInputEvent::PressEnter { .. }) {
                return;
            }
            if let Ok(v) = inp.read(cx).value().trim().parse::<i32>() {
                this.commit_lev_edit(LevManageEdit::FixLev(v), cx);
            }
        })
        .detach();

        // Фокус корня окна для хоткеев (см. поле `focus`). Фокусируем сразу, чтобы F-клавиши
        // работали даже при пустом Main (когда фокусировать в доке нечего).
        let focus = cx.focus_handle();
        window.focus(&focus, cx);

        Self {
            backend,
            group,
            dock,
            last_frame: None,
            fps: 0.0,
            last_notify: None,
            last_follow: true,
            last_price_scale: None,
            last_order_size_rev: 0,
            window_handle,
            size_input,
            size_edit: None,
            sell_input,
            sell_edit: None,
            tp_slider_normal,
            tp_slider_ext,
            tp_fine_slider,
            sl_slider,
            lev_slider,
            tp_input,
            sl_input,
            lev_input,
            open_metric_popup: None,
            metric_popup_hovered: false,
            focus,
        }
    }

    fn drain_order_size_edit_request(&mut self, cx: &mut Context<Self>) {
        let edit_req = self.backend.update(cx, |b, _| b.order_size_edit_req.take());
        let Some((core, ix)) = edit_req.filter(|(_, i)| *i < 6) else {
            return;
        };
        let cur = {
            let b = self.backend.read(cx);
            let base = b.session.core_base(core).unwrap_or("");
            b.config
                .servers
                .iter()
                .find(|s| s.id == core)
                .map(|s| s.order_sizes_or_default(base)[ix])
                .unwrap_or_else(|| moon_core::config::servers::default_order_sizes(base)[ix])
        };
        self.size_edit = Some((core, ix));
        let input = self.size_input.clone();
        let value = controls::fmt_adaptive(cur);
        let handle = self.window_handle;
        cx.defer(move |app| {
            let _ = handle.update(app, move |_, window, app| {
                input.update(app, |st, cx| {
                    st.set_value(value, window, cx);
                    st.focus(window, cx);
                });
            });
        });
    }

    /// Дабл-клик по S-кнопке: открыть инпут поверх неё с текущим процентом пресета и
    /// сфокусировать (аналог `drain_order_size_edit_request`, но значение — из ядра).
    fn drain_sell_edit_request(&mut self, cx: &mut Context<Self>) {
        let edit_req = self.backend.update(cx, |b, _| b.sell_edit_req.take());
        let Some((core, ix)) = edit_req.filter(|(_, i)| *i < 6) else {
            return;
        };
        let cur = self.backend.read(cx).fixed_sell_pct(core, ix);
        self.sell_edit = Some((core, ix));
        let input = self.sell_input.clone();
        let value = controls::fmt_adaptive(cur);
        let handle = self.window_handle;
        cx.defer(move |app| {
            let _ = handle.update(app, move |_, window, app| {
                input.update(app, |st, cx| {
                    st.set_value(value, window, cx);
                    st.focus(window, cx);
                });
            });
        });
    }
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::diag::bump(&crate::diag::SHELL_RENDER);
        let _order_count = panels::count_orders(self.backend.read(cx), &self.group);

        // Header-данные (рынок/цена/conn). Чарт/ввод/оси — в ChartPanel.
        // FPS рендера (сглаженный) — диагностика статус-бара (порт host.fps).
        let now_inst = Instant::now();
        if let Some(prev) = self.last_frame {
            let dt = now_inst.duration_since(prev).as_secs_f32().max(1e-4);
            self.fps = self.fps * 0.9 + (1.0 / dt) * 0.1;
        }
        self.last_frame = Some(now_inst);
        let fps = self.fps;

        let (conn, license, snap, book_levels) = {
            let b = self.backend.read(cx);
            let conn = b.session.conn_summary_group(&self.group);
            let license = b.session.license_summary_group(&self.group);
            let snap = b.snap;
            // Для статус-бара нужно лишь число уровней стакана текущего Main-чарта.
            let book_levels = match b.main_chart_target(&self.group) {
                Some((core, m)) => b.session.with_orderbook_view(core, &m, |data| {
                    data.map(|(book, _)| book.len()).unwrap_or(0)
                }),
                None => 0,
            };
            (conn, license, snap, book_levels)
        };
        let chrome_width = f32::from(window.viewport_size().width);
        let p = MoonPalette::active(cx);

        // Overlay-попап активной метрики тулбара (TP/SL/Lev): абсолютный бокс под кнопкой +
        // полноэкранный dismiss-слой (как попап раскладки чарта). Клик внутри не закрывает
        // (stop_propagation), клик вне или увод мыши — закрывает.
        let metric_overlay = self.open_metric_popup.map(|metric| {
            use controls::TradeMetric;
            let extended = self.active_tp_extended(cx);
            let (slider, input) = match metric {
                TradeMetric::Tp => (
                    if extended {
                        &self.tp_slider_ext
                    } else {
                        &self.tp_slider_normal
                    },
                    &self.tp_input,
                ),
                TradeMetric::Sl => (&self.sl_slider, &self.sl_input),
                TradeMetric::Lev => (&self.lev_slider, &self.lev_input),
            };
            let hedge_on = {
                let b = self.backend.read(cx);
                b.active_trade_core(&self.group)
                    .and_then(|c| b.session.store().core(c))
                    .and_then(|d| d.hedge_mode)
                    .unwrap_or(false)
            };
            let content = controls::metric_popup_content(
                metric,
                slider,
                &self.tp_fine_slider,
                input,
                extended,
                hedge_on,
                &self.backend,
                &self.group,
                p,
                cx,
            );
            let (left, top) = self.metric_popup_pos(metric, cx);
            div()
                .id("metric-popup")
                .absolute()
                .left(left)
                .top(top)
                // Клик/драг внутри попапа НЕ закрывает (иначе нельзя тянуть слайдер): гасим
                // на mouse_down, чтобы не дошло до dismiss-слоя. Закрытие — клик вне или по кнопке.
                .on_mouse_down(MouseButton::Left, |_, _w, app| app.stop_propagation())
                // Авто-выход по уводу мыши — НО не во время drag слайдера: gpui на время
                // `on_drag` слайдера гасит hover родителя (hovered=false), и без этой проверки
                // попап закрывался бы прямо при перетаскивании ползунка. `has_active_drag()` —
                // штатный публичный запрос gpui (форк править не нужно).
                .on_hover(cx.listener(|this, hovered: &bool, _w, cx| {
                    if *hovered {
                        this.metric_popup_hovered = true;
                    } else if this.metric_popup_hovered && !cx.has_active_drag() {
                        this.close_metric_popup(cx);
                    }
                }))
                .child(content)
        });
        let metric_dismiss = self.open_metric_popup.map(|_| {
            div()
                .id("metric-popup-dismiss")
                .absolute()
                .inset_0()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _ev, _w, cx| {
                        this.close_metric_popup(cx);
                        cx.stop_propagation();
                    }),
                )
        });

        v_flex()
            .size_full()
            .relative() // для absolute-позиционирования демо-попапа поверх дока
            // Фокусируемый корень → хоткеи (`on_key_down`) ловятся даже при пустом Main.
            .track_focus(&self.focus)
            // НЕТ корневого .bg(): чарт-регион (центр дока) держим прозрачным «окном» под
            // own-pass (UnderScene). Хром (хедер/тулбар/панели/статус) красит свой фон сам.
            .font_family(design::mono())
            .text_color(rgb(p.text))
            .text_size(design::t_body(cx))
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _window, cx| {
                let group = this.group.clone();
                let handled = this.backend.update(cx, |b, bcx| {
                    // Фаза 1 (только чтение cfg): какой хоткей совпал. Сравниваем нажатую
                    // клавишу с каждым настроенным сочетанием (gpui Keystroke).
                    let (size_ix, sell_ix, is_cancel) = {
                        let cfg = b.preview.as_ref().unwrap_or(&b.config);
                        let pressed = |raw: &str| {
                            let raw = raw.trim();
                            !raw.is_empty()
                                && matches!(Keystroke::parse(raw), Ok(k) if k == ev.keystroke)
                        };
                        let size_ix = cfg.hotkeys.order_size.iter().position(|r| pressed(r));
                        let sell_ix = cfg.hotkeys.sell_preset.iter().position(|r| pressed(r));
                        (size_ix, sell_ix, pressed(&cfg.hotkeys.cancel_buy))
                    };
                    // Фаза 2 (мутация): F1-F6 = выбрать пресет размера активного ядра; S1-S6 =
                    // выбрать fixed-sell слот (меняет TP); cancel_buy — отмена покупок Main.
                    if let Some(i) = size_ix {
                        match b.active_trade_core(&group) {
                            Some(core) => {
                                b.order_size_sel.insert(core, i);
                                b.order_size_rev = b.order_size_rev.wrapping_add(1);
                                bcx.notify();
                                true
                            }
                            None => false,
                        }
                    } else if let Some(i) = sell_ix {
                        match b.active_trade_core(&group) {
                            Some(core) => {
                                if let Err(error) = b.session.edit_client_settings(
                                    core,
                                    ClientSettingsEdit::SelectFixedSellSlot(i + 1),
                                ) {
                                    log::warn!("hotkey select fixed-sell slot failed: {error}");
                                }
                                true
                            }
                            None => false,
                        }
                    } else if is_cancel {
                        b.cancel_buy_for_main_chart(&group);
                        true
                    } else {
                        false
                    }
                });
                if handled {
                    cx.stop_propagation();
                }
            }))
            // ── Header ──────────────────────────────────────────────
            .child(terminal_chrome::header(
                &self.group,
                self.backend.clone(),
                p,
                cx,
            ))
            // ── Тулбар: тонкая фикс. полоса (Размеры/Продажа/Масштаб+Live), порт верхней
            //    полосы стенда. Не dock-панель — единый ряд на высоту кнопки. ──
            .child(controls::toolbar(
                &self.backend,
                &self.group,
                self.size_edit,
                &self.size_input,
                self.sell_edit,
                &self.sell_input,
                &cx.entity(),
                self.open_metric_popup,
                cx,
            ))
            // ── Центр: единый DockArea (чарт=center, детекты+ордер=right, вкладки=bottom) ──
            .child(
                div()
                    .relative()
                    .flex_1()
                    .w_full()
                    .min_h(px(0.0))
                    .overflow_hidden()
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .right_0()
                            .bottom_0()
                            .left_0()
                            .child(self.dock.clone()),
                    ),
            )
            // ── Status bar (полный порт egui `shell::ui` нижней панели) ──
            .child(self.status_bar(conn, license, snap, book_levels, fps, cx))
            .child(
                MoonWindowFrame::main("moon-main-window-frame", chrome_width)
                    .header_height(design::HEADER_TOP_H)
                    .leading_inset(design::titlebar_leading_inset())
                    .show_controls(design::show_custom_window_controls())
                    .hit_overlay(),
            )
            // Попап метрики поверх всего: dismiss-слой (ловит клик вне) под самим попапом.
            .children(metric_dismiss)
            .children(metric_overlay)
    }
}
