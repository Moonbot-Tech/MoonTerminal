//! Синк ордеров из сессии + подписи ордер-линий (вынос из data_state.rs, verbatim).

use super::*;

use crate::chartdx::text::fmt_pct;

fn hash_order_zones(zones: &[moon_chart::layers::ZoneInstance]) -> u64 {
    let mut h = 0x9E37_79B9_7F4A_7C15u64 ^ zones.len() as u64;
    for z in zones {
        h = h.rotate_left(5) ^ z.price0.to_bits() as u64;
        h = h.rotate_left(7) ^ z.price1.to_bits() as u64;
        for c in z.color {
            h = h.rotate_left(11) ^ c.to_bits() as u64;
        }
    }
    h
}

impl ChartDataState {
    pub(crate) fn sync_orders_from_session(
        &mut self,
        session: &SessionManager,
        force: bool,
    ) -> bool {
        let area = Rect {
            x: 0.0,
            y: 0.0,
            w: self.w as f32,
            h: self.h as f32,
        };
        let layout = self.container.borrow().layout(area);
        let now = now_unix_ms();
        let mut st = self.render.borrow_mut();
        let mut container = self.container.borrow_mut();
        let mut pixels_changed = false;
        let mut base_changed = false;
        // Смена числа панелей (в т.ч. удаление последней монеты → пусто) обязана пометить
        // base_dirty: иначе base-кэш продолжит блитить СТАРЫЙ чарт сквозь пустой слот (логотип
        // прозрачный). Зеркалит проверку в sync_from_market_source.
        if st.panes.len() != container.pane_count() {
            pixels_changed = true;
            base_changed = true;
        }
        st.panes
            .resize_with(container.pane_count(), PaneRender::new);

        for (idx, _) in &layout {
            let Some(pane) = container.pane_mut(*idx) else {
                continue;
            };
            let pr = &mut st.panes[*idx];
            if pr.core != Some(pane.core) || pr.market != pane.market {
                *pr = PaneRender::new();
                pr.core = Some(pane.core);
                pr.market = pane.market.clone();
                pixels_changed = true;
                base_changed = true;
            }
            // Имя ядра для угловой подписи: резолвим тут — только здесь под рукой `session`.
            // Меняется редко (смена ядра панели), поэтому флагаем present лишь при изменении.
            let core_name = session
                .sessions()
                .iter()
                .find(|s| s.id == pane.core)
                .map(|s| s.name.clone())
                .unwrap_or_default();
            if pr.core_name != core_name {
                pr.core_name = core_name;
                pixels_changed = true;
            }
            let device_gen = pr.layers.device_gen();
            let device_lost = pr.last_device_gen != device_gen;
            if device_lost {
                pr.last_order_lines_rev = u64::MAX;
                pr.gpu_prepare_dirty = true;
                pixels_changed = true;
            }

            let order_price = session
                .store()
                .core(pane.core)
                .and_then(|core_st| core_st.order_lines.buy_sell_range(&pane.market));
            if pr.cached_order_price != order_price {
                pr.cached_order_price = order_price;
                self.view_dirty = true;
                pixels_changed = true;
            }

            if let Some(core_st) = session.store().core(pane.core) {
                let highlight_uid = self
                    .order_highlight
                    .and_then(|(core, uid)| (core == pane.core).then_some(uid));
                let drag_preview = self
                    .order_drag_preview
                    .and_then(|(core, uid, kind, price)| {
                        (core == pane.core).then_some((uid, kind, price))
                    });
                let drag_preview_sig =
                    drag_preview.map(|(uid, kind, price)| (uid, kind, price.to_bits()));
                let figures_sig = self.figures_sig();
                if force
                    || pr.last_order_lines_rev != core_st.order_lines_rev
                    || pr.last_order_highlight_uid != highlight_uid
                    || pr.last_order_drag_preview != drag_preview_sig
                    || pr.last_figures_sig != figures_sig
                {
                    let mut hlines = Vec::new();
                    let mut segs = Vec::new();
                    let mut markers = Vec::new();
                    let mut zones = Vec::new();
                    moon_chart::build_order_geometry(
                        &core_st.order_lines,
                        &pane.market,
                        &self.orders,
                        highlight_uid,
                        drag_preview,
                        pane.view.epoch_ms,
                        now,
                        f32::NEG_INFINITY,
                        f32::INFINITY,
                        0.0,
                        &mut zones,
                        &mut hlines,
                        &mut segs,
                        &mut markers,
                    );
                    // Пользовательские фигуры (слой рисования) — теми же userdata-слоями,
                    // ПОСЛЕ ордеров (рисуются поверх их зон, под маркерами курсора).
                    self.append_figure_geometry(
                        pane.core,
                        &pane.market,
                        pane.view.epoch_ms,
                        &mut hlines,
                        &mut segs,
                        &mut markers,
                    );
                    let zone_sig = hash_order_zones(&zones);
                    if pr.last_order_zone_sig != zone_sig {
                        pr.last_order_zone_sig = zone_sig;
                        base_changed = true;
                    }
                    pr.layers.set_userdata(&zones, &hlines, &segs, &markers);
                    let quote_usd = self
                        .market_source
                        .as_ref()
                        .and_then(|s| s.quote_usd_rate(pane.core, &pane.market));
                    build_order_labels(
                        &mut pr.order_labels,
                        &mut pr.orderbook_labels,
                        &core_st.order_lines,
                        &pane.market,
                        &self.theme,
                        quote_usd,
                        drag_preview,
                    );
                    rebuild_order_label_order(&mut pr.order_label_order, &pr.order_labels);
                    refresh_orderbook_label_notionals(
                        &mut pr.orderbook_labels,
                        &pr.orderbook_levels,
                    );
                    pr.last_order_lines_rev = core_st.order_lines_rev;
                    pr.last_order_lines_sync_ms = now;
                    pr.pending_order_gpu_rev = Some(core_st.order_lines_rev);
                    pr.last_order_highlight_uid = highlight_uid;
                    pr.last_order_drag_preview = drag_preview_sig;
                    pr.last_figures_sig = figures_sig;
                    pr.gpu_prepare_dirty = true;
                    pixels_changed = true;
                }
            } else if force || pr.last_order_lines_rev != u64::MAX {
                if pr.last_order_zone_sig != 0 {
                    pr.last_order_zone_sig = 0;
                    base_changed = true;
                }
                pr.layers.set_userdata(&[], &[], &[], &[]);
                pr.order_labels.clear();
                pr.order_label_order.clear();
                pr.orderbook_labels.clear();
                pr.last_order_lines_rev = u64::MAX;
                pr.last_order_lines_sync_ms = now;
                pr.pending_order_gpu_rev = Some(u64::MAX);
                pr.last_order_highlight_uid = None;
                pr.last_order_drag_preview = None;
                pr.gpu_prepare_dirty = true;
                pixels_changed = true;
            }
            pr.last_device_gen = device_gen;
        }

        if base_changed {
            st.base_dirty = true;
        }
        if pixels_changed {
            st.needs_present = true;
        }
        pixels_changed
    }
}

