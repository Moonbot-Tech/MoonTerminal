//! Сборка кадра окна группы (`impl Render for Shell`) + оверлей попапа торговой метрики.
//! Вынесено из `shell/mod.rs` точь-в-точь.

use std::time::Instant;

use gpui::*;

use moon_ui::{MoonPalette, MoonWindowFrame, v_flex};

use super::Shell;
use crate::{controls, design, terminal_chrome};

impl Shell {
    /// Слой попапа активной метрики тулбара (TP/SL/Lev): сам попап (absolute, под кнопкой) +
    /// полноэкранный dismiss-слой под ним. Возвращает `(попап, dismiss)` — оба `None`, если
    /// попап закрыт. Вынесено из `render` (там это ~70 строк сборки оверлея).
    fn metric_popup_layers(
        &self,
        p: MoonPalette,
        cx: &mut Context<Self>,
    ) -> (Option<AnyElement>, Option<AnyElement>) {
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
                .into_any_element()
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
                .into_any_element()
        });
        (metric_overlay, metric_dismiss)
    }
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::diag::bump(&crate::diag::SHELL_RENDER);

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
        let (metric_overlay, metric_dismiss) = self.metric_popup_layers(p, cx);

        // Попап настроек ядра — MoonPopover у кнопки ⚙ (контролируемый open в Shell):
        // контент строим только при открытом попапе, позиционирование к кнопке — от popover.
        let core_settings_content = self
            .core_settings_open
            .then(|| self.core_settings_popup_content(p, cx));

        // Тикер курса в шапке: сохранённый выбор или read-only дефолт. Render не мутирует backend.
        let ticker_sel = self.backend.read(cx).header_ticker();
        let (ticker_overlay, ticker_dismiss) = self.ticker_popup_layers(p, cx);

        // Активность Main для авто-закрытия по неактивности: ОКОННЫЙ слушатель ловит ВСЕ
        // движения мыши, в т.ч. над виджетами/панелями/чартом, которые блокируют hitbox
        // корня (там gated `.on_mouse_move` молчал — отсюда «график закрылся, хотя мышь
        // двигалась в окне»). Только при активном окне; без notify — это лишь отметка
        // времени (дёшево, хоть и часто).
        //
        // CAPTURE-фаза (а НЕ bubble): чарт-панель в своём элементном `.on_mouse_move` при
        // наведении зовёт `cx.stop_propagation()` (render.rs) — в bubble это гасит корневой
        // слушатель, и движение НАД ЧАРТОМ не считалось активностью → график закрывался, хотя
        // мышь по нему водили. Capture проходит до bubble и не подвержен его stop_propagation
        // (gpui window.rs: фазы идут capture→bubble на одном флаге `propagate_event`).
        {
            let backend = self.backend.clone();
            let group = self.group.clone();
            window.on_mouse_event::<MouseMoveEvent>(move |_e, phase, window, cx| {
                if phase == DispatchPhase::Capture && window.is_window_active() {
                    backend.update(cx, |b, _| b.note_main_input(&group));
                }
            });
        }

        v_flex()
            .size_full()
            .relative() // для absolute-позиционирования демо-попапа поверх дока
            // Фокусируемый корень → хоткеи (`on_key_down`) ловятся даже при пустом Main.
            .track_focus(&self.focus)
            // Активность для авто-закрытия Main по неактивности теперь пишет оконный
            // `on_mouse_event::<MouseMoveEvent>` выше (gated `.on_mouse_move` на корне не
            // ловил движение над блокирующими mouse виджетами).
            // НЕТ корневого .bg(): чарт-регион (центр дока) держим прозрачным «окном» под
            // own-pass (UnderScene). Хром (хедер/тулбар/панели/статус) красит свой фон сам.
            .font_family(design::mono())
            .text_color(rgb(p.text))
            .text_size(design::t_body(cx))
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _window, cx| this.on_hotkey(ev, cx)))
            // ── Header ──────────────────────────────────────────────
            .child(terminal_chrome::header(
                &self.group,
                self.backend.clone(),
                cx.entity(),
                ticker_sel,
                self.core_settings_open,
                core_settings_content,
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
            // Попап выбора источника тикера курса (клик по «1 BTC = …» в шапке).
            .children(ticker_dismiss)
            .children(ticker_overlay)
    }
}
