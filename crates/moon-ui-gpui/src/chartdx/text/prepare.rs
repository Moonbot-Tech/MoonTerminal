//! Главный `prepare_text`: оси, подписи ордер-линий и курсорный ридаут
//! (вынос из text.rs, verbatim).

use moon_chart::axes::price_decimals;

use super::*;

impl RenderState {
    pub(crate) fn prepare_text(
        &mut self,
        ctx: &mut GpuCanvasTextContext<'_>,
    ) -> anyhow::Result<()> {
        self.text_run_cursor = 0;
        let sf = ctx.scale_factor().max(0.1);
        let ink = color(self.axis_label);
        let readout = color(self.readout_label);
        let label_neutral = color(self.label_neutral);
        // Угловая подпись — отдельный цвет chart theme, без подложки.
        let caption_fg = color(self.caption_label);
        let tz_offset_sec = local_offset_sec();
        let mut firetest_text_drawn = false;
        let mut readout_metrics_changed = false;

        for idx in 0..self.panes.len() {
            let (
                active,
                pane_bounds,
                view,
                epoch_ms,
                core_name,
                market,
                orderbook_enabled,
                price_axis_pos,
                time_axis_visible,
            ) = {
                let pr = &self.panes[idx];
                (
                    pr.active,
                    pr.pane_bounds,
                    pr.view,
                    pr.epoch_ms,
                    pr.core_name.clone(),
                    pr.market.clone(),
                    pr.orderbook_enabled,
                    pr.price_axis_pos,
                    pr.time_axis_visible,
                )
            };
            if !active {
                continue;
            }
            let cached_last_price = self.panes[idx].cached_last_price;
            let prospective_usd = self.panes[idx].prospective_usd;
            // Раскладка подписей этого кадра (для плашек в sync_readout_params). Старую держим для
            // сравнения: при зуме меняется Y, и подложки должны переехать вместе с текстом.
            let previous_placed = std::mem::take(&mut self.panes[idx].label_placed);
            let mut placed: Vec<PlacedLabel> = Vec::new();
            let pane_left = pane_bounds[0] / sf;
            let pane_right = (pane_bounds[0] + pane_bounds[2]) / sf;
            let pane_bottom = (pane_bounds[1] + pane_bounds[3]) / sf;
            let plot_left = view.bounds[0] / sf;
            let plot_top = view.bounds[1] / sf;
            let plot_w = view.bounds[2] / sf;
            let plot_h = view.bounds[3] / sf;
            let plot_bottom = plot_top + plot_h;
            let plot_right = plot_left + plot_w;
            // Сторона оси цен: Left → подписи в жёлобе слева от плота; Right → справа у края панели
            // (жёлоб за стаканом); Hide → ось не рисуем вовсе. Правый якорь текста (align 1.0) общий.
            use crate::chart_persist::PriceAxisPos;
            let axis_hidden = matches!(price_axis_pos, PriceAxisPos::Hide);
            let axis_on_right = matches!(price_axis_pos, PriceAxisPos::Right);
            let axis_label_x = if axis_on_right {
                pane_right - 4.0
            } else {
                plot_left - 4.0
            };

            // Угловая подпись: имя ядра + тикер, светлый текст на прозрачной плашке (её строит
            // render_state по `caption_w`). Якорь правым краем: есть стакан → у края панели (над
            // стаканом), нет стакана → у края плота (в области графика). Тот же выбор повторён в
            // render_state для плашки — держать синхронно. Рисуем ДО гейта по `plot_w`, чтобы в
            // режиме «только стакан» (чарт схлопнут) подпись осталась над стаканом.
            {
                let right_edge = if orderbook_enabled {
                    pane_right
                } else {
                    plot_right
                };
                let cap_x = right_edge - CAPTION_PAD_X;
                let cap_y = plot_top + CAPTION_PAD_Y;
                let mut cap_w = 0.0_f32;
                let mut lines = 0u32;
                if !core_name.is_empty() {
                    cap_w = cap_w.max(self.measure_text(ctx, &core_name).width.as_f32());
                    self.draw_text(ctx, &core_name, cap_x, cap_y, 1.0, 0.0, caption_fg)?;
                    lines += 1;
                }
                let ticker = moon_core::symbol::display_pair(&market);
                if !ticker.is_empty() {
                    cap_w = cap_w.max(self.measure_text(ctx, &ticker).width.as_f32());
                    self.draw_text(ctx, &ticker, cap_x, cap_y + LINE_H, 1.0, 0.0, caption_fg)?;
                    lines += 1;
                }
                if (self.panes[idx].caption_w - cap_w).abs() > 0.25 {
                    self.panes[idx].caption_w = cap_w;
                    readout_metrics_changed = true;
                }
                // Метла: отличие last ЭТОЙ биржи от якоря (замочек) — крупно, под подписью,
                // знак и цвет ±. Данные обеих сторон живые: свой last — из пейна, last якоря
                // приносит стек (`set_compare_ref_price` в apply_compare) на каждом observe.
                let (delta_w, delta_h) = {
                    let (ob_only, own_last) = {
                        let pr = &self.panes[idx];
                        (pr.orderbook_only, pr.cached_last_price)
                    };
                    let pct = self
                        .compare_ref_price
                        .filter(|_| ob_only)
                        .zip(own_last)
                        .filter(|(r, l)| *r > 0.0 && *l > 0.0)
                        .map(|(r, l)| (l - r) / r * 100.0);
                    if let Some(pct) = pct {
                        let text = fmt_pct(pct);
                        let col = pct_hsla(pct, self.label_positive, self.label_negative);
                        let size = self.label_font_px() * 1.7;
                        let m = self.draw_sized_text(
                            ctx,
                            &text,
                            size,
                            cap_x,
                            cap_y + lines as f32 * LINE_H + 2.0,
                            1.0,
                            0.0,
                            col,
                        )?;
                        (m.width.as_f32(), m.line_height.as_f32() + 2.0)
                    } else {
                        (0.0, 0.0)
                    }
                };
                if (self.panes[idx].caption_delta_w - delta_w).abs() > 0.25
                    || (self.panes[idx].caption_delta_h - delta_h).abs() > 0.25
                {
                    self.panes[idx].caption_delta_w = delta_w;
                    self.panes[idx].caption_delta_h = delta_h;
                    readout_metrics_changed = true;
                }
                // Бейдж текущего Y-масштаба — ЛЕВЕЕ блока подписи, тем же крупным кеглем,
                // что и дельта метлы, цветом подписи (без ±). Целый процент («14%»).
                // Показ решает sync_from_market_source (Авто всегда / ручной при расхождении).
                let (scale_w, scale_h) = if let Some(pct) = self.panes[idx].scale_badge {
                    // Диапазон уже целого 1% (авто на спокойном рынке) → «<1%», не голый ноль.
                    let text = if pct == 0 {
                        "<1%".to_string()
                    } else {
                        format!("{pct}%")
                    };
                    // Чуть мельче дельты метлы (−2px), чтобы бейдж не спорил с ней за внимание.
                    let size = self.label_font_px() * 1.7 - 2.0;
                    let block_w = cap_w.max(delta_w);
                    let gap = if block_w > 0.0 {
                        CAPTION_SCALE_GAP
                    } else {
                        0.0
                    };
                    let m = self.draw_sized_text(
                        ctx,
                        &text,
                        size,
                        cap_x - block_w - gap,
                        cap_y,
                        1.0,
                        0.0,
                        caption_fg,
                    )?;
                    (m.width.as_f32() + gap, m.line_height.as_f32())
                } else {
                    (0.0, 0.0)
                };
                if (self.panes[idx].caption_scale_w - scale_w).abs() > 0.25
                    || (self.panes[idx].caption_scale_h - scale_h).abs() > 0.25
                {
                    self.panes[idx].caption_scale_w = scale_w;
                    self.panes[idx].caption_scale_h = scale_h;
                    readout_metrics_changed = true;
                }
            }

            // Дальше — оси/курсор/сетка, только для нормального (не схлопнутого) чарта.
            if plot_w < 60.0 || plot_h < 60.0 || view.price_to_px <= 0.0 {
                // Призрак compare-режима живёт и на схлопнутом чарте метлы (только стакан):
                // объём/% рисуем по виду стакана, линию даёт cursor.hlsl.
                self.draw_ghost_cursor_labels(ctx, idx, sf, &mut placed)?;
                if previous_placed != placed {
                    self.panes[idx].label_placed = placed;
                    readout_metrics_changed = true;
                }
                continue;
            }

            if !firetest_text_drawn {
                self.draw_firetest_text(ctx, plot_left, plot_top, plot_w, plot_h, ink)?;
                firetest_text_drawn = true;
            }

            let price_to_px = view.price_to_px / sf;
            let price_range = plot_h / price_to_px.max(1e-6);
            let y_min = view.view_price0;
            let line_y = |price: f32| -> f32 {
                ((plot_bottom * sf) - (price - y_min) * view.price_to_px).round() / sf
            };
            let dec = price_decimals(y_min + price_range * 0.5);
            let time_to_px = (view.time_to_px / sf).max(moon_chart::view::MIN_PX_PER_MS);
            let window_ms = plot_w as f64 / time_to_px as f64;
            let left_unix = epoch_ms + view.view_time0 as f64;

            // Левый край стакана / раздельной зоны (справа) — к нему прижаты (правым краем)
            // подписи ордерных линий и курсора. Стакан вкл → плот кончается у стакана →
            // его правый край = левый край стакана. Стакан выкл → левый край зоны управления.
            let zone_left = if orderbook_enabled {
                plot_right
            } else {
                let zone_w = moon_chart::GLASS_ZONE_PX.min((pane_right - pane_left) * 0.5);
                pane_right - zone_w
            };
            let label_x = zone_left - READOUT_PAD_X;

            // Подписи ордерных линий — отдельный столбик слева от разделителя, правым краем к нему.
            // Рисуем ВСЕ подписи (не прячем при наложении): порядок = приоритет ПО ВОЗРАСТАНИЮ,
            // поэтому старшая (SELL/STOP > BUY) рисуется ПОСЛЕДНЕЙ — её текст и полу-плотная плашка
            // ложатся ПОВЕРХ младшей, а младшая просвечивает сквозь плашку (~15%) → «заходит под»,
            // не исчезает. Подпись отстоит от линии на LABEL_LINE_GAP, чтобы плашка не накрыла саму
            // линию ордера. `force` (drag/hover) рисуем в самом конце — поверх всего.
            // Per-вкладка галка «подписи у линий» (попап ⚙). Выкл → столбец не строим.
            if self.line_labels {
                // Высота строки подписей зависит от их кегля (настраивается слайдером темы).
                let label_line_h = self.label_font_px() + 4.0;
                let mut force_items: Vec<(f32, f32, &OrderLabel)> = Vec::new();
                for &li in &self.panes[idx].order_label_order {
                    let order_labels = &self.panes[idx].order_labels;
                    if li >= order_labels.len() {
                        continue;
                    }
                    let label = &order_labels[li];
                    let y = line_y(label.price);
                    if y < plot_top - label_line_h || y > plot_bottom + label_line_h {
                        continue;
                    }
                    let (dy, ay) = if label.above {
                        (y - LABEL_LINE_GAP, 1.0)
                    } else {
                        (y + LABEL_LINE_GAP, 0.0)
                    };
                    if label.force {
                        force_items.push((dy, ay, label));
                        continue;
                    }
                    let fg = if label.color == ORDER_LABEL_NEUTRAL {
                        label_neutral
                    } else {
                        color(label.color)
                    };
                    let m = draw_label_text_run(
                        &mut self.text_runs,
                        &mut self.text_run_cursor,
                        ctx,
                        self.label_font_delta,
                        &label.text,
                        label_x,
                        dy,
                        1.0,
                        ay,
                        fg,
                    )?;
                    placed.push(PlacedLabel {
                        x: label_x,
                        y: dy,
                        ax: 1.0,
                        ay,
                        w: m.width.as_f32(),
                        h: m.line_height.as_f32(),
                        solid: false,
                    });
                }
                for (dy, ay, label) in force_items {
                    let fg = if label.color == ORDER_LABEL_NEUTRAL {
                        label_neutral
                    } else {
                        color(label.color)
                    };
                    let m = draw_label_text_run(
                        &mut self.text_runs,
                        &mut self.text_run_cursor,
                        ctx,
                        self.label_font_delta,
                        &label.text,
                        label_x,
                        dy,
                        1.0,
                        ay,
                        fg,
                    )?;
                    placed.push(PlacedLabel {
                        x: label_x,
                        y: dy,
                        ax: 1.0,
                        ay,
                        w: m.width.as_f32(),
                        h: m.line_height.as_f32(),
                        solid: false,
                    });
                }
            }

            // Moonbot `LastSellOrderPriceVol`: отдельная подпись глубины стакана у sell-линии.
            // Это НЕ текст ордера, а сумма book notional до цены закрытия:
            // long → ask ниже sell, short → bid выше sell. Рисуем в зоне стакана; курсорный
            // readout ниже идёт поверх неё, если пользователь навёлся в ту же точку.
            if orderbook_enabled && self.line_labels && !self.panes[idx].orderbook_levels.is_empty()
            {
                let right_x = zone_left + READOUT_PAD_X;
                let label_line_h = self.label_font_px() + 4.0;
                for label in &self.panes[idx].orderbook_labels {
                    let y = line_y(label.price);
                    if y < plot_top - label_line_h || y > plot_bottom + label_line_h {
                        continue;
                    }
                    let q = label.notional;
                    let text = fmt_amount(q);
                    let col = if q <= 1e-6 {
                        color(self.label_positive)
                    } else {
                        color(self.label_negative)
                    };
                    let dy = y - 2.0;
                    let m = draw_label_text_run(
                        &mut self.text_runs,
                        &mut self.text_run_cursor,
                        ctx,
                        self.label_font_delta,
                        &text,
                        right_x,
                        dy,
                        0.0,
                        1.0,
                        col,
                    )?;
                    placed.push(PlacedLabel {
                        x: right_x,
                        y: dy,
                        ax: 0.0,
                        ay: 1.0,
                        w: m.width.as_f32(),
                        h: m.line_height.as_f32(),
                        solid: false,
                    });
                }
            }

            // Per-вкладка галка «подпись у перекрестия» (попап ⚙). Выкл → курсорный ридаут не рисуем.
            let cursor = self
                .cursor
                .filter(|cursor| cursor.pane == idx)
                .filter(|_| self.cursor_labels);
            let mut skip_time_label_x = None;
            let mut skip_price_label_y = None;

            if let Some(cursor) = cursor {
                let cx_log = (self.slot_origin[0] + cursor.local[0]) / sf;
                let cy_log = (self.slot_origin[1] + cursor.local[1]) / sf;

                if cx_log >= plot_left && cx_log <= plot_right {
                    let unix = left_unix + (cx_log - plot_left) as f64 / time_to_px as f64;
                    let now_ms = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map_or(0.0, |d| d.as_millis() as f64);
                    // Не сегодняшний день → «ДД.ММ ЧЧ:ММ:СС» (большие ТФ/окна).
                    let label =
                        moon_chart::axes::fmt_clock_dated(unix, tz_offset_sec, true, now_ms);
                    let metrics = self.measure_label_text(ctx, &label);
                    let width = metrics.width.as_f32();
                    let line_h = metrics.line_height.as_f32();
                    if (self.panes[idx].readout_time_width - width).abs() > 0.25
                        || (self.panes[idx].readout_time_line_h - line_h).abs() > 0.25
                    {
                        self.panes[idx].readout_time_width = width;
                        self.panes[idx].readout_time_line_h = line_h;
                        readout_metrics_changed = true;
                    }
                    let half_w = metrics.width.as_f32() * 0.5;
                    let x = clamp_anchor(
                        cx_log,
                        plot_left + half_w + READOUT_PAD_X + READOUT_INSET,
                        plot_right - half_w - READOUT_PAD_X - READOUT_INSET,
                    );
                    let y = pane_bottom - 1.0;
                    let dst = readout_rect_dst(x, y, metrics, 0.5, 1.0, sf);
                    self.draw_label_text(ctx, &label, x, y, 0.5, 1.0, readout)?;
                    skip_time_label_x = Some(rect_x_range_log(dst, sf));
                }

                if !axis_hidden && cy_log >= plot_top && cy_log <= plot_bottom {
                    let price = y_min + (plot_bottom - cy_log) / price_to_px.max(1e-6);
                    let label = format!("{price:.dec$}");
                    let metrics = self.measure_label_text(ctx, &label);
                    let width = metrics.width.as_f32();
                    let line_h = metrics.line_height.as_f32();
                    if (self.panes[idx].readout_price_width - width).abs() > 0.25
                        || (self.panes[idx].readout_price_line_h - line_h).abs() > 0.25
                    {
                        self.panes[idx].readout_price_width = width;
                        self.panes[idx].readout_price_line_h = line_h;
                        readout_metrics_changed = true;
                    }
                    // Right → плашка у правого края панели (за стаканом); Left → у левого жёлоба.
                    let x = if axis_on_right {
                        pane_right - 3.0
                    } else {
                        (plot_left - 3.0)
                            .max(pane_left + READOUT_INSET + READOUT_PAD_X + metrics.width.as_f32())
                    };
                    let dst = readout_rect_dst(x, cy_log, metrics, 1.0, 0.5, sf);
                    self.draw_label_text(ctx, &label, x, cy_log, 1.0, 0.5, readout)?;
                    skip_price_label_y = Some(rect_y_range_log(dst, sf));
                }

                // Подписи у крестовины. Размер ордера ($) — СЛЕВА от разделителя (сторона графика),
                // прижат правым краем к разделителю, на линии курсора. Объём стакана и % — СПРАВА от
                // разделителя (в зоне стакана): объём НАД линией, % ПОД линией. Цвет всех трёх единый:
                // курсор ниже текущей цены → зелёный, выше → красный.
                if cy_log >= plot_top && cy_log <= plot_bottom {
                    let cursor_price = y_min + (plot_bottom - cy_log) / price_to_px.max(1e-6);
                    // Опора % и цвета курсора — БЛИЖНЯЯ сторона стакана, НЕ last (как в Moonbot):
                    // курсор ниже цены → лучший бид, выше → лучший аск. Расстояние считается от
                    // цены исполнения на своей стороне книги — поэтому для лонга/шорта опора разная
                    // (спред сдвигает %). Нет стакана/цены → фолбэк на last (прежнее поведение).
                    let cursor_ref = cached_last_price.filter(|l| *l > 0.0).map(|last| {
                        let levels = &self.panes[idx].orderbook_levels;
                        let best_bid = levels
                            .iter()
                            .filter(|l| !l.is_ask)
                            .map(|l| l.price)
                            .fold(f32::NEG_INFINITY, f32::max);
                        let best_ask = levels
                            .iter()
                            .filter(|l| l.is_ask)
                            .map(|l| l.price)
                            .fold(f32::INFINITY, f32::min);
                        if cursor_price >= last {
                            if best_ask.is_finite() { best_ask } else { last }
                        } else if best_bid.is_finite() {
                            best_bid
                        } else {
                            last
                        }
                    });
                    let cur_col = cursor_ref
                        .map(|r| {
                            pct_hsla(r - cursor_price, self.label_positive, self.label_negative)
                        })
                        .unwrap_or(readout);
                    let right_x = zone_left + READOUT_PAD_X;
                    // Зазор от линии: плашка подписи не должна резать горизонталь перекрестия.
                    let gap = cursor_label_gap(self.cursor_thickness, sf);
                    // Курсорные цифры — приоритетные, на переднем плане, в столбики НЕ входят
                    // (рисуются на своём фикс. месте у крестовины), но получают плотную подложку.
                    // Размер ордера — НАД линией курсора, слева от разделителя, правым краем.
                    // Без $/K-M, всегда 2 знака после запятой («100.00»).
                    if let Some(usd) = prospective_usd {
                        let text = format!("{usd:.2}");
                        let m = self.draw_label_text(
                            ctx,
                            &text,
                            label_x,
                            cy_log - gap,
                            1.0,
                            1.0,
                            cur_col,
                        )?;
                        placed.push(PlacedLabel {
                            x: label_x,
                            y: cy_log - gap,
                            ax: 1.0,
                            ay: 1.0,
                            w: m.width.as_f32(),
                            h: m.line_height.as_f32(),
                            solid: true,
                        });
                    }
                    // Объём стакана на уровне курсора — правее разделителя, над линией.
                    if orderbook_enabled && !self.panes[idx].orderbook_levels.is_empty() {
                        let tol = 6.0 / price_to_px.max(1e-6);
                        if let Some(q) = nearest_orderbook_notional(
                            &self.panes[idx].orderbook_levels,
                            cursor_price,
                            tol,
                        ) {
                            let m = self.draw_label_text(
                                ctx,
                                &fmt_amount(q),
                                right_x,
                                cy_log - gap,
                                0.0,
                                1.0,
                                cur_col,
                            )?;
                            placed.push(PlacedLabel {
                                x: right_x,
                                y: cy_log - gap,
                                ax: 0.0,
                                ay: 1.0,
                                w: m.width.as_f32(),
                                h: m.line_height.as_f32(),
                                solid: true,
                            });
                        }
                    }
                    // % отклонения курсора от опоры (ближняя сторона стакана) — правее
                    // разделителя, под линией.
                    if let Some(r) = cursor_ref {
                        if r > 0.0 {
                            let pct = (cursor_price - r) / r * 100.0;
                            let m = self.draw_label_text(
                                ctx,
                                &fmt_pct(pct),
                                right_x,
                                cy_log + gap,
                                0.0,
                                0.0,
                                cur_col,
                            )?;
                            placed.push(PlacedLabel {
                                x: right_x,
                                y: cy_log + gap,
                                ax: 0.0,
                                ay: 0.0,
                                w: m.width.as_f32(),
                                h: m.line_height.as_f32(),
                                solid: true,
                            });
                        }
                    }
                }
            } else {
                // Нет реального курсора на панели → призрак compare-режима (объём/% на цене
                // соседа). При реальном курсоре призрак не рисуем — хелпер сам проверяет.
                self.draw_ghost_cursor_labels(ctx, idx, sf, &mut placed)?;
            }

            // Готовая раскладка подписей кадра → плашки-подложки строит sync_readout_params.
            // При зуме Y меняется даже если текст/ширина прежние, поэтому сравниваем раскладку:
            // иначе подложки остаются на старой цене и визуально «висят в воздухе».
            if previous_placed != placed {
                self.panes[idx].label_placed = placed;
                readout_metrics_changed = true;
            } else {
                self.panes[idx].label_placed = previous_placed;
            }

            // Подписи цены: на фикс. долях высоты — совпадают со СТАТИЧНЫМИ горизонталями сетки
            // (модель Moonbot: сетка стоит, едут только подписи). Цена НЕкруглая — показываем ту,
            // что попала на линию (как ось времени показывает некруглые метки на фикс. вертикалях).
            // Рисуем внутренние линии (края у рамки плота не подписываем). Пропуск подписи, если
            // она перекрыта курсорным ридаутом.
            let min_v_gap = LINE_H;
            let mut last_y = f32::INFINITY;
            let n_horiz = GRID_N_HORIZ as i32;
            for k in 1..n_horiz {
                if axis_hidden {
                    break;
                }
                let frac = k as f32 / GRID_N_HORIZ;
                let y = (plot_top + frac * plot_h).round();
                let price = y_min + (plot_bottom - y) / price_to_px.max(1e-6);
                let overlaps_readout = skip_price_label_y
                    .is_some_and(|(top, bottom)| y >= top - 1.0 && y <= bottom + 1.0);
                if y >= plot_top - 1.0
                    && y <= plot_bottom + 1.0
                    && !overlaps_readout
                    && (last_y - y).abs() >= min_v_gap
                {
                    let label = format!("{price:.dec$}");
                    self.draw_text(ctx, &label, axis_label_x, y, 1.0, 0.5, ink)?;
                    last_y = y;
                }
            }

            // Метки времени — на КРУГЛЫХ границах локального времени (nice_time_step:
            // 1с..6ч под ~6 подписей). Раньше метки стояли на фикс. долях ширины окна →
            // некруглые времена с плавающим шагом (19:46, 19:56, 20:05 — то +9, то +10).
            let step_ms =
                (moon_chart::axes::nice_time_step(window_ms / 1000.0, 6.0) * 1000.0).max(1000.0);
            let with_sec = step_ms < 60_000.0;
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0.0, |d| d.as_millis() as f64);
            let tz_ms = tz_offset_sec as f64 * 1000.0;
            let right_unix = left_unix + window_ms;
            // Первая круглая граница в окне (выравнивание по ЛОКАЛЬНОМУ времени).
            let mut tick_unix = ((left_unix + tz_ms) / step_ms).ceil() * step_ms - tz_ms;
            // Прореживание по горизонтали: при узком окне подписи налезают друг на друга —
            // рисуем подпись, только если её левый край отстоит от ПРАВОГО края предыдущей
            // нарисованной (иначе пропуск → «через одну»).
            let min_h_gap = 6.0;
            let mut last_right = f32::NEG_INFINITY;
            while time_axis_visible && tick_unix <= right_unix + 0.5 {
                let unix = tick_unix;
                tick_unix += step_ms;
                // Правые ~10% окна — «будущее» за живым краем: время, которого ещё нет,
                // не подписываем (сбивало с толку у границы стакана).
                if now_ms > 0.0 && unix > now_ms {
                    break;
                }
                let x = plot_left + ((unix - left_unix) / window_ms) as f32 * plot_w;
                // Подписи оси на не-сегодняшних сутках получают дату «ДД.ММ» — без неё
                // на широких окнах метки читались как идущие «назад» (шаг > суток).
                let label =
                    moon_chart::axes::fmt_clock_dated(unix, tz_offset_sec, with_sec, now_ms);
                let metrics = self.measure_text(ctx, &label);
                let half_w = metrics.width.as_f32() * 0.5;
                let left = x - half_w;
                let right = x + half_w;
                let overlaps_readout = skip_time_label_x.is_some_and(|(skip_left, skip_right)| {
                    right >= skip_left && left <= skip_right
                });
                if !overlaps_readout && left >= last_right + min_h_gap && left >= plot_left - 1.0 {
                    self.draw_text(ctx, &label, x, pane_bottom - 2.0, 0.5, 1.0, ink)?;
                    last_right = right;
                }
            }
        }

        if readout_metrics_changed {
            self.sync_readout_params();
            self.needs_present = true;
        }

        if self.text_run_cursor < self.text_runs.len() {
            for run in &mut self.text_runs[self.text_run_cursor..] {
                run.clear();
            }
        }
        Ok(())
    }
}