/// Подписи ордерных линий рынка для слоя текста: размер у buy-линии, % от входа +
/// количество купленного у sell-линии, % стопа у stop-линии. Сторона размещения
/// (над/под линией) зависит от long/short — как в эталоне MoonBot (категория E).
/// Только открытые ордера (закрытые/исполненные не подписываем).
fn build_order_labels(
    out: &mut Vec<OrderLabel>,
    book_out: &mut Vec<OrderBookLabel>,
    store: &moon_core::session::order_lines::OrderLineStore,
    market: &str,
    theme: &ChartTheme,
    quote_usd: Option<f64>,
    drag_preview: Option<(u64, LineKind, f32)>,
) {
    out.clear();
    book_out.clear();
    let mut orders: Vec<_> = store
        .iter_market(market)
        .filter(|o| o.closed_ms.is_none())
        .collect();
    orders.sort_by_key(|o| o.seq);
    for o in orders {
        let preview = drag_preview
            .filter(|(uid, _, price)| *uid == o.uid && price.is_finite() && *price > 0.0);
        let line_price = |kind: LineKind| {
            preview
                .filter(|(_, preview_kind, _)| *preview_kind == kind)
                .map(|(_, _, price)| price)
                .or_else(|| o.lines[kind as usize].current_price())
        };
        let line_forced =
            |kind: LineKind| preview.is_some_and(|(_, preview_kind, _)| preview_kind == kind);
        let mut push =
            |price: f32, text: String, above: bool, color: u32, priority: u8, force: bool| {
                if price.is_finite() && price > 0.0 && !text.is_empty() {
                    out.push(OrderLabel {
                        price,
                        text,
                        above,
                        color,
                        priority,
                        force,
                    });
                }
            };
        let buy = line_price(LineKind::Buy);
        let sell = line_price(LineKind::Sell);
        let stop = line_price(LineKind::Stop);
        let short = o.is_short;
        // Порядковый номер ордера на чарте — на основной подписи каждой линии (buy/sell/stop),
        // чтобы связать линии одного ордера: «$X [10]», «-5% [10]», стоп «-3% [10]».
        let tag = if o.chart_num > 0 {
            format!("[{}]", o.chart_num)
        } else {
            String::new()
        };
        let with_tag = |text: String| {
            if tag.is_empty() {
                text
            } else {
                format!("{text} {tag}")
            }
        };
        // BUY (линия входа): ожидающий ордер (fill=0) → «размер [N]» ОДНОЙ строкой, чтобы номер
        // и размер не наложились друг на друга на одной стороне линии; исполненный → только [N].
        // Размер входа — ВСЕГДА белый (не цвет линии, не по стороне).
        if let Some(bp) = buy {
            let forced = line_forced(LineKind::Buy);
            let text = if o.fill_pct <= 0.0 && o.size > 0.0 {
                let amount = match quote_usd {
                    Some(rate) if rate > 0.0 => fmt_usd(o.size as f64 * bp as f64 * rate),
                    _ => fmt_amount(o.size),
                };
                with_tag(amount)
            } else {
                tag.clone()
            };
            push(bp, text, !short, ORDER_LABEL_NEUTRAL, PRIO_BUY, forced);
        }
        // SELL: профит-% от цены входа (знаковый цвет) + РАЗМЕР на продажу в $-ноционале
        // (remaining·цена_продажи·курс) на противоположной стороне линии — как в MoonBot:
        // процент primary рисуется всегда, остаток caption проходит через YTextFill.
        if let Some(sp) = sell {
            let forced = line_forced(LineKind::Sell);
            if sp.is_finite() && sp > 0.0 {
                book_out.push(OrderBookLabel {
                    price: sp,
                    short,
                    notional: 0.0,
                });
            }
            if let Some(bp) = buy {
                if bp > 0.0 {
                    let pct = signed_pct(sp, bp, short);
                    push(
                        sp,
                        with_tag(fmt_pct(pct)),
                        short,
                        pct_color(theme, pct),
                        PRIO_SELL_PCT,
                        forced,
                    );
                }
            }
            let remaining = if o.remaining_size > 0.0 {
                o.remaining_size
            } else {
                o.size
            };
            if remaining > 0.0 && sp > 0.0 {
                let amount = match quote_usd {
                    Some(rate) if rate > 0.0 => fmt_usd(remaining as f64 * sp as f64 * rate),
                    _ => fmt_amount(remaining),
                };
                push(
                    sp,
                    amount,
                    !short,
                    side_color(theme, short),
                    PRIO_SELL_SIZE,
                    forced,
                );
            }
        }
        // STOP: % стопа от цены покупки. Шорт → сверху, лонг → снизу. Primary без YTextFill,
        // как Delphi-блок stop-loss label.
        if let (Some(stp), Some(bp)) = (stop, buy) {
            if bp > 0.0 {
                let forced = line_forced(LineKind::Stop);
                let pct = signed_pct(stp, bp, short);
                push(
                    stp,
                    with_tag(fmt_pct(pct)),
                    short,
                    pct_color(theme, pct),
                    PRIO_STOP_PCT,
                    forced,
                );
            }
        }
    }
}

