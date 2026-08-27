//! Synchronizes session orders and order-line labels.

use super::*;
use moon_core::config::ChartLabelField;

use crate::chartdx::text::fmt_pct;

fn hash_order_zones(zones: &[moon_chart::layers::ZoneInstance]) -> u64 {
    let mut h = 0x9E37_79B9_7F4A_7C15u64 ^ zones.len() as u64;
    for z in zones {
        h = h.rotate_left(5) ^ z.price0.to_bits() as u64;
        h = h.rotate_left(7) ^ z.price1.to_bits() as u64;
        h = h.rotate_left(9) ^ z.t0_rel.to_bits() as u64;
        h = h.rotate_left(13) ^ z.t1_rel.to_bits() as u64;
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

        let labels_cfg = st.chart_labels.clone();
        // A shot needs the venue whether or not the user asked for a venue caption: it is drawn in
        // the CORE NAME's place, so the substitution has nothing to put there without it. Read once
        // here, beside the configuration it overrides, because both answers must hold for the whole
        // sync — a shot arming halfway through would otherwise caption some panes and not others.
        let shot = st.shot_caption_active();
        // Whether this ENGINE draws a frozen picture rather than a live market. Engine-level, not
        // per-pane: the answer belongs to the engine a trade window owns, so it is the same for
        // every pane it holds and is resolved once for the whole sync. One predicate, shared with
        // the market-side gates — see `draws_live_market`.
        let frozen = !self.draws_live_market();
        // Both caption gates are answered ONCE for the sync, not per pane: they read the
        // configuration, which cannot change inside a sync, and a walk over sixteen rows of eight
        // captions per pane per order revision is real work for an answer that never differs.
        let wants_venue_cfg = shot || labels_cfg.any_drawn(|f| f == ChartLabelField::Venue);
        // `!frozen` for the same reason the order lines below are emptied: these captions read the
        // core's CURRENT open position and the strategy behind it. Printed over a trade that closed
        // hours ago they are not stale, they are about a different thing entirely — and a caption
        // is read as describing the picture under it.
        let wants_position_cfg = !frozen
            && labels_cfg.any_drawn(|f| f.uses_pnl_basis() || f == ChartLabelField::OrderStrategy);
        // Which venues have a core behind them, for the column's dimming. Read ONCE for the sync,
        // like the caption gates beside it: it is a property of the connected cores, not of a pane,
        // and a walk per pane would repeat it for every chart in a stack.
        let arb_reachable: Vec<(u8, String)> = labels_cfg
            .any_drawn(|f| f == ChartLabelField::ArbColumn)
            .then(|| {
                session
                    .core_venues()
                    .values()
                    .map(|venue| (venue.id.code, venue.dex.clone()))
                    .collect()
            })
            .unwrap_or_default();
        // The LATEST detect this core fired, which is a live event. Same argument as the position
        // captions above.
        let wants_detect_cfg = !frozen
            && labels_cfg.any_drawn(|f| {
                matches!(
                    f,
                    ChartLabelField::DetectStrategy | ChartLabelField::DetectMsg
                )
            });
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
            // Caption inputs that only the SESSION can answer: the venue behind the core, the open
            // orders on this market and the strategy that placed the newest of them. Collected here
            // for the same reason the core name is — this is where the session is in hand — and
            // formatted only if the caption configuration actually asks for any of it.
            // Through the shared label helper, never a local spelling: naming a venue lives in one
            // place so the caption, the Orders picker and the detect card cannot disagree.
            // Gated on the configuration, answered once above: a caption nobody asked for must not
            // cost a venue lookup or a walk over the core's whole order array.
            // Two spellings on purpose. A shot takes the SECTION label, which is never empty: it
            // answers with the shared "not identified" wording for a core that has not finished
            // `BaseCheck`, and an empty string there would drop the caption altogether and leave a
            // picture with no attribution at all. A configured venue caption keeps `venue_label`,
            // which resolves to nothing when the venue cannot be named, because a caption the user
            // asked for should stay absent rather than print a placeholder on every frame.
            let venue = wants_venue_cfg
                .then(|| {
                    let venue = session.core_venues().get(&pane.core);
                    match shot {
                        true => Some(crate::controls::venue_section_label(venue)),
                        false => venue.map(crate::controls::venue_label),
                    }
                })
                .flatten()
                .unwrap_or_default();
            pr.venue = venue;
            let (basis, strategy) = wants_position_cfg
                .then(|| {
                    session.store().core(pane.core).map(|core_st| {
                        crate::chartdx::text::collect_open_stats(&core_st.orders, &pane.market)
                    })
                })
                .flatten()
                .unwrap_or_default();
            // Deliberately NOT `pixels_changed`: these move with every mark tick, while the caption
            // they feed is printed to two decimals. `refresh_pane_labels` below compares the
            // FORMATTED result and is the one that decides whether anything has to repaint.
            pr.label_basis = basis;
            pr.label_strategy = strategy;
            // The newest detect THIS core fired on THIS market, straight off the store's index —
            // the ring itself is two thousand rows and this runs per pane on every order revision.
            let (detect_strategy, detect_msg) = wants_detect_cfg
                .then(|| {
                    let core_st = session.store().core(pane.core)?;
                    let det = core_st.latest_detect.get(&pane.market)?;
                    Some((det.strat_name.clone(), det.msg.clone()))
                })
                .flatten()
                .unwrap_or_default();
            pr.label_arb_reachable = arb_reachable.clone();
            pr.label_detect_strategy = detect_strategy;
            pr.label_detect_msg = detect_msg;
            let device_gen = pr.layers.device_gen();
            let device_lost = pr.last_device_gen != device_gen;
            if device_lost {
                pr.last_order_lines_rev = u64::MAX;
                pr.gpu_prepare_dirty = true;
                pixels_changed = true;
            }

            // An engine holding a FROZEN REPLAY draws no LIVE orders, and this is the only place
            // that can enforce it: the buttons and the click gestures are gone from the trade
            // window, but the order LAYER is built here, from the session store, for every pane
            // this engine owns. Left alone it would paint the user's current buy and sell lines,
            // their zones and their labels across a chart of a trade that closed hours ago — live
            // prices over past candles, which is not decoration but a wrong picture.
            //
            // Only the ORDER source is emptied. Figures, news marks, warning badges and — the whole
            // point of this window — the trade-history ARROWS ride the same userdata pass further
            // down and are untouched.
            //
            // `None` for the live chart is impossible here: `trade_replay` is set only on an engine
            // a trade window owns, so every other chart in the application takes the same branch it
            // always did.
            let no_orders = moon_core::session::order_lines::OrderLineStore::default();
            // The auto-Y fit goes with them. A fit stretched to reach a live order's price would
            // squash the very candles the window exists to show.
            let order_price = match frozen {
                true => None,
                false => session
                    .store()
                    .core(pane.core)
                    .and_then(|core_st| core_st.order_lines.auto_fit_range(&pane.market)),
            };
            if pr.cached_order_price != order_price {
                pr.cached_order_price = order_price;
                self.view_dirty = true;
                pixels_changed = true;
            }

            let news_sig = self.news_sig();
            let warn_sig = self.warn_sig();
            let trade_history_sig = self.trade_history_sig(&pane.view);
            if let Some(core_st) = session.store().core(pane.core) {
                // The ONE order source both consumers below read. Emptied on a frozen engine, so
                // neither the geometry nor the labels can reach a live order.
                let order_lines = match frozen {
                    true => &no_orders,
                    false => &core_st.order_lines,
                };
                // Both are order-line state, so they follow the order lines out. Input to a
                // historical chart is already gated, but a hover or a drag left over from before
                // the replay attached must not privilege a label that is no longer drawn.
                let highlight_uid = self
                    .order_highlight
                    .and_then(|(core, uid)| (core == pane.core && !frozen).then_some(uid));
                let drag_preview = self
                    .order_drag_preview
                    .and_then(|(core, uid, kind, price)| {
                        (core == pane.core && !frozen).then_some((uid, kind, price))
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
                    || pr.last_trade_history_sig != trade_history_sig
                    || pr.last_warn_sig != warn_sig
                {
                    let mut hlines = Vec::new();
                    let mut segs = Vec::new();
                    let mut markers = Vec::new();
                    let mut zones = Vec::new();
                    moon_chart::build_order_geometry(
                        order_lines,
                        &pane.market,
                        &self.orders,
                        &self.chart_graphics,
                        self.last_ppp,
                        highlight_uid,
                        drag_preview,
                        pane.view.epoch_ms,
                        now,
                        f32::NEG_INFINITY,
                        f32::INFINITY,
                        0.0,
                        // Per-chart, from this panel's candle popup. A change reaches here because
                        // the panel marks the view dirty and the render path then forces this sync.
                        self.candle_view.moonshot_zone,
                        &mut zones,
                        &mut hlines,
                        &mut segs,
                        &mut markers,
                    );
                    // Add user figures through the same userdata layers after orders, placing them
                    // above order zones but below cursor markers. Their FILLS join the zone layer
                    // (drawn over the grid, under the candles) before `hash_order_zones` reads it —
                    // a fill that appears or moves must invalidate the base cache exactly like an
                    // order zone.
                    self.append_figure_geometry(
                        pane.core,
                        &pane.market,
                        pane.view.epoch_ms,
                        &mut moon_chart::figures::FigureBuffers {
                            zones: &mut zones,
                            hlines: &mut hlines,
                            segs: &mut segs,
                            markers: &mut markers,
                            labels: &mut pr.figure_labels,
                        },
                    );
                    // News marks, then trade history, then warning badges ride the same layer
                    // after the orders, so none of them is hidden under an order line's cross.
                    // The order among these three is their stacking order, last one on top.
                    self.append_news_geometry(pane.view.epoch_ms, &mut markers);
                    pr.trade_geometry = self.append_trade_history_geometry(
                        *idx,
                        pane.core,
                        &pane.view,
                        &mut markers,
                        &mut segs,
                    );
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
                        order_lines,
                        &pane.market,
                        &self.theme,
                        quote_usd,
                        drag_preview,
                        highlight_uid,
                    );
                    rebuild_order_label_order(&mut pr.order_label_order, &pr.order_labels);
                    // The rebuilt labels carry no volume yet. Measuring it here would take the
                    // market-source lock on a path that deliberately avoids it (`quote_usd_rate`),
                    // so ask the book path for it with the `u64::MAX` sentinel and wake that path
                    // — gated on `view_dirty`, it would never run on a quiet market. Only worth
                    // either when the pane actually has sell lines.
                    if !pr.orderbook_labels.is_empty() {
                        pr.last_label_book_rev = u64::MAX;
                        self.view_dirty = true;
                    }
                    pr.last_order_lines_rev = core_st.order_lines_rev;
                    pr.last_order_lines_sync_ms = now;
                    pr.pending_order_gpu_rev = Some(core_st.order_lines_rev);
                    pr.last_order_highlight_uid = highlight_uid;
                    pr.last_order_drag_preview = drag_preview_sig;
                    pr.last_figures_sig = figures_sig;
                    pr.last_news_sig = news_sig;
                    pr.last_trade_history_sig = trade_history_sig;
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
                    || pr.last_trade_history_sig != trade_history_sig
                    || pr.last_warn_sig != warn_sig
                {
                    if pr.last_order_zone_sig != 0 {
                        pr.last_order_zone_sig = 0;
                        base_changed = true;
                    }
                    let mut markers = Vec::new();
                    // Trade history outlives its core: the replica is durable, so a pane whose core
                    // was removed still draws its closed trades — and their connectors, which is why
                    // this branch carries a real segment buffer rather than an empty slice.
                    let mut segs = Vec::new();
                    self.append_news_geometry(pane.view.epoch_ms, &mut markers);
                    pr.trade_geometry = self.append_trade_history_geometry(
                        *idx,
                        pane.core,
                        &pane.view,
                        &mut markers,
                        &mut segs,
                    );
                    self.append_warn_geometry(pane.view.epoch_ms, &mut markers);
                    pr.layers.set_userdata(&[], &[], &segs, &markers);
                    pr.order_labels.clear();
                    pr.order_label_order.clear();
                    pr.orderbook_labels.clear();
                    // Figure readouts live beside the order labels and must go with them: the
                    // text pass would otherwise keep drawing a label whose line is no longer there.
                    pr.figure_labels.clear();
                    pr.last_order_lines_rev = u64::MAX;
                    pr.last_figures_sig = u64::MAX;
                    pr.last_order_lines_sync_ms = now;
                    pr.pending_order_gpu_rev = Some(u64::MAX);
                    pr.last_order_highlight_uid = None;
                    pr.last_order_drag_preview = None;
                    pr.last_news_sig = news_sig;
                    pr.last_trade_history_sig = trade_history_sig;
                    pr.last_warn_sig = warn_sig;
                    pr.gpu_prepare_dirty = true;
                    pixels_changed = true;
                }
            }
            pr.last_device_gen = device_gen;
        }

        // Captions read both the session and the market, and this is the session half's revision.
        for (idx, _) in &layout {
            if st.refresh_pane_labels(*idx) {
                pixels_changed = true;
            }
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
// Eight arguments, and they are eight separate facts a label needs: the two sinks, the store and
// market it reads, the theme it colours from, the rate it converts with, and the two pointer states
// (`drag_preview`, `highlight_uid`) that decide which labels are privileged. Bundling them would
// invent a type for one call site. `build_order_geometry` next door carries the same allow.
#[allow(clippy::too_many_arguments)]
fn build_order_labels(
    out: &mut Vec<OrderLabel>,
    book_out: &mut Vec<OrderBookLabel>,
    store: &moon_core::session::order_lines::OrderLineStore,
    market: &str,
    theme: &ChartTheme,
    quote_usd: Option<f64>,
    drag_preview: Option<(u64, LineKind, f32)>,
    highlight_uid: Option<u64>,
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
        // The flags are functions of the LINE and the order, so the caller names the line and not
        // the flags: `force` is "this is the leg being dragged", `highlighted` is "this order is
        // under the pointer", and `pinned` is `line_is_pinned`'s answer for it — the same one the
        // geometry and the hit test get.
        let mut push =
            |kind: LineKind, price: f32, text: String, above: bool, color: u32, priority: u8| {
                if price.is_finite() && price > 0.0 && !text.is_empty() {
                    out.push(OrderLabel {
                        price,
                        text,
                        above,
                        color,
                        priority,
                        force: line_forced(kind),
                        highlighted: highlight_uid == Some(o.uid),
                        pinned: moon_chart::order_geometry::line_is_pinned(o, kind),
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
            let text = if o.fill_pct <= 0.0 && o.size > 0.0 {
                let amount = match quote_usd {
                    Some(rate) if rate > 0.0 => fmt_usd(o.size as f64 * bp as f64 * rate),
                    _ => fmt_amount(o.size),
                };
                with_tag(amount)
            } else {
                tag.clone()
            };
            push(
                LineKind::Buy,
                bp,
                text,
                !short,
                ORDER_LABEL_NEUTRAL,
                PRIO_BUY,
            );
        }
        // For a sell line, show profit percentage from entry using a sign-dependent color and the
        // dollar-notional sell size (remaining * sell price * rate) on the opposite side, matching
        // Moonbot. The primary percentage is always drawn; the remaining caption uses YTextFill.
        if let Some(sp) = sell {
            if sp.is_finite() && sp > 0.0 {
                book_out.push(OrderBookLabel {
                    price: sp,
                    short,
                    notional: None,
                });
            }
            if let Some(bp) = buy {
                if bp > 0.0 {
                    let pct = signed_pct(sp, bp, short);
                    push(
                        LineKind::Sell,
                        sp,
                        with_tag(fmt_pct(pct)),
                        short,
                        pct_color(theme, pct),
                        PRIO_SELL_PCT,
                    );
                }
            }
            let remaining = o.exit_size();
            if remaining > 0.0 && sp > 0.0 {
                let amount = match quote_usd {
                    Some(rate) if rate > 0.0 => fmt_usd(remaining as f64 * sp as f64 * rate),
                    _ => fmt_amount(remaining),
                };
                push(
                    LineKind::Sell,
                    sp,
                    amount,
                    !short,
                    side_color(theme, short),
                    PRIO_SELL_SIZE,
                );
            }
        }
        // Show stop percentage from the buy price above the line for shorts and below for longs.
        // The primary label bypasses YTextFill, matching the Delphi stop-loss label block.
        if let (Some(stp), Some(bp)) = (stop, buy) {
            if bp > 0.0 {
                let pct = signed_pct(stp, bp, short);
                push(
                    LineKind::Stop,
                    stp,
                    with_tag(fmt_pct(pct)),
                    short,
                    pct_color(theme, pct),
                    PRIO_STOP_PCT,
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

fn rebuild_order_label_order(order: &mut Vec<usize>, labels: &[OrderLabel]) {
    order.clear();
    order.extend(0..labels.len());
    order.sort_by_key(|&ix| labels[ix].priority);
}

/// Marks every sell-line depth label unmeasured, for a pane with no book to measure against: the
/// market view went away with the core, or the order book was switched off.
///
/// The label's visibility follows the figure, so leaving the last measurement standing would keep
/// a number on screen describing glass that is no longer drawn or received.
pub(in crate::chartdx) fn clear_orderbook_label_notionals(labels: &mut [OrderBookLabel]) {
    for label in labels.iter_mut() {
        label.notional = None;
    }
}

/// Measures each sell line's order-book volume against the LIVE book, matching Moonbot.
///
/// Reads the whole book rather than the pane's visible slice (`orderbook_levels`): the label
/// answers how much glass sits between price and the line, which is a property of the market, and
/// the visible slice made that figure change as the user panned.
///
/// A side the book does not carry stays `None` and draws nothing. An empty view is installed the
/// moment a market opens, so measuring it would put a green `0` — "no glass to clear" — under
/// every sell line before any data arrives.
pub(super) fn refresh_orderbook_label_notionals(
    labels: &mut [OrderBookLabel],
    book: &moon_core::data::OrderBookModel,
) {
    for label in labels.iter_mut() {
        // A long's sell line clears the asks below it; a short's clears the bids above it.
        let asks = !label.short;
        label.notional = book.side_notional_toward(label.price, asks);
    }
}

/// Formats a dollar amount with an SI suffix, for example 1234 as "$1.23K".
fn fmt_usd(v: f64) -> String {
    format!("${}", fmt_size_2dp(v))
}
