//! `impl Render for ChartPanel` — own-pass canvas под сценой + слой ввода (колесо/кнопки/
//! движение мыши/ховер) + GPUI-оверлеи (логотип пустого слота, FireTest-probe, риска зоны
//! управления, кнопки ✕/пин). Вынесено из `chart.rs` без изменения поведения.

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{MoonButton, MoonButtonSize, MoonButtonVariant, MoonPalette, MoonRect, rgba_from};

use moon_chart::paint::now_unix_ms;

use crate::axes;

use super::render_input;
use super::{ChartPanel, chart_bootstrap_present_rate_hz};
use crate::chart_persist::ChartBtnPos;

/// Тип кнопки рыночного действия в оверлее чарта.
#[derive(Clone, Copy)]
enum ActKind {
    CancelBuy,
    PanicSell,
}

/// Кнопка Cancel Buy / Panic Sell — общий конструктор per-pane ветки и
/// fullscreen-оверлея (label/variant/on_click идентичны; вызывающие добавляют
/// только id и `.full_width()`). Бренд-термины MoonBot — НЕ локализуем.
fn action_button(
    kind: ActKind,
    id: SharedString,
    armed: bool,
    backend: Entity<crate::Backend>,
    core: moon_core::session::CoreId,
    market: String,
) -> MoonButton {
    let (label, variant, selected) = match kind {
        ActKind::CancelBuy => ("Cancel Buy", MoonButtonVariant::Soft, false),
        ActKind::PanicSell => ("Panic Sell", MoonButtonVariant::Danger, armed),
    };
    MoonButton::new(id)
        .label(label)
        .size(MoonButtonSize::Micro)
        .variant(variant)
        .selected(selected)
        .on_click(move |_, _w, app| match kind {
            ActKind::CancelBuy => {
                let b = backend.read(app);
                if let Err(error) = b.session.cancel_market_buys(core, market.clone()) {
                    log::warn!("cancel market buys failed: {error:#}");
                }
            }
            ActKind::PanicSell => {
                backend.update(app, |b, cx| {
                    b.toggle_panic_sell(core, market.clone());
                    cx.notify();
                });
            }
        })
}

