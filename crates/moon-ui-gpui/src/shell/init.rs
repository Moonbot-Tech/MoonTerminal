//! Конструктор [`Shell::new`]: сборка дока/панелей окна группы, observe/subscribe-плумбинг
//! (инпуты, слайдеры, док-события, активация окна) + `wire_metric_subscriptions`.
//! Вынесено из `shell/mod.rs` точь-в-точь.

use std::rc::Rc;
use std::time::Instant;

use gpui::*;
use rust_i18n::t;

use moon_ui::{
    DockArea, DockEvent, DockItem, MoonBackgroundPolicy, MoonInputEvent, MoonInputState,
    MoonSliderEvent, MoonSliderState, PanelView,
};

use moon_core::feed::ClientSettingsEdit;
use moon_core::session::CoreId;

use super::Shell;
use crate::chart_tabs::ChartTabs;
use crate::dock_persist::DOCK_VERSION;
use crate::panels::{AssetsView, DetectsPanel, LogPanel, OrdersPanel, ReportPanel};
use crate::{Backend, controls, core_settings_popup};

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
            if !detached_set.contains("Alerts") {
                bottom_tabs.push(Rc::new(cx.new(|cx| {
                    crate::panels::AlertsPanel::new(backend.clone(), group.clone(), window, cx)
                })));
            }

            // ВСЁ — в center-сплите: размеры панелей меняются split-handle'ами,
            // tab-docking/drag-to-edge — отдельный следующий слой док-механики.
            // Чарт-вкладки слева, детекты справа (≈220px), нижние вкладки внизу.
            // Тулбар (Размеры/Продажа/Масштаб) — отдельная фикс. полоса в Shell::render, не док.
            let chart_item = DockItem::tab(charts, &weak, window, cx);
            let right = DockItem::tab(detects, &weak, window, cx);
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
            this.drain_engine_action_toasts(cx);
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

        // Тик часов шапки: раз в секунду будим Shell-рендер, чтобы «HH:MM:SS» шли даже в
        // простое (backend-notify гейтится наличием данных → без этого часы бы замирали).
        // Один таймер на окно; 1 Гц ≤ штатного троттла статус-бара (4 Гц) — дёшево. Останов —
        // по смерти сущности (окно закрыто).
        cx.spawn(async move |this, cx| {
            loop {
                let executor = cx.update(|cx| cx.background_executor().clone());
                executor.timer(std::time::Duration::from_secs(1)).await;
                let alive = cx.update(|cx| {
                    this.update(cx, |_this, cx| {
                        crate::diag::bump(&crate::diag::CLOCK_NOTIFY);
                        cx.notify();
                    })
                    .is_ok()
                });
                if !alive {
                    break;
                }
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

        // Активность окна для авто-закрытия Main по неактивности: пока окно НЕ в фокусе,
        // движение мыши над ним не считается активностью (таймер неактивности тикает). При
        // получении фокуса засчитываем активность, чтобы не закрыть графики сразу после возврата.
        cx.observe_window_activation(window, |this, window, cx| {
            this.window_active = window.is_window_active();
            if this.window_active {
                let group = this.group.clone();
                this.backend.update(cx, |b, _| b.note_main_input(&group));
            }
        })
        .detach();

        // Инпут инлайн-редактирования размера ордера (дабл-клик по кнопке F1-F6). По Blur
        // (клик вне) или Enter — пишем значение в `ServerConfig.order_sizes` фокусного ядра
        // и сохраняем на диск (config.save). Пустой/нечисловой ввод — отмена без записи.
        // Каждый ВАЛИДНЫЙ кейстрок коммитится сразу (дебаунс-сейв через config_dirty):
        // покупка хоткеем/кликом до Enter должна брать УЖЕ набранное значение.
        let size_input = cx.new(|cx| MoonInputState::new(window, cx));
        cx.subscribe(&size_input, |this, inp, ev: &MoonInputEvent, cx| {
            if matches!(ev, MoonInputEvent::Change) {
                let Some((core, ix)) = this.size_edit else {
                    return;
                };
                if let Ok(v) = inp.read(cx).value().trim().replace(',', ".").parse::<f64>() {
                    if v > 0.0 && ix < 6 {
                        this.backend.update(cx, |b, bcx| {
                            b.set_order_size_value(core, ix, v);
                            b.order_size_rev = b.order_size_rev.wrapping_add(1);
                            bcx.notify();
                        });
                    }
                }
                return;
            }
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
        let gtp_slider = mk_slider(cx, core_settings_popup::CORE_GTP_BOUNDS, 0.5);
        let trailing_slider = mk_slider(cx, core_settings_popup::CORE_TRAILING_BOUNDS, -0.1);
        let vstop_slider = mk_slider(cx, core_settings_popup::CORE_VSTOP_BOUNDS, 0.0);
        let gtp_input = cx.new(|cx| MoonInputState::new(window, cx));
        let trailing_input = cx.new(|cx| MoonInputState::new(window, cx));
        let vstop_input = cx.new(|cx| MoonInputState::new(window, cx));
        let blacklist_input = cx.new(|cx| MoonInputState::new(window, cx));
        let def_strategy_input = cx.new(|cx| {
            MoonInputState::new(window, cx)
                .placeholder(t!("core_settings.def_strategy_search").to_string())
        });
        // Ввод в поле поиска стратегии → перерисовать попап (пере-фильтровать список).
        cx.subscribe(&def_strategy_input, |_this, _, ev: &MoonInputEvent, cx| {
            if matches!(ev, MoonInputEvent::Change) {
                cx.notify();
            }
        })
        .detach();
        // Multi-line от рождения; Enter коммитит (submit), а не вставляет перенос строки.
        let blacklist_area = cx.new(|cx| {
            MoonInputState::new(window, cx)
                .multi_line(true)
                .submit_on_enter(true)
        });
        let ticker_input = cx.new(|cx| MoonInputState::new(window, cx).placeholder("BTC…"));
        // Ввод в поиске тикера — только перерисовать попап (список считается в layers).
        cx.subscribe(&ticker_input, |_this, _inp, ev: &MoonInputEvent, cx| {
            if matches!(ev, MoonInputEvent::Change) {
                cx.notify();
            }
        })
        .detach();

        // Слайдеры/поля метрик: на изменение шлём правку активному ядру и держим numeric-поле
        // попапа в синхроне. Вынесено в `wire_metric_subscriptions` — в `new` это ~80 строк
        // повторяющегося плумбинга подписок.
        Self::wire_metric_subscriptions(
            cx,
            &tp_slider_normal,
            &tp_slider_ext,
            &sl_slider,
            &lev_slider,
            &tp_input,
            &sl_input,
            &lev_input,
        );

        // Поля попапа настроек ядра: коммит по Blur/Enter. Порог паники = `price_drop_level`
        // Глобальный TP пишем как `GlobalTakeProfit { on: true, pct }` (поле подразумевает
        // включённый глоб-TP); трейлинг — `TrailingDrop`. Пустой/нечисловой ввод — игнор.
        cx.subscribe(&gtp_input, |this, inp, ev: &MoonInputEvent, cx| {
            if !matches!(ev, MoonInputEvent::Blur | MoonInputEvent::PressEnter { .. }) {
                return;
            }
            if let Ok(v) = inp.read(cx).value().trim().replace(',', ".").parse::<f64>() {
                this.commit_client_edit(
                    ClientSettingsEdit::GlobalTakeProfit { on: true, pct: v },
                    cx,
                );
            }
        })
        .detach();
        cx.subscribe(&trailing_input, |this, inp, ev: &MoonInputEvent, cx| {
            if !matches!(ev, MoonInputEvent::Blur | MoonInputEvent::PressEnter { .. }) {
                return;
            }
            if let Ok(v) = inp.read(cx).value().trim().replace(',', ".").parse::<f32>() {
                this.commit_client_edit(ClientSettingsEdit::TrailingDrop(v), cx);
            }
        })
        .detach();

        // Слайдеры попапа настроек ядра: на изменение шлём правку активному ядру и живо обновляем
        // соответствующее поле (как у метрик-попапов тулбара).
        cx.subscribe(&gtp_slider, |this, _e, ev: &MoonSliderEvent, cx| {
            if let MoonSliderEvent::Change(v) = ev {
                let v = v.end();
                this.commit_client_edit(
                    ClientSettingsEdit::GlobalTakeProfit {
                        on: true,
                        pct: v as f64,
                    },
                    cx,
                );
                this.live_set_field(this.gtp_input.clone(), controls::fmt_field2(v), cx);
            }
        })
        .detach();
        cx.subscribe(&trailing_slider, |this, _e, ev: &MoonSliderEvent, cx| {
            if let MoonSliderEvent::Change(v) = ev {
                let v = v.end();
                this.commit_client_edit(ClientSettingsEdit::TrailingDrop(v), cx);
                this.live_set_field(
                    this.trailing_input.clone(),
                    controls::fmt_field2_signed(v),
                    cx,
                );
            }
        })
        .detach();
        // V-Stop (vol_drop_level, целое %): слайдер → правка + целочисленное поле.
        cx.subscribe(&vstop_slider, |this, _e, ev: &MoonSliderEvent, cx| {
            if let MoonSliderEvent::Change(v) = ev {
                let n = v.end().round() as i32;
                this.commit_client_edit(ClientSettingsEdit::VolDropLevel(n), cx);
                this.live_set_field(this.vstop_input.clone(), format!("{n}"), cx);
            }
        })
        .detach();
        cx.subscribe(&vstop_input, |this, inp, ev: &MoonInputEvent, cx| {
            if !matches!(ev, MoonInputEvent::Blur | MoonInputEvent::PressEnter { .. }) {
                return;
            }
            if let Ok(n) = inp.read(cx).value().trim().parse::<i32>() {
                this.commit_client_edit(ClientSettingsEdit::VolDropLevel(n), cx);
            }
        })
        .detach();
        // Текст чёрного списка: коммит по Blur/Enter (одна логика для однострочного поля и
        // развёрнутого multi-line редактора). Флаг вкл берём текущий у активного ядра.
        let commit_bl = |this: &mut Self,
                         inp: Entity<MoonInputState>,
                         ev: &MoonInputEvent,
                         cx: &mut Context<Self>| {
            if !matches!(ev, MoonInputEvent::Blur | MoonInputEvent::PressEnter { .. }) {
                return;
            }
            let text = inp.read(cx).value().to_string();
            this.commit_blacklist_text(text, cx);
        };
        cx.subscribe(
            &blacklist_input,
            move |this, inp, ev: &MoonInputEvent, cx| commit_bl(this, inp, ev, cx),
        )
        .detach();
        cx.subscribe(
            &blacklist_area,
            move |this, inp, ev: &MoonInputEvent, cx| commit_bl(this, inp, ev, cx),
        )
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
            gtp_slider,
            trailing_slider,
            vstop_slider,
            gtp_input,
            trailing_input,
            vstop_input,
            blacklist_input,
            blacklist_area,
            def_strategy_input,
            open_metric_popup: None,
            metric_popup_hovered: false,
            focus,
            window_active: true,
            core_settings_open: false,
            core_settings_cancel_confirm: false,
            core_settings_bl_expanded: false,
            ticker_popup_open: false,
            ticker_popup_hovered: false,
            ticker_input,
        }
    }

    /// Подписки слайдеров/полей попапов торговых метрик (TP/SL/Lev): на каждое изменение
    /// шлём правку активному ядру и обновляем numeric-поле попапа. moonproto коалесит pending
    /// settings → драг не штормит провод. Регистрируется из `new` (self ещё строится, поэтому
    /// сущности приходят параметрами; `this` в замыканиях даёт сам `cx.subscribe`).
    fn wire_metric_subscriptions(
        cx: &mut Context<Self>,
        tp_slider_normal: &Entity<MoonSliderState>,
        tp_slider_ext: &Entity<MoonSliderState>,
        sl_slider: &Entity<MoonSliderState>,
        lev_slider: &Entity<MoonSliderState>,
        tp_input: &Entity<MoonInputState>,
        sl_input: &Entity<MoonInputState>,
        lev_input: &Entity<MoonInputState>,
    ) {
        cx.subscribe(tp_slider_normal, |this, _e, ev: &MoonSliderEvent, cx| {
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
        cx.subscribe(tp_slider_ext, |this, _e, ev: &MoonSliderEvent, cx| {
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
        cx.subscribe(sl_slider, |this, _e, ev: &MoonSliderEvent, cx| {
            if let MoonSliderEvent::Change(v) = ev {
                let v = v.end();
                this.commit_client_edit(ClientSettingsEdit::StopLossPct(v), cx);
                this.live_set_field(this.sl_input.clone(), controls::fmt_field2_signed(v), cx);
            }
        })
        .detach();
        cx.subscribe(lev_slider, |this, _e, ev: &MoonSliderEvent, cx| {
            // Плечо НЕ применяем на драг (биржевое действие) — только живой фидбэк в поле.
            // Коммит идёт по кнопке «Применить» в попапе (читает значение из поля).
            if let MoonSliderEvent::Change(v) = ev {
                let v = v.end();
                this.live_set_field(this.lev_input.clone(), format!("{}", v as i32), cx);
            }
        })
        .detach();

        // Поля ввода: коммит по Blur/Enter (точное значение). Пустой/нечисловой ввод — игнор.
        // TP читает текущий режим x_tmode активного ядра, чтобы отправить правку в тот же диапазон.
        cx.subscribe(tp_input, |this, inp, ev: &MoonInputEvent, cx| {
            if !matches!(ev, MoonInputEvent::Blur | MoonInputEvent::PressEnter { .. }) {
                return;
            }
            if let Ok(v) = inp.read(cx).value().trim().replace(',', ".").parse::<f64>() {
                let extended = this.active_tp_extended(cx);
                this.commit_client_edit(ClientSettingsEdit::TakeProfit { pct: v, extended }, cx);
            }
        })
        .detach();
        cx.subscribe(sl_input, |this, inp, ev: &MoonInputEvent, cx| {
            if !matches!(ev, MoonInputEvent::Blur | MoonInputEvent::PressEnter { .. }) {
                return;
            }
            if let Ok(v) = inp.read(cx).value().trim().replace(',', ".").parse::<f32>() {
                this.commit_client_edit(ClientSettingsEdit::StopLossPct(v), cx);
            }
        })
        .detach();
        // Поле плеча НЕ коммитит само (ни Blur, ни Enter): плечо — биржевое действие, его
        // отправляет только кнопка «Применить» в попапе. Поле/слайдер — лишь выбор значения.
        let _ = lev_input;
    }
}
