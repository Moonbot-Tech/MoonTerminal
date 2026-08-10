//! Main `prepare_text` implementation for axes, order-line labels, and cursor readouts.

use moon_chart::axes::price_decimals;
use moon_chart::figures::LabelValue as FigLabelValue;
use moon_core::figures::LabelPlace as FigLabelPlace;

use super::*;

impl RenderState {
    /// Prepares axis, order, cursor, and corner-caption text runs for the current frame.
    ///
    /// This is also the sole owner of the caption's backing-plate geometry: publishing the finished
    /// rectangle with the measured runs keeps the later readout pass from repeating layout rules.
    ///
    /// Args:
    ///     ctx: Text context used to measure and retain GPU text runs.
    ///
    /// Returns:
    ///     `Ok(())` after all visible pane text has been prepared.
    ///
    /// Errors:
    ///     Propagates failures from measuring or drawing retained text runs.
    pub(crate) fn prepare_text(
        &mut self,
        ctx: &mut GpuCanvasTextContext<'_>,
    ) -> anyhow::Result<()> {
        self.text_run_cursor = 0;
        let sf = ctx.scale_factor().max(0.1);
        let ink = color(self.axis_label);
        let readout = color(self.readout_label);
        let label_neutral = color(self.label_neutral);
        // The corner caption uses a dedicated chart-theme color without a backdrop.
        let caption_fg = color(self.caption_label);
        let mut firetest_text_drawn = false;
        let mut readout_metrics_changed = false;

        for idx in 0..self.panes.len() {
            let (
                active,
                pane_bounds,
                view,
                epoch_ms,
                core_name,
                ticker,
                orderbook_enabled,
                price_axis_pos,
                time_axis_visible,
                volume_stats,
            ) = {
                let pr = &self.panes[idx];
                (
                    pr.active,
                    pr.pane_bounds,
                    pr.view,
                    pr.epoch_ms,
                    pr.core_name.clone(),
                    pr.ticker.clone(),
                    pr.orderbook_enabled,
                    pr.price_axis_pos,
                    pr.time_axis_visible,
                    pr.layers.volume_stats(),
                )
            };
            if !active {
                continue;
            }
            let cached_last_price = self.panes[idx].cached_last_price;
            let prospective_usd = self.panes[idx].prospective_usd;
            // Label layout for this frame, used by badges in sync_readout_params. Retain the old
            // layout for comparison: zoom changes Y, so backdrops must move with their text.
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
            // Price-axis side: Left places labels in the gutter left of the plot; Right places
            // them at the panel's right edge (the gutter beyond the order book); Hide omits the
            // axis. All variants anchor text by its right edge (alignment 1.0).
            use crate::persistence::chart_persist::PriceAxisPos;
            let axis_hidden = matches!(price_axis_pos, PriceAxisPos::Hide);
            let axis_on_right = matches!(price_axis_pos, PriceAxisPos::Right);
            let axis_label_x = if axis_on_right {
                pane_right - 4.0
            } else {
                plot_left - 4.0
            };

            // Corner caption: the coin, then the core name that qualifies it, over a translucent
            // plate this block also publishes. Its zone comes from `text::caption`, the one home of
            // that geometry. Drawn before the `plot_w` gate so the caption remains above the book in
            // collapsed book-only mode.
            let caption_zone = caption_geom(
                pane_left,
                pane_right,
                plot_left,
                plot_right,
                plot_top,
                orderbook_enabled,
                self.panes[idx].orderbook_view.bounds[0] / sf,
                CAPTION_PAD_X,
                CAPTION_PAD_Y,
            );
            // A pane with no room draws NO caption at all — not the rows suppressed while the broom
            // delta and the scale badge still paint at substitute coordinates.
            if let Some(zone) = caption_zone {
                // Where each line goes is `caption::caption_layout`'s decision, not this pass's:
                // over a book with chart to its left the coin centres on the book and the core name
                // hangs off the book's left edge, and everywhere else both lines share one column.
                let lay = caption_layout(&zone, plot_left);
                let cap_y = zone.top_y;
                // One bounding box per column. Split, the two plates are separate — a single plate
                // spanning both would darken the whole run of candles lying between them.
                let mut coin_box = CaptionBox::default();
                let mut core_box = CaptionBox::default();
                let mut block_h = 0.0_f32;
                // The coin's own line height and the core name's drawn width, both needed once the
                // two share a row: the coin is set larger, so centring the name against it needs
                // its height, and the scale badge hangs off the name's left edge rather than the
                // column's.
                let mut coin_h = 0.0_f32;
                let mut core_w = 0.0_f32;
                // The coin leads, one size up: it is the fact a glance needs, and the core name is
                // context for it.
                let coin_size = self.label_font_px() * 1.25;
                if !ticker.is_empty() {
                    let (text, w) = crate::design::fit_text(&ticker, lay.coin_max_w, |s| {
                        measure_run_width(ctx, s, coin_size)
                    });
                    let m = self.draw_sized_text(
                        ctx,
                        &text,
                        coin_size,
                        lay.coin_x,
                        cap_y,
                        lay.coin_ax,
                        0.0,
                        caption_fg,
                    )?;
                    coin_box.add(
                        lay.coin_x - w * lay.coin_ax,
                        w,
                        cap_y,
                        m.line_height.as_f32(),
                    );
                    coin_h = m.line_height.as_f32();
                    block_h += coin_h;
                }
                // Split, the two sit SIDE BY SIDE and share a row, so the smaller core name is
                // centred against the coin's taller line rather than hung from the same top edge —
                // aligned by their tops the name reads as having slipped downward. Unsplit they
                // remain stacked, the name on the second line.
                let core_y = if lay.split {
                    cap_y + ((coin_h - LINE_H) * 0.5).max(0.0)
                } else {
                    cap_y + block_h
                };
                if !core_name.is_empty() {
                    let (text, w) = crate::design::fit_text(&core_name, lay.core_max_w, |s| {
                        measure_run_width(ctx, s, FONT_SIZE)
                    });
                    self.draw_text(ctx, &text, lay.core_x, core_y, lay.core_ax, 0.0, caption_fg)?;
                    core_w = w;
                    column(lay.split, &mut coin_box, &mut core_box).add(
                        lay.core_x - w * lay.core_ax,
                        w,
                        core_y,
                        LINE_H,
                    );
                }
                // Broom mode: show this exchange's last-price difference from the locked anchor
                // prominently below the caption, with sign and positive/negative color. Both
                // sides are live: this pane provides its last price, while the stack supplies
                // the anchor's last price through `set_compare_ref_price` in apply_compare on
                // every observation.
                {
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
                        // Follows the coin: the delta belongs to the same instrument, so the two
                        // read as one column wherever that column ended up.
                        let top = cap_y + block_h + 2.0;
                        let m = self.draw_sized_text(
                            ctx,
                            &text,
                            size,
                            lay.coin_x,
                            top,
                            lay.coin_ax,
                            0.0,
                            col,
                        )?;
                        let w = m.width.as_f32();
                        coin_box.add(lay.coin_x - w * lay.coin_ax, w, top, m.line_height.as_f32());
                    }
                };
                // Where the scale badge hangs its right edge. In one-column layout it sits beside
                // the caption; in split layout it aligns with the core name so the left column
                // reads as a coherent block instead of a stray figure.
                let badge_right = if lay.split {
                    // Beside the core name on the same row, not above it: the name no longer sits
                    // on a line of its own for the badge to occupy.
                    lay.core_x - core_w - CAPTION_SCALE_GAP
                } else {
                    coin_box.left_or(lay.coin_x) - CAPTION_SCALE_GAP
                };
                // Place the current Y-scale badge LEFT of the caption block, using the caption
                // color and a size 2 px smaller than the broom delta, without a sign. Display an
                // integer percentage such as "14%". sync_from_market_source controls visibility:
                // always in Auto mode, or manually when scales differ.
                if let Some(pct) = self.panes[idx].scale_badge {
                    // A range below a whole 1% in a quiet Auto market is shown as "<1%", not zero.
                    let text = if pct == 0 {
                        "<1%".to_string()
                    } else {
                        format!("{pct}%")
                    };
                    // Keep the broom delta visually dominant over this secondary scale indicator.
                    let size = self.label_font_px() * 1.7 - 2.0;
                    // Right-anchored, so the badge always sits beside its column and never under it.
                    let m = self.draw_sized_text(
                        ctx,
                        &text,
                        size,
                        badge_right,
                        cap_y,
                        1.0,
                        0.0,
                        caption_fg,
                    )?;
                    let w = m.width.as_f32();
                    // Drawn at `cap_y` in both modes: the badge is the tallest run in the caption,
                    // so its own top edge is the one that should line up with the coin's.
                    column(lay.split, &mut coin_box, &mut core_box).add(
                        badge_right - w,
                        w,
                        cap_y,
                        m.line_height.as_f32(),
                    );
                }
                // Publish the finished plate rectangles in DEVICE pixels. `render_state` draws them
                // verbatim so measured text and its plate share this single geometry source.
                let plates = [coin_box.plate(sf), core_box.plate(sf)];
                if self.panes[idx].caption_plates != plates {
                    self.panes[idx].caption_plates = plates;
                    readout_metrics_changed = true;
                }
            } else if self.panes[idx].caption_plates != [[0.0; 4]; 2] {
                // No room this frame: retire any plate left over from a wider layout.
                self.panes[idx].caption_plates = [[0.0; 4]; 2];
                readout_metrics_changed = true;
            }

            // Axes, cursor, and grid below apply only to a normal, non-collapsed chart.
            if plot_w < 60.0 || plot_h < 60.0 || view.price_to_px <= 0.0 {
                // The compare-mode ghost remains visible on a collapsed book-only broom chart:
                // derive volume/percentage from the book view, while the backend cursor layer
                // draws the line.
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

            let volume_max = volume_stats.max();
            let volume_mid = volume_max * 0.5;
            let volume_alpha = view.volume_alpha.clamp(0.0, 1.0);
            if volume_max > 1e-6 && volume_alpha > 0.01 {
                let volume_h = plot_h * view.volume_height_frac.clamp(0.02, 0.45);
                let axis_x = (plot_right - 64.0).max(plot_left + 86.0);
                let size = (self.label_font_px() - 2.0).clamp(7.0, 16.0);
                let max_y = plot_bottom - volume_h;
                if max_y >= plot_top + size {
                    let text = format!("max {}", fmt_amount(volume_max));
                    self.draw_sized_text(ctx, &text, size, axis_x - 10.0, max_y, 1.0, 0.5, ink)?;
                }
                if volume_mid > 1e-6 {
                    let mid_y = plot_bottom - volume_h * 0.5;
                    if mid_y >= plot_top + size && mid_y <= plot_bottom - size * 0.25 {
                        let text = format!("avg {}", fmt_amount(volume_mid));
                        self.draw_sized_text(
                            ctx,
                            &text,
                            size,
                            axis_x - 10.0,
                            mid_y,
                            1.0,
                            0.5,
                            ink,
                        )?;
                    }
                }
            }

            // Order-line and cursor labels align their right edges to the order book's left edge,
            // or to the separate zone on the right. With the book enabled, the plot ends at the
            // book, so its right edge equals the book's left edge. Without the book, use the
            // control zone's left edge.
            let zone_left = if orderbook_enabled {
                plot_right
            } else {
                let zone_w = moon_chart::GLASS_ZONE_PX.min((pane_right - pane_left) * 0.5);
                pane_right - zone_w
            };
            let label_x = zone_left - READOUT_PAD_X;

            // Order-line labels form a separate column left of the separator and align their
            // right edge to it. Draw ALL labels even when they overlap, in ascending priority,
            // so the higher-priority one (SELL/STOP > BUY) is drawn LAST. Its text and semi-opaque
            // badge cover the lower-priority label, which remains ~15% visible underneath rather
            // than disappearing. Offset labels by LABEL_LINE_GAP so badges do not cover the order
            // line. Draw `force` labels (drag/hover) last, above everything. A per-tab "line labels"
            // checkbox in the settings popup disables the entire column.
            // Label row height follows the font size configured by the theme slider. Shared by
            // the order-label column and the figure readouts below it.
            let label_line_h = self.label_font_px() + 4.0;
            if self.line_labels {
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

            // Figure readouts: a price at the right edge for a full-width line, the move a trend
            // line describes at the end it points to, and a ratio scale's level beside each of its
            // lines. For every tool but the scale the list is empty unless the pointer is on a
            // figure or one is being drawn — a merely selected figure has none — so an idle chart
            // with no scale on it does no work here.
            // Room a ratio scale's readouts need, taken as the WIDEST of them: the side they sit
            // on has to be decided for the whole column at once. Per label, the wide levels would
            // flip while the narrow ones stayed, tearing the column the placement exists to make.
            // Rough width only — it over-estimates, so the decision errs toward keeping the text
            // inside the plot, and no text is shaped on this path.
            let span_label_room = self.panes[idx]
                .figure_labels
                .iter()
                .filter(|l| matches!(l.place, FigLabelPlace::LineSpan { .. }))
                .map(|l| match &l.text {
                    FigLabelValue::Ready(s) => rough_label_width(s, self.label_font_delta),
                    _ => 0.0,
                })
                .fold(0.0f32, f32::max);
            for li in 0..self.panes[idx].figure_labels.len() {
                // Cloned out: a level's text is an `Arc<str>`, so this is a refcount bump, and
                // holding a borrow of the pane would block measuring through `&mut self` below.
                let label = self.panes[idx].figure_labels[li].clone();
                // The per-tab "line labels" switch hides the readouts a figure draws AT REST,
                // whichever placement they use: a ratio scale's, whether it spans a box like ours
                // or the whole chart like Moonbot's. It used to be applied inside the `LineSpan`
                // arm alone, which let a scale placing its readouts at the right edge draw straight
                // past a switch the user had turned off.
                //
                // A readout that appears only under the POINTER is not one of those and stays.
                // `permanent` cannot tell them apart — it means "not the draft" — so the VALUE does:
                // a hover readout is a typed number the tool leaves to this layer to format, a
                // `Price` or a `PctDelta`, while a scale's level arrives already formatted as
                // `Ready`. Naming the hover kinds one by one was tried and was wrong the moment a
                // second one existed; this asks the positive question instead.
                if label.permanent
                    && !self.line_labels
                    && matches!(label.text, FigLabelValue::Ready(_))
                {
                    continue;
                }
                let y = line_y(label.price);
                if y < plot_top - label_line_h || y > plot_bottom + label_line_h {
                    continue;
                }
                // Cull on POSITION before formatting: a scrolled-off figure must not pay for a
                // string it will never draw.
                let x_of = |t_rel: f32| plot_left + (t_rel - view.view_time0) * time_to_px;
                let node_x = match label.place {
                    FigLabelPlace::RightEdge => label_x,
                    FigLabelPlace::Above => {
                        let x = x_of(label.t_rel);
                        if x < plot_left || x > plot_right {
                            continue;
                        }
                        x
                    }
                    // The label rides the line's LEFT end, clipped INTO the plot: a scale must
                    // stay readable while any part of its lines is on screen. A scale's levels are
                    // the one readout that stays after the pointer leaves, so the per-tab "line
                    // A ratio scale's column of levels, placed at the box's anchor rather than
                    // under the pointer: with the left end as the anchor, a box drawn rightward
                    // keeps its column still while the prices in it change. The "line labels"
                    // switch is applied above, for every placement rather than only for this one.
                    FigLabelPlace::LineSpan { t0_ms, t1_ms } => {
                        let (x0, x1) = (
                            x_of((t0_ms - epoch_ms) as f32),
                            x_of((t1_ms - epoch_ms) as f32),
                        );
                        if x1 < plot_left || x0 > plot_right {
                            continue;
                        }
                        x0.max(plot_left)
                    }
                };
                // A ratio level's text was rendered once at the geometry rebuild — its format is
                // pure and deliberately unlike the axis. A price and a percentage are formatted
                // HERE, where the axis's own precision lives.
                let text: std::borrow::Cow<'_, str> = match &label.text {
                    FigLabelValue::Ready(s) => std::borrow::Cow::Borrowed(&**s),
                    FigLabelValue::Price(p) => std::borrow::Cow::Owned(format!("{p:.dec$}")),
                    FigLabelValue::PctDelta { from, to } => {
                        if *from == 0.0 {
                            continue;
                        }
                        std::borrow::Cow::Owned(fmt_pct(((to / from - 1.0) * 100.0) as f32))
                    }
                };
                let (x, ax, dy, ay) = match label.place {
                    // Already in the label column, which sits outside the plot when the order book
                    // is off — never gate it on the plot's own right edge.
                    FigLabelPlace::RightEdge => (label_x, 1.0, y - LABEL_LINE_GAP, 1.0),
                    FigLabelPlace::Above => {
                        // Anchored at the node, but flipped to the LEFT of it when the text would
                        // otherwise run past the plot into the order-book zone. The real width is
                        // measured ONLY near the edge, where the answer can differ — text shaping
                        // is the expensive call on this path and every label would pay for it.
                        let ax = if node_x + rough_label_width(&text, self.label_font_delta)
                            > plot_right - READOUT_PAD_X
                            && node_x + self.measure_label_text(ctx, &text).width.as_f32()
                                > plot_right
                        {
                            1.0
                        } else {
                            0.0
                        };
                        (node_x, ax, y - LABEL_LINE_GAP, 1.0)
                    }
                    // LEFT-anchored at the line's start, sitting just above the line it names: the
                    // column of numbers sits where a row is read FROM, which is where every
                    // charting package puts a ratio scale's.
                    //
                    // Flipped to the other side of the anchor when the column would otherwise run
                    // past the plot — a scale drawn against the right edge would push ELEVEN
                    // readouts over the order book at once, and figure text is not clipped there.
                    // Both room tests use the same `span_label_room`, so the whole column flips or
                    // none of it does; a column with no room on EITHER side stays on the left,
                    // where it overlaps the plot rather than the price axis beside it.
                    FigLabelPlace::LineSpan { .. } => {
                        let fits_right = node_x + READOUT_PAD_X + span_label_room <= plot_right;
                        let fits_left = node_x - READOUT_PAD_X - span_label_room >= plot_left;
                        if fits_right || !fits_left {
                            (node_x + READOUT_PAD_X, 0.0, y - LABEL_LINE_GAP, 1.0)
                        } else {
                            (node_x - READOUT_PAD_X, 1.0, y - LABEL_LINE_GAP, 1.0)
                        }
                    }
                };
                let m = draw_label_text_run(
                    &mut self.text_runs,
                    &mut self.text_run_cursor,
                    ctx,
                    self.label_font_delta,
                    &text,
                    x,
                    dy,
                    ax,
                    ay,
                    color(label.color),
                )?;
                placed.push(PlacedLabel {
                    x,
                    y: dy,
                    ax,
                    ay,
                    w: m.width.as_f32(),
                    h: m.line_height.as_f32(),
                    solid: false,
                });
            }

            // Moonbot `LastSellOrderPriceVol`: a separate order-book depth label at the sell line.
            // This is NOT order text, but cumulative book notional up to the close price: asks
            // below sell for a long, bids above sell for a short. Draw it in the order-book zone;
            // the cursor readout below covers it when the user points at the same location.
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

            // A per-tab "crosshair label" checkbox in the settings popup disables cursor readout.
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
                    // For a day other than today, show "DD.MM HH:MM:SS" for large timeframes/windows.
                    let label = crate::chartdx::axes::format_clock_dated(unix, true, now_ms);
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
                    // Right places the badge at the panel's right edge beyond the book; Left uses the left gutter.
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

                // Crosshair labels: order size ($) sits LEFT of the separator on the chart side,
                // right-aligned to the separator at the cursor line. Order-book volume and percent
                // sit RIGHT of the separator in the book zone: volume ABOVE the line and percent
                // BELOW it. All three share a color: green below current price, red above it.
                if cy_log >= plot_top && cy_log <= plot_bottom {
                    let cursor_price = y_min + (plot_bottom - cy_log) / price_to_px.max(1e-6);
                    // Percent and cursor color use the NEAREST side of the book, not last price, as
                    // in Moonbot: best bid when the cursor is below price, best ask when above it.
                    // Distance is measured from the execution price on the matching side, so long
                    // and short references differ because the spread shifts the percentage. Fall
                    // back to last price when the book or its price is unavailable.
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
                    // Leave a gap so the label badge does not cut through the crosshair line.
                    let gap = cursor_label_gap(self.cursor_thickness, sf);
                    // Cursor values are foreground priority elements outside the label columns:
                    // they occupy fixed positions at the crosshair and receive an opaque backdrop.
                    // Place order size ABOVE the cursor line, left of and right-aligned to the
                    // separator. Omit $/K-M suffixes and always show two decimals, such as "100.00".
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
                    // Draw order-book volume at the cursor level right of the separator, above the line.
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
                    // Draw the cursor's percentage deviation from the nearest book side right of
                    // the separator, below the line.
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
                // With no real cursor on the pane, draw the compare-mode ghost (volume/percentage
                // at the neighboring price). The helper suppresses it when a real cursor exists.
                self.draw_ghost_cursor_labels(ctx, idx, sf, &mut placed)?;
            }

            // sync_readout_params builds backdrop badges from the frame's completed label layout.
            // Compare layouts because zoom changes Y even when text and width stay unchanged;
            // otherwise backdrops remain at the old price and appear to float away from labels.
            if previous_placed != placed {
                self.panes[idx].label_placed = placed;
                readout_metrics_changed = true;
            } else {
                self.panes[idx].label_placed = previous_placed;
            }

            // Price labels use fixed height fractions matching the STATIC horizontal grid lines
            // (Moonbot model: the grid stays fixed while labels move). Display the exact non-round
            // price at each line. Time labels follow a different model: round local-time boundaries
            // positioned from time coordinates, independently of the fixed vertical grid lines.
            // Label internal horizontal lines only, omitting plot-frame edges and any label
            // overlapped by the cursor readout.
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

            // Place time labels at ROUND local-time boundaries (`nice_time_step`, from 1 s to 6 h
            // for roughly six labels). Fixed window fractions previously produced non-round times
            // with uneven steps, such as 19:46, 19:56, 20:05 (+10, then +9).
            if !time_axis_visible {
                continue;
            }
            let step_ms =
                (moon_chart::axes::nice_time_step(window_ms / 1000.0, 6.0) * 1000.0).max(1000.0);
            let with_sec = step_ms < 60_000.0;
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0.0, |d| d.as_millis() as f64);
            let right_unix = left_unix + window_ms;
            // Thin labels horizontally in narrow windows: draw only when a label's left edge is
            // separated from the RIGHT edge of the previously drawn label; otherwise skip it.
            let min_h_gap = 6.0;
            let mut last_right = f32::NEG_INFINITY;
            for unix in crate::chartdx::axes::aligned_ticks_ms(left_unix, right_unix + 0.5, step_ms)
            {
                // The rightmost ~10% is future space beyond the live edge; do not label times that
                // have not occurred, which would be confusing near the order-book boundary.
                if now_ms > 0.0 && unix > now_ms {
                    break;
                }
                let x = plot_left + ((unix - left_unix) / window_ms) as f32 * plot_w;
                // Include a "DD.MM" date on axis labels outside the current day; without it,
                // labels in wide windows with steps over one day appeared to run backward.
                let label = crate::chartdx::axes::format_clock_dated(unix, with_sec, now_ms);
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