impl Render for ChartPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::diag::bump(&crate::diag::CHART_RENDER);
        let became_visible = !self.scene_visible;
        self.scene_visible = true;
        self.chart.set_scene_visible(true);
        self.chart
            .set_market_source(Some(self.backend.read(cx).session.market_source()));
        let ppp = window.scale_factor();
        // Запоминаем DPI для data prepare path (у него нет window). DPI меняется редко.
        self.last_ppp = ppp;
        self.chart.set_last_ppp(ppp);
        let palette = MoonPalette::active(cx);
        self.chart.set_ui_palette(palette);
        // Bootstrap only: chartdx refines this from real `gpu_canvas.frame()` cadence,
        // so macOS/Linux do not depend on this fallback staying exact forever.
        let monitor_rate_hz = chart_bootstrap_present_rate_hz();
        let fast_divisor = (monitor_rate_hz / 60.0).round().max(1.0) as u32;
        let effective_present_rate_hz = if self.fast {
            monitor_rate_hz / fast_divisor as f32
        } else {
            60.0
        };
        self.chart.set_present_rate_hz(effective_present_rate_hz);
        // ВАЖНО: НЕТ request_animation_frame/continuous-present. `gpu_canvas.frame()` решает
        // present на platform tick без dirty GPUI tree; `draw()` рисует в тот же tick.
        let (theme, orders_style, follow, prospective_usd, candle_view) = {
            let b = self.backend.read(cx);
            let eff = b.preview.as_ref().unwrap_or(&b.config);
            // Прогнозный размер ордера (s1-s6) активной монеты в $ — для подписи на перекрестии.
            let prospective = self
                .chart
                .active_target()
                .and_then(|(core, _)| b.prospective_order_usd(core));
            // Наборы темы чарта и стилей линий — по активной теме (светлая/тёмная).
            // Светлый набор теперь полноценный (theme.toml `[light]`) — никаких
            // перекрытий палитрой на лету, цвета редактируются как у тёмного.
            let orders = eff.orders.get(palette.is_light()).clone();
            let theme = eff.theme.get(palette.is_light()).clone();
            // Свечи: per-вкладочный override панели, иначе глобальный дефолт из layout.
            let candle_view = self.candle_view.unwrap_or(b.layout.candle_view);
            (theme, orders, b.follow, prospective, candle_view)
        };
        // Масштаб — ПО-ВКЛАДОЧНЫЙ: берём self.scale (его правят set_scale из тулбара активной
        // вкладки / шапки выносного окна), а не глобальный backend.price_scale.
        let mut settings_changed = self.chart.set_theme(theme)
            | self.chart.set_orders(orders_style)
            | self.chart.set_scale(self.scale)
            | self.chart.set_orderbook_enabled(self.orderbook_enabled)
            | self
                .chart
                .set_liquidations_enabled(self.liquidations_enabled)
            | self.chart.set_orderbook_only(self.orderbook_only)
            | self.chart.set_candle_view(candle_view)
            | self.chart.set_price_axis_pos(self.price_axis_pos)
            | self.chart.set_time_axis_visible(self.time_axis_visible)
            | self.chart.set_line_labels(self.line_labels)
            | self.chart.set_cursor_labels(self.cursor_labels)
            | self.chart.set_prospective_usd(prospective_usd)
            | self.chart.set_follow(follow, now_unix_ms());
        // Режим сравнения: пока активен lock, держим Y-окно якоря (перебивает scale каждый кадр —
        // set_locked_y идемпотентен, без изменений вернёт false). Снятие lock — в set_locked_y.
        if let Some((center, range)) = self.locked_y {
            settings_changed |= self.chart.set_locked_y(center, range);
        }
        if settings_changed {
            self.view_dirty = true;
        }

        // Render path only publishes layout/settings dirtiness. Market data is pulled
        // by gpu_canvas.frame(); account/order overlays have their own narrow sync.
        let view_changed = self.view_dirty;
        if became_visible || view_changed {
            self.view_dirty = false;
            self.sync_orders_if_visible(cx, true);
        }

        // axis_panes (раскладка панелей + снимок) считаем ОДИН раз за кадр и переиспользуем
        // и для hit-теста ввода (pane_rects), и для отрисовки осей — раньше layout панелей
        // гонялся дважды (внутри гейта prepare ради pane_rects + здесь ради отрисовки).
        let axis_panes = self.chart.axis_panes(axes::local_offset_sec());
        self.input.pane_rects = self.chart.pane_rects();
        // Хит-тест ввода должен знать сторону оси (отступ/ширина плота). Метла прячет ось.
        self.input.price_axis_pos = if self.orderbook_only {
            crate::chart_persist::PriceAxisPos::Hide
        } else {
            self.price_axis_pos
        };
        // Угловой ✕ закрытия монеты — на панели графика (и Main, и AddToChart):
        // закрыл монету на Main → вернулись к лого. Позиция из раскладки панелей (девайс-px →
        // лог.px слота); собираем ДО canvas, который забирает axis_panes по move.
        let close_btns: Vec<(usize, f32, f32)> = axis_panes
            .iter()
            .map(|(idx, rect, _)| (*idx, (rect.x + rect.w) / ppp, rect.y / ppp))
            .collect();
        // Cursor-only motion is handled by the chart-slot hitbox below. It updates retained
        // gpu_canvas cursor/readout directly and does not notify the GPUI tree.
        // П.2: кнопка «пин» в левом верхнем углу ВНУТРИ области графика (правее ценовой оси,
        // не на самой оси) — ТОЛЬКО на AddToChart-панелях (с TTL). Пин отменяет авто-закрытие.
        // (idx, pinned, left_px, top_px). PRICE_AXIS_W — логическая ширина оси (rect в девайс-px).
        // Кнопки (пин/замок/метла) у ЛЕВОГО края плота → сдвиг на ось нужен ТОЛЬКО когда ось слева.
        // При оси справа/скрытой (и в режиме метлы) плот начинается у края слота → сдвига нет.
        let axis_off = if matches!(
            self.price_axis_pos,
            crate::chart_persist::PriceAxisPos::Left
        ) && !self.orderbook_only
        {
            moon_chart::PRICE_AXIS_W
        } else {
            0.0
        };
        let pin_btns: Vec<(usize, bool, f32, f32)> = axis_panes
            .iter()
            .filter(|(idx, _, _)| self.chart.pane_is_pinnable(*idx))
            .map(|(idx, rect, _)| {
                (
                    *idx,
                    self.chart.pane_pinned(*idx),
                    rect.x / ppp + axis_off,
                    rect.y / ppp,
                )
            })
            .collect();
        // Кнопка-замок режима сравнения — ТОЛЬКО когда вкладка горизонтальная (`compare_eligible`),
        // рядом с пином. Горит на якоре (`is_compare_anchor`). Клик переносит чарт влево и делает
        // его ведущим по цене (обрабатывает стек по `take_compare_lock_request`).
        let compare_anchor = self.is_compare_anchor;
        let compare_broom_on = self.compare_broom_on;
        let lock_btns: Vec<(usize, f32, f32)> = if self.compare_eligible {
            axis_panes
                .iter()
                .map(|(idx, rect, _)| (*idx, rect.x / ppp + axis_off, rect.y / ppp))
                .collect()
        } else {
            Vec::new()
        };
        // Кнопка-метла — ТОЛЬКО на якоре (рядом с горящим замком). Переключает «только стакан»
        // у соседей якоря.
        let broom_btns: Vec<(usize, f32, f32)> = if self.compare_eligible && compare_anchor {
            axis_panes
                .iter()
                .map(|(idx, rect, _)| (*idx, rect.x / ppp + axis_off, rect.y / ppp))
                .collect()
        } else {
            Vec::new()
        };
        // Риска зоны управления: при раздельных зонах И СКРЫТОМ стакане рисуем границу зоны
        // ордеров (справа поверх чарта), чтобы было видно, где клики ставят ордера, а где
        // дабл-клик уходит на Main. Стакан виден → его видно и так, риску не дублируем.
        // (idx, left_лог, top_лог, w_лог, h_лог) — device-px из axis_panes делим на ppp, как ✕.
        let show_zone_marker = self.show_zone && self.separate_zones(cx) && !self.orderbook_enabled;
        let zone_markers: Vec<(usize, f32, f32, f32, f32)> = if show_zone_marker {
            axis_panes
                .iter()
                .map(|(idx, rect, _)| {
                    let zone_w = moon_chart::GLASS_ZONE_PX.min(rect.w * 0.5);
                    // Ось времени скрыта → жёлоб под подписи не резервируем, зона до низа слота.
                    let time_axis_h = if self.time_axis_visible {
                        moon_chart::TIME_AXIS_H * ppp
                    } else {
                        0.0
                    };
                    let plot_h = (rect.h - time_axis_h).max(1.0);
                    (
                        *idx,
                        (rect.x + rect.w - zone_w) / ppp,
                        rect.y / ppp,
                        zone_w / ppp,
                        plot_h / ppp,
                    )
                })
                .collect()
        } else {
            Vec::new()
        };
        // Кнопки рыночных действий (Cancel Buy / Panic Sell) — GPUI-оверлей внизу тела графика,
        // НАД строкой оси времени (OverScene-текст осей рисуется поверх GPUI, в его зону не лезем —
        // см. docs-internal/POPUP_LAYOUT_TZ.md). Позиция каждой кнопки (Hide/Left/Center/Right) —
        // из настроек вкладки. Кладём СТРОГО в зону чарта: слева режем ось цены, справа — зону
        // стакана/управления (даже когда стакан выключен). Не влезает — ужимаем кнопки (текст
        // обрежется справа overflow_hidden). Список: (kind, x, top, w, h, core, market, armed).
        const ACT_BTN_W: f32 = 92.0;
        const ACT_GAP: f32 = 8.0;
        const ACT_MIN_W: f32 = 30.0;
        // Кнопки — MoonButton размера Micro (маленькие, как close/pin/lock-оверлей чарта). Их
        // высоту для раскладки берём ИЗ ТЕМЫ (масштабируется ползунком шрифта, как сама кнопка),
        // а не хардкод-числом: базовые метрики Micro (h18/line12/pad3) — те же, что moonui считает
        // в height_for_size → MoonTheme::fit_height. Ширину кнопок задаём мы (зона чарта).
        let act_btn_h = crate::design::fit_h_value(cx, 18.0, 12.0, 3.0);
        let cancel_pos = self.cancel_buy_pos;
        let panic_pos = self.panic_sell_pos;
        // Одиночный пейн (фулскрин Main) → кнопки кладём GPUI-раскладкой ниже (`action_overlay`):
        // GPUI ресайзит их синхронно со слотом. Per-pane позиции из `axis_panes` берут own-pass
        // геометрию (`data.w/h`), которая обновляется на present-тике и при фулскрин-тогле отстаёт
        // на кадры — отсюда «прыжок» кнопок. Несколько пейнов (стек/сравнение) → per-pane.
        let single_pane =
            !self.orderbook_only && axis_panes.len() == 1 && self.chart.pane_target(0).is_some();
        let mut action_btns: Vec<(
            ActKind,
            f32,
            f32,
            f32,
            f32,
            moon_core::session::CoreId,
            String,
            bool,
        )> = Vec::new();
        if !single_pane && !self.orderbook_only {
            for (idx, rect, _) in axis_panes.iter() {
                let Some((core, market)) = self.chart.pane_target(*idx) else {
                    continue;
                };
                let pane_left = rect.x / ppp;
                let pane_w = rect.w / ppp;
                // Зона чарта: [ось цены .. начало зоны стакана/управления]. Стакан-зону резервируем
                // всегда (и при выключенном стакане), как просит ТЗ. Жёлоб оси режем с той стороны,
                // где она стоит: слева (axis_off) или справа за стаканом (доп. резерв справа).
                let glass_reserve = moon_chart::GLASS_ZONE_PX.min(pane_w * 0.5);
                let right_axis_reserve = if matches!(
                    self.price_axis_pos,
                    crate::chart_persist::PriceAxisPos::Right
                ) {
                    moon_chart::PRICE_AXIS_W
                } else {
                    0.0
                };
                let zone_left = pane_left + axis_off;
                let zone_right = pane_left + pane_w - glass_reserve - right_axis_reserve;
                let zone_w = zone_right - zone_left;
                if zone_w < ACT_MIN_W {
                    continue;
                }
                let time_axis_reserve = if self.time_axis_visible {
                    moon_chart::TIME_AXIS_H
                } else {
                    0.0
                };
                let top = (rect.y + rect.h) / ppp - time_axis_reserve - act_btn_h - 10.0;
                let armed = self.backend.read(cx).is_panic_armed(core, &market);
                // Видимые кнопки (kind, anchor) в стабильном порядке.
                let mut vis: Vec<(ActKind, ChartBtnPos)> = Vec::new();
                if cancel_pos != ChartBtnPos::Hide {
                    vis.push((ActKind::CancelBuy, cancel_pos));
                }
                if panic_pos != ChartBtnPos::Hide {
                    vis.push((ActKind::PanicSell, panic_pos));
                }
                if vis.is_empty() {
                    continue;
                }
                // Глобальный шринк: ВСЕ кнопки должны помещаться в зону одним рядом — иначе при
                // разных якорях (лево+право) на узком чарте они бы наложились.
                let n = vis.len() as f32;
                let bw = if n * ACT_BTN_W + (n - 1.0) * ACT_GAP > zone_w {
                    ((zone_w - (n - 1.0) * ACT_GAP) / n).max(ACT_MIN_W)
                } else {
                    ACT_BTN_W
                };
                let hi = (zone_right - bw).max(zone_left);
                let anchor_x = |a: ChartBtnPos| -> f32 {
                    let x = match a {
                        ChartBtnPos::Left => zone_left,
                        ChartBtnPos::Center => zone_left + (zone_w - bw) * 0.5,
                        ChartBtnPos::Right => zone_right - bw,
                        ChartBtnPos::Hide => zone_left,
                    };
                    x.clamp(zone_left, hi)
                };
                let order = |a: ChartBtnPos| -> u8 {
                    match a {
                        ChartBtnPos::Left => 0,
                        ChartBtnPos::Center => 1,
                        ChartBtnPos::Right => 2,
                        ChartBtnPos::Hide => 0,
                    }
                };
                let mut placed: Vec<(ActKind, f32)> = Vec::new();
                if vis.len() == 1 {
                    placed.push((vis[0].0, anchor_x(vis[0].1)));
                } else if vis[0].1 == vis[1].1 {
                    // Одинаковый якорь — ряд из двух кнопок у этого якоря.
                    let total = 2.0 * bw + ACT_GAP;
                    let start = match vis[0].1 {
                        ChartBtnPos::Center => zone_left + (zone_w - total) * 0.5,
                        ChartBtnPos::Right => zone_right - total,
                        _ => zone_left,
                    }
                    .clamp(zone_left, (zone_right - total).max(zone_left));
                    placed.push((vis[0].0, start));
                    placed.push((vis[1].0, start + bw + ACT_GAP));
                } else {
                    // Разные якоря — слева-направо по порядку якоря, без наложения.
                    let (mut a, mut b) = (vis[0], vis[1]);
                    if order(a.1) > order(b.1) {
                        std::mem::swap(&mut a, &mut b);
                    }
                    let xa = anchor_x(a.1);
                    let xb = anchor_x(b.1).max(xa + bw + ACT_GAP).clamp(zone_left, hi);
                    placed.push((a.0, xa));
                    placed.push((b.0, xb));
                }
                for (kind, x) in placed {
                    action_btns.push((kind, x, top, bw, act_btn_h, core, market.clone(), armed));
                }
            }
        }
        // Оверлей кнопок для ОДИНОЧНОГО пейна — чистая GPUI-раскладка (insets + flex), без
        // own-pass геометрии, поэтому позиция синхронна со слотом (нет «прыжка» при фулскрине).
        // Зона чарта = слот минус ось цены (слева), зона стакана/управления (справа), ось времени
        // (снизу). Три региона = якоря Left/Center/Right.
        let action_overlay = if single_pane {
            self.chart.pane_target(0).and_then(|(core, market)| {
                let armed = self.backend.read(cx).is_panic_armed(core, &market);
                let backend0 = self.backend.clone();
                let mk = |kind: ActKind| -> AnyElement {
                    let id = match kind {
                        ActKind::CancelBuy => "chart-cancelbuy-fs",
                        ActKind::PanicSell => "chart-panic-fs",
                    };
                    action_button(
                        kind,
                        SharedString::from(id),
                        armed,
                        backend0.clone(),
                        core,
                        market.clone(),
                    )
                    .render()
                    .into_any_element()
                };
                let mut left: Vec<AnyElement> = Vec::new();
                let mut center: Vec<AnyElement> = Vec::new();
                let mut right: Vec<AnyElement> = Vec::new();
                for (kind, pos) in [
                    (ActKind::CancelBuy, cancel_pos),
                    (ActKind::PanicSell, panic_pos),
                ] {
                    match pos {
                        ChartBtnPos::Left => left.push(mk(kind)),
                        ChartBtnPos::Center => center.push(mk(kind)),
                        ChartBtnPos::Right => right.push(mk(kind)),
                        ChartBtnPos::Hide => {}
                    }
                }
                if left.is_empty() && center.is_empty() && right.is_empty() {
                    return None;
                }
                // Регионы по СОДЕРЖИМОМУ (не flex_1): кнопки держат свою ширину и не режутся, когда
                // на одном якоре их две (дефолт — обе справа). Якорение L/C/R даёт пара flex-спейсеров
                // между регионами. Левый отступ = ось цены ТОЛЬКО когда она слева; правый = зона
                // стакана + жёлоб оси, если она справа (за стаканом).
                let region = |btns: Vec<AnyElement>| {
                    div().flex().items_center().gap(px(ACT_GAP)).children(btns)
                };
                let left_pad = if matches!(
                    self.price_axis_pos,
                    crate::chart_persist::PriceAxisPos::Left
                ) {
                    moon_chart::PRICE_AXIS_W
                } else {
                    0.0
                };
                let right_pad = moon_chart::GLASS_ZONE_PX
                    + if matches!(
                        self.price_axis_pos,
                        crate::chart_persist::PriceAxisPos::Right
                    ) {
                        moon_chart::PRICE_AXIS_W
                    } else {
                        0.0
                    };
                Some(
                    div()
                        .absolute()
                        .inset_0()
                        .pl(px(left_pad))
                        .pr(px(right_pad))
                        .pb(px(if self.time_axis_visible {
                            moon_chart::TIME_AXIS_H + 10.0
                        } else {
                            10.0
                        }))
                        .flex()
                        .flex_col()
                        .justify_end()
                        .child(
                            div()
                                .w_full()
                                .h(px(act_btn_h))
                                .flex()
                                .items_center()
                                .child(region(left))
                                .child(div().flex_1())
                                .child(region(center))
                                .child(div().flex_1())
                                .child(region(right)),
                        )
                        .into_any_element(),
                )
            })
        } else {
            None
        };
        let show_empty_logo = axis_panes.is_empty();
        let (slot_w, _) = self.chart.slot_dev_size();
        let logo_w = ((slot_w as f32 / ppp) * 0.28).clamp(180.0, 280.0);
        div()
            .id("chart-slot")
            .size_full()
            .min_w_0()
            .overflow_hidden()
            .relative()
            .track_focus(&self.focus)
            // Над перетаскиваемой линией ордера (ховер ИЛИ активный drag) — вертикальная
            // стрелка «вверх-вниз»: линию двигают только по цене (Y), это сразу читается как
            // «можно тянуть». Отдельный grab/grabbing не используем — ns-resize точнее
            // отражает одномерную (вертикальную) природу перетаскивания.
            .when(
                self.order_drag.is_some() || self.order_hover.is_some(),
                |this| this.cursor_ns_resize(),
            )
            .on_scroll_wheel(cx.listener(render_input::scroll_wheel))
            .on_mouse_down(MouseButton::Left, cx.listener(render_input::mouse_down_left))
            .on_mouse_up(MouseButton::Left, cx.listener(render_input::mouse_up_left))
            .on_mouse_down(MouseButton::Right, cx.listener(render_input::mouse_down_right))
            .on_mouse_up(MouseButton::Right, cx.listener(render_input::mouse_up_right))
            .on_mouse_down(MouseButton::Middle, cx.listener(render_input::mouse_down_middle))
            .on_mouse_move(cx.listener(render_input::mouse_move))
            .on_hover(cx.listener(render_input::hover))
            // own-pass: геометрию слота движок берёт синхронно из `GpuFrameInfo.bounds` в
            // `frame()` (см. data_state::apply_slot_geometry) — поэтому уже первый present рисует
            // в реальном слоте, без «распахивания» дефолтного размера и без лага при рефлоу.
            .child(self.chart.canvas().text_under().absolute().size_full())
            .when(show_empty_logo, |this| {
                // Непрозрачный фон поверх own-pass: пустой слот = логотип на фоне чарта, без
                // просвечивания старого графика (own-pass рисуется ПОД сценой GPUI).
                this.child(
                    div()
                        .absolute()
                        .size_full()
                        .bg(rgb(palette.chart_bg))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(crate::design::logo_glow_sized(cx, logo_w)),
                )
            })
            // FireTest probe only. Геометрию самого чарта не берём из GPUI-probe: единственный
            // source of truth для input/own-pass — `GpuFrameInfo.bounds`.
            .child({
                let is_main = self.num.is_none();
                let backend = self.backend.clone();
                canvas(
                    move |bounds, _, _| bounds,
                    move |bounds, _, window, cx| {
                        let sf = window.scale_factor();
                        let firetest_probe = crate::firetest::ChartProbe::new(
                            crate::windowing::window_hwnd(window),
                            f32::from(window.window_bounds().get_bounds().origin.x),
                            f32::from(window.window_bounds().get_bounds().origin.y),
                            f32::from(bounds.origin.x),
                            f32::from(bounds.origin.y),
                            f32::from(bounds.size.width),
                            f32::from(bounds.size.height),
                            sf,
                        );
                        if is_main {
                            if let Some(probe) = firetest_probe {
                                backend.update(cx, |b, _| {
                                    crate::firetest::observe_chart_probe(b, probe);
                                });
                            }
                        }
                    },
                )
                .absolute()
                .size_full()
            })
            .children(zone_markers.into_iter().map(|(_idx, left, top, w, h)| {
                // Тусклая заливка зоны управления (стакан скрыт) — без линии-границы.
                div()
                    .absolute()
                    .left(px(left))
                    .top(px(top))
                    .w(px(w))
                    .h(px(h))
                    .bg(rgba_from(palette.blue, 0.03))
            }))
            .children(close_btns.into_iter().map(|(idx, right, top)| {
                let entity = cx.entity();
                MoonButton::new(SharedString::from(format!("chart-close-{idx}")))
                    // Крестик ярче и жирнее (было приглушённый ghost-fg text_muted@0.78):
                    // `text_segment` задаёт цвет (полный `text`) и вес (700). Подложка/ховер — как
                    // у Ghost (прозрачно по умолчанию, лёгкий фон на наведении).
                    .text_segment("×", palette.text, 700.0)
                    .size(MoonButtonSize::Micro)
                    .variant(MoonButtonVariant::Ghost)
                    // 22×22 — чтобы не мискликнуть мимо на стакан при быстром закрытии нескольких
                    // графиков подряд.
                    .bounds(MoonRect::new(right - 26.0, top + 3.0, 22.0, 22.0))
                    .on_click(move |_, _w, app| {
                        entity.update(app, |this, cx| this.remove_pane(idx, cx));
                    })
                    .render()
            }))
            .children(pin_btns.into_iter().map(|(idx, pinned, left, top)| {
                // Пин-кнопка в левом верхнем углу: заполненный кружок = приколото, контур = нет (П.2).
                let entity = cx.entity();
                MoonButton::new(SharedString::from(format!("chart-pin-{idx}")))
                    .label(if pinned { "●" } else { "○" })
                    .size(MoonButtonSize::Micro)
                    .variant(if pinned {
                        MoonButtonVariant::Blue
                    } else {
                        MoonButtonVariant::Ghost
                    })
                    .selected(pinned)
                    .bounds(MoonRect::new(left + 3.0, top + 3.0, 15.0, 15.0))
                    .on_click(move |_, _w, app| {
                        entity.update(app, |this, cx| this.toggle_pin(idx, cx));
                    })
                    .render()
            }))
            .children(lock_btns.into_iter().map(|(idx, left, top)| {
                // Замок справа от пина: клик → этот чарт в начало ряда + ведущий по цене.
                let entity = cx.entity();
                MoonButton::new(SharedString::from(format!("chart-lock-{idx}")))
                    .label("🔒")
                    .size(MoonButtonSize::Micro)
                    .variant(if compare_anchor {
                        MoonButtonVariant::Blue
                    } else {
                        MoonButtonVariant::Ghost
                    })
                    .selected(compare_anchor)
                    .bounds(MoonRect::new(left + 21.0, top + 3.0, 15.0, 15.0))
                    .on_click(move |_, _w, app| {
                        entity.update(app, |this, cx| this.request_compare_lock(cx));
                    })
                    .render()
            }))
            .children(broom_btns.into_iter().map(|(idx, left, top)| {
                // Метла справа от замка (на якоре): «только стакан» у соседей.
                let entity = cx.entity();
                MoonButton::new(SharedString::from(format!("chart-broom-{idx}")))
                    .label("🧹")
                    .size(MoonButtonSize::Micro)
                    .variant(if compare_broom_on {
                        MoonButtonVariant::Blue
                    } else {
                        MoonButtonVariant::Ghost
                    })
                    .selected(compare_broom_on)
                    .bounds(MoonRect::new(left + 39.0, top + 3.0, 15.0, 15.0))
                    .on_click(move |_, _w, app| {
                        entity.update(app, |this, cx| this.request_compare_broom(cx));
                    })
                    .render()
            }))
            .children(action_btns.into_iter().enumerate().map(
                |(i, (kind, x, y, w, h, core, market, armed))| {
                    let id = match kind {
                        ActKind::CancelBuy => format!("chart-cancelbuy-{i}"),
                        ActKind::PanicSell => format!("chart-panic-{i}"),
                    };
                    let btn = action_button(
                        kind,
                        SharedString::from(id),
                        armed,
                        self.backend.clone(),
                        core,
                        market,
                    )
                    .full_width()
                    .render();
                    // Контейнер задаёт ширину для обрезки текста (overflow клипует к bounds по
                    // ОБЕИМ осям). Высоту берём с запасом (h+4), чтобы низ кнопки не срезался —
                    // кнопка прижата к верху и целиком внутри.
                    div()
                        .absolute()
                        .left(px(x))
                        .top(px(y))
                        .w(px(w))
                        .h(px(h + 4.0))
                        .overflow_x_hidden()
                        .child(btn)
                },
            ))
            .children(action_overlay)
    }
}