fn rgb_u32(c: [u8; 3]) -> u32 {
    ((c[0] as u32) << 16) | ((c[1] as u32) << 8) | c[2] as u32
}

/// Знаковый процент уровня от цены входа С УЧЁТОМ СТОРОНЫ: лонг — как есть; шорт —
/// инвертирован (выше входа = минус). Так профит у обеих сторон зелёный, лосс красный (как MB):
/// для шорта sell ниже входа → «+», стоп выше входа → «−».
fn signed_pct(level: f32, entry: f32, short: bool) -> f32 {
    let raw = (level - entry) / entry * 100.0;
    if short { -raw } else { raw }
}

/// Цвет подписи размера по стороне ордера: лонг → positive, шорт → negative.
fn side_color(theme: &ChartTheme, short: bool) -> u32 {
    if short {
        rgb_u32(theme.label_negative)
    } else {
        rgb_u32(theme.label_positive)
    }
}

/// Цвет знакового процента: плюс → positive, минус → negative.
fn pct_color(theme: &ChartTheme, v: f32) -> u32 {
    if v >= 0.0 {
        rgb_u32(theme.label_positive)
    } else {
        rgb_u32(theme.label_negative)
    }
}

/// Компактное число (база/штуки) с SI-суффиксом K/M/B/T.
/// Размер ордера: SI-суффикс (K/M/B/T) + ДО 2 знаков дробной части (сотые, если есть),
/// без хвостовых нулей. Не использует общий `compact_si` (тот для десятков даёт до 3 знаков
/// — «49.744»). 50 → «50»; 49.744 → «49.74»; 1234 → «1.23K»; 49744 → «49.74K».
fn fmt_size_2dp(v: f64) -> String {
    let a = v.abs();
    let (n, suffix) = if a >= 1e12 {
        (v / 1e12, "T")
    } else if a >= 1e9 {
        (v / 1e9, "B")
    } else if a >= 1e6 {
        (v / 1e6, "M")
    } else if a >= 1e3 {
        (v / 1e3, "K")
    } else {
        (v, "")
    };
    let s = format!("{n:.2}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    format!("{s}{suffix}")
}

fn fmt_amount(v: f32) -> String {
    fmt_size_2dp(v as f64)
}

fn sell_book_notional(levels: &[moon_core::data::BookDepthPoint], price: f32, short: bool) -> f32 {
    let mut sum = 0.0_f32;
    for level in levels {
        if short {
            // MoonBot: short sell-line volume uses buy glass above sell price.
            if !level.is_ask && level.price > price {
                sum += level.notional;
            }
        } else {
            // MoonBot: long sell-line volume uses sell glass below sell price.
            if level.is_ask && level.price < price {
                sum += level.notional;
            }
        }
    }
    sum
}

fn rebuild_order_label_order(order: &mut Vec<usize>, labels: &[OrderLabel]) {
    order.clear();
    order.extend(0..labels.len());
    order.sort_by_key(|&ix| labels[ix].priority);
}

pub(super) fn refresh_orderbook_label_notionals(
    labels: &mut [OrderBookLabel],
    levels: &[moon_core::data::BookDepthPoint],
) {
    for label in labels {
        label.notional = sell_book_notional(levels, label.price, label.short);
    }
}

/// $-сумма с SI-суффиксом: 1234 → «$1.23K».
fn fmt_usd(v: f64) -> String {
    format!("${}", fmt_size_2dp(v))
}
