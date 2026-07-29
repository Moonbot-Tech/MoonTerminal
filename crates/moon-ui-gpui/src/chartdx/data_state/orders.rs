//! Synchronizes session orders and order-line labels.

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
        // A pane-count change, including removal of the last market, must dirty the base. Otherwise
        // the base cache keeps blitting the old chart through the empty slot because the logo is
        // transparent. This mirrors the check in sync_from_market_source.
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
            // Resolve the core name for the corner label here, where the session is available. It
            // changes only when the pane switches cores, so request presentation only on a change.
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
                .and_then(|core_st| core_st.order_lines.auto_fit_range(&pane.market));
            if pr.cached_order_price != order_price {
                pr.cached_order_price = order_price;
                self.view_dirty = true;
                pixels_changed = true;
            }

            let news_sig = self.news_sig();
            let warn_sig = self.warn_sig();
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
                    || pr.last_news_sig != news_sig
                    || pr.last_warn_sig != warn_sig
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
                    // Add user figures through the same userdata layers after orders, placing them
                    // above order zones but below cursor markers.
                    self.append_figure_geometry(
                        pane.core,
                        &pane.market,
                        pane.view.epoch_ms,
                        &mut hlines,
                        &mut segs,
                        &mut markers,
                    );
                    // News marks ride the same layer, last, so a mark is never hidden under an
                    // order line's cross.
                    self.append_news_geometry(pane.view.epoch_ms, &mut markers);
                    // Warning badges ride the same layer, after news.
                    self.append_warn_geometry(pane.view.epoch_ms, &mut markers);
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
                    pr.last_news_sig = news_sig;
                    pr.last_warn_sig = warn_sig;
                    pr.gpu_prepare_dirty = true;
                    pixels_changed = true;
                }
            } else {
                // The pane's own core carries no data (removed or not yet connected). Orders and
                // figures go away with it, but news marks come from OTHER cores and stay, so this
                // branch still rebuilds them instead of clearing the layer outright.
                if force
                    || pr.last_order_lines_rev != u64::MAX
                    || pr.last_news_sig != news_sig
                    || pr.last_warn_sig != warn_sig
                {
                    if pr.last_order_zone_sig != 0 {
                        pr.last_order_zone_sig = 0;
                        base_changed = true;
                    }
                    let mut markers = Vec::new();
                    self.append_news_geometry(pane.view.epoch_ms, &mut markers);
                    self.append_warn_geometry(pane.view.epoch_ms, &mut markers);
                    pr.layers.set_userdata(&[], &[], &[], &markers);
                    pr.order_labels.clear();
                    pr.order_label_order.clear();
                    pr.orderbook_labels.clear();
                    pr.last_order_lines_rev = u64::MAX;
                    pr.last_order_lines_sync_ms = now;
                    pr.pending_order_gpu_rev = Some(u64::MAX);
                    pr.last_order_highlight_uid = None;
                    pr.last_order_drag_preview = None;
                    pr.last_news_sig = news_sig;
                    pr.last_warn_sig = warn_sig;
                    pr.gpu_prepare_dirty = true;
                    pixels_changed = true;
                }
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

/// Builds market order-line labels for the text layer: size at the buy line, percentage from entry
/// plus sell quantity at the sell line, and stop percentage at the stop line. Long versus short
/// determines whether labels appear above or below the line, matching Moonbot category E. Only
/// open orders receive labels; closed or completed orders do not.
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
        // Put the chart order number on each line's primary label to associate an order's buy,
        // sell, and stop lines, for example "$X [10]", "-5% [10]", and stop "-3% [10]".
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
        // For an unfilled buy entry, place "size [N]" on one line so the number and size do not
        // overlap on the same side. For a filled entry, show only [N]. Entry size is always white,
        // independent of line color and order side.
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
        // For a sell line, show profit percentage from entry using a sign-dependent color and the
        // dollar-notional sell size (remaining * sell price * rate) on the opposite side, matching
        // Moonbot. The primary percentage is always drawn; the remaining caption uses YTextFill.
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
        // Show stop percentage from the buy price above the line for shorts and below for longs.
        // The primary label bypasses YTextFill, matching the Delphi stop-loss label block.
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

/// Returns a level's signed percentage from entry with order side applied. Long values retain their
/// sign, while short values are inverted so a level above entry is negative. This keeps profit green
/// and loss red for either side: a short sell below entry is positive and a stop above it is negative.
fn signed_pct(level: f32, entry: f32, short: bool) -> f32 {
    let raw = (level - entry) / entry * 100.0;
    if short { -raw } else { raw }
}

/// Selects the positive size-label color for longs and the negative color for shorts.
fn side_color(theme: &ChartTheme, short: bool) -> u32 {
    if short {
        rgb_u32(theme.label_negative)
    } else {
        rgb_u32(theme.label_positive)
    }
}

/// Selects the positive color for nonnegative percentages and the negative color otherwise.
fn pct_color(theme: &ChartTheme, v: f32) -> u32 {
    if v >= 0.0 {
        rgb_u32(theme.label_positive)
    } else {
        rgb_u32(theme.label_negative)
    }
}

/// Formats a compact base-unit count with a K/M/B/T SI suffix and at most two fractional digits,
/// trimming trailing zeros. This intentionally does not use shared `compact_si`, which can emit
/// three fractional digits for tens. Examples: 50 becomes "50", 49.744 becomes "49.74", 1234
/// becomes "1.23K", and 49744 becomes "49.74K".
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
            // Moonbot: short sell-line volume uses buy glass above sell price.
            if !level.is_ask && level.price > price {
                sum += level.notional;
            }
        } else {
            // Moonbot: long sell-line volume uses sell glass below sell price.
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
    for label in labels.iter_mut() {
        label.notional = sell_book_notional(levels, label.price, label.short);
    }
}

/// Formats a dollar amount with an SI suffix, for example 1234 as "$1.23K".
fn fmt_usd(v: f64) -> String {
    format!("${}", fmt_size_2dp(v))
}
