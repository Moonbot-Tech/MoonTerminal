use super::model::*;
use super::*;

use super::fit::{fit_caption, grow_column_band};

impl RenderState {
    /// Draw rows stacked in one column, downward for a top zone and upward for a bottom one.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn draw_stack(
        &mut self,
        ctx: &mut gpui::GpuCanvasTextContext<'_>,
        idx: usize,
        texts: &[LabelText],
        rows: &mut [Row],
        column: Column,
        start_y: f32,
        limit_y: f32,
        downward: bool,
        caption_fg: Hsla,
        hits: &mut Vec<ArbHit>,
        bars: &mut Vec<CaptionBar>,
        vol_hits: &mut Vec<(usize, CaptionBox)>,
        act_draws: &mut Vec<ActionDraw>,
        hungry: Option<HungryBudget>,
    ) -> anyhow::Result<(Vec<(usize, CaptionBox)>, f32, f32)> {
        let mut plates: Vec<(usize, CaptionBox)> = Vec::new();
        // The band is as wide as its widest LINE — what the bands drawn after it have to clear.
        let mut used_w = 0.0_f32;
        let mut y = start_y;
        // Whether anything has actually been PUT on the pane. Not the loop index: the first line
        // of a band is exempt from the clamp below, and "first" means first DRAWN.
        let mut any_drawn = false;
        for row in rows.iter_mut() {
            // The module's own spacing, in the direction the band runs: below the previous line in
            // a band that fills downward, above it in one that fills upward. On the FIRST line it
            // is the indent from the band's own edge, which is the case an "after this module"
            // reading cannot express at all.
            let gap = row.gap;
            let mut travelled = if downward {
                (y - start_y).abs() + gap
            } else {
                (start_y - y).abs() + gap
            };
            if let Some(budget) = hungry {
                // A wrapping skip list that would open beside the core name is moved past it.
                // Squeezing the first sentence into the leftover next to a one-line caption wraps
                // it in half and still reads as printing through the name.
                if row.cells.iter().any(Cell::has_wrap) {
                    let clear_at = budget.taken.blocking_height(budget.align) + CAPTION_GAP;
                    if clear_at > travelled {
                        let extra = clear_at - travelled;
                        y += if downward { extra } else { -extra };
                        travelled = clear_at;
                    }
                }
                self.wrap_hungry_row(ctx, texts, row, budget, travelled, downward);
            }
            let available_h = if downward {
                limit_y - y - gap
            } else {
                y - gap - limit_y
            };
            loop {
                let mut trimmed = false;
                for cell in &mut row.cells {
                    trimmed |= cell.fit_filter_header(texts, available_h);
                }
                let Some(budget) = hungry.filter(|_| trimmed) else {
                    break;
                };
                // Removing bottom-anchored entries changes the retained lines' depths beside
                // neighbouring bands. Rewrap at those positions, then recheck the height. Each
                // repeat removes an entry, so the bounded column guarantees termination.
                self.wrap_hungry_row(ctx, texts, row, budget, travelled, downward);
            }
            let row_h = row.height();
            // The band stacks until it runs out of pane. Lines past that are dropped rather than
            // drawn: a caption over the time axis — or outside the pane entirely — reads as a
            // glitch, and the horizontal budget already drops what does not fit the same way.
            //
            // The FIRST line of a band is exempt. A pane can be shorter than one line of text — a
            // broom follower, a compressed stack slot — and the coin caption drew there before this
            // clamp existed; suppressing a band entirely because it is cramped would take the
            // chart's only identification with it.
            let overflows = if downward {
                y + gap + row_h > limit_y
            } else {
                y - gap - row_h < limit_y
            };
            if overflows && any_drawn {
                break;
            }
            y += if downward { gap } else { -gap };
            // Runs are drawn top-anchored, so an upward stack subtracts the row's height BEFORE
            // drawing it. Taking that height from the styles rather than from a measurement is what
            // makes this possible without drawing the row twice.
            let top = if downward { y } else { y - row_h };
            // A hungry row may run wider than the band's capped budget: lines that have cleared
            // the neighbours wrap at the leftover of the zone, and the cell must be measured at
            // that leftover or the already-wrapped lines are treated as overflow.
            let mut col = column;
            if let Some(budget) = hungry {
                col.max_w = widths::line_budget(
                    budget.total,
                    budget.align,
                    budget.taken,
                    travelled + row_h,
                    budget.inset,
                );
            }
            // One box PER MODULE on this line: two modules can share a line — that is what the
            // placement axis is for — and one rectangle behind both would put a backing under the
            // module that switched it off.
            let row_w = self.draw_row(
                ctx,
                idx,
                texts,
                &row.cells,
                col,
                top,
                limit_y,
                downward,
                caption_fg,
                &mut plates,
                hits,
                bars,
                vol_hits,
                act_draws,
            )?;
            used_w = used_w.max(row_w);
            any_drawn = true;
            y += if downward { row_h } else { -row_h };
        }
        let used_h = if downward {
            (y - start_y).max(0.0)
        } else {
            (start_y - y).max(0.0)
        };
        Ok((plates, used_w, used_h))
    }

    /// Draw one row of captions, returning the width it actually took.
    ///
    /// That width is what the bands drawn after this one are placed against — see [`widths`] — so
    /// it is the DRAWN width, not the measured one: a caption shaped as a prefix run plus a value
    /// run can come out a little wider than it measured, and a neighbour placed from the
    /// measurement would be the one to pay for it.
    ///
    /// Only captions that ask for a plate grow `plate`: the backing is a per-caption setting, and a
    /// box grown by every drawn run would put a plate under a caption that switched it off.
    #[allow(clippy::too_many_arguments)]
    fn draw_row(
        &mut self,
        ctx: &mut gpui::GpuCanvasTextContext<'_>,
        idx: usize,
        texts: &[LabelText],
        cells: &[Cell],
        column: Column,
        top: f32,
        // Edge of the band this line lives in, and which way the band fills. A CELL can be taller
        // than the pane on its own — an arbitrage column is one line per venue — and the caller's
        // per-line guard exempts the first line of a band, so the stack is clipped here too.
        limit_y: f32,
        downward: bool,
        caption_fg: Hsla,
        plates: &mut Vec<(usize, CaptionBox)>,
        hits: &mut Vec<ArbHit>,
        bars: &mut Vec<CaptionBar>,
        vol_hits: &mut Vec<(usize, CaptionBox)>,
        act_draws: &mut Vec<ActionDraw>,
    ) -> anyhow::Result<f32> {
        if cells.is_empty() {
            return Ok(0.0);
        }
        // A centred line must know its own width before it can be placed; the other two directions
        // walk out from their edge and never need the total.
        let start_x = if column.align > 0.0 && column.align < 1.0 {
            column.x - self.measure_row(ctx, texts, cells, column.max_w) * 0.5
        } else {
            column.x
        };
        // Right-anchored lines draw at their right edge and walk LEFT; the other two draw at their
        // left edge and walk right. One `ax` per direction, the same arithmetic mirrored.
        let rightwards = column.align < 1.0;
        let ax = if rightwards { 0.0 } else { 1.0 };
        let mut cursor = start_x;
        let mut budget = column.max_w;
        for (n, cell) in cells.iter().enumerate() {
            // The base spacing keeps two columns from touching; the module's own gap is ADDED to
            // it, so `0` means "as before" and any value means "and this much more".
            //
            // The FIRST column takes none of it. Its module is the one that OPENED this line, and
            // that gap has already been spent on the space above the line — spending it twice
            // would move the line diagonally instead of spacing it.
            let gap = if n == 0 { 0.0 } else { CAPTION_GAP + cell.gap };
            budget -= gap;
            // No room left on this line. Dropping the rest is deliberate: a caption clipped
            // mid-number reads as a plausible WRONG number.
            if budget < MIN_LEGIBLE_W {
                break;
            }
            // The column is as wide as its widest caption, and every caption in it is anchored to
            // the same edge — which is what makes a block read as a block. A CENTRED band is the
            // exception: there each caption is centred inside that width.
            let cell_w = self.measure_cell(ctx, texts, cell, budget);
            // The bar column, decided ONCE for the module: every track in it starts on the same
            // vertical, which is what lets two of them be compared at a glance. Placed after the
            // figures in reading order either way — a track before the number it belongs to reads
            // as belonging to the line above.
            let reserve = bar_zone(texts, cell);
            let text_w = (cell_w - reserve).max(0.0);
            let anchor_x = if rightwards {
                cursor + gap
            } else {
                cursor - gap
            };
            // Where every bar of this module starts, and where its text ends.
            //
            // Filling rightwards the text runs from the anchor and the track follows it; filling
            // leftwards the module is pinned by its RIGHT edge, so the track takes that edge and
            // the text ends before it. Both put the figure first and the track second.
            let (bar_x, text_anchor_x) = match (reserve > 0.0, rightwards) {
                (false, _) => (0.0, anchor_x),
                (true, true) => (anchor_x + text_w + CAPTION_GAP, anchor_x),
                // Pinned by its RIGHT edge: the track takes that edge, and the text column starts a
                // whole track and gap before it.
                (true, false) => (anchor_x - BAR_W, anchor_x - reserve - text_w),
            };
            // Inside a module with bars the figures are LEFT-aligned against each other, whatever
            // edge the module itself is pinned to. Right-aligning them would line up their last
            // digits and leave `Bv` and `Sv` starting in different places — the block reads as a
            // pair of labelled figures, and a label that moves is not one.
            let cell_ax = match reserve > 0.0 {
                true => 0.0,
                false => ax,
            };
            // A CENTRED band centres its captions against each other too, not just the line as a
            // whole: a module whose captions stack — a detect line under its strategy — reads as a
            // ragged left edge otherwise, which is the one thing centring was asked for.
            let centred = column.align > 0.0 && column.align < 1.0;
            let mut y = top;
            let mut drawn_w = 0.0_f32;
            for (n_item, item) in cell.items.iter().enumerate() {
                // Out of pane. Which END of the stack is lost depends on which way the band fills,
                // and getting that backwards is how an over-tall arbitrage column printed one venue
                // outside the plot and dropped the two dozen that would have fitted:
                //
                // - a band filling DOWNWARD keeps its top lines and drops the tail;
                // - a band filling UPWARD is anchored at its BOTTOM edge, so the lines that fall
                //   off are the ones at the top — skipped, not a reason to stop.
                //
                // One line always survives, mirroring the first LINE of a band above: a pane can be
                // shorter than one line of text, and a stack that printed nothing there would take
                // the chart's only identification with it.
                let past_edge = if downward {
                    y + item.line_h() > limit_y
                } else {
                    y < limit_y
                };
                let last = n_item + 1 == cell.items.len();
                if past_edge {
                    if downward {
                        if n_item > 0 {
                            break;
                        }
                    } else if !last {
                        // Its whole block, not one line: an upward band reserved `block_h` for it,
                        // and stepping over a wrapped caption by a single line would put every
                        // caption below it two lines out of place.
                        y += item.block_h();
                        continue;
                    }
                }
                let Some(entry) = texts.get(item.pos) else {
                    continue;
                };
                // A line's own colour wins over the caption's style: an arbitrage row is coloured
                // by its VENUE, which one style cannot say for a dozen lines.
                let value_color = match entry.color {
                    Some(rgb) => gpui::rgb(rgb).into(),
                    None => self.caption_color(item.style.color, entry.sign, caption_fg),
                };
                // A prefix that takes the same colour as its value is not a second run: gluing them
                // makes ONE string, which shapes once and cannot drift apart. Only "colour the
                // value alone" needs the split, and then the prefix keeps the theme's colour.
                // The prefix is measured FIRST, through its OWN retained run: it is drawn with
                // the value and spends the same budget, and measuring it through a throwaway run
                // would re-shape it on every measure and every frame — the cost `caption_runs`
                // exists to avoid.
                let prefix_w =
                    match !item.wraps && item.style.value_only && !entry.prefix.is_empty() {
                        true => self.measure_caption_run(
                            ctx,
                            idx,
                            item.row,
                            PREFIX_PART_BASE + item.part,
                            &entry.prefix,
                            item.size,
                        ),
                        false => 0.0,
                    };
                // Too little room for the pair: the caption falls back to ONE run holding both,
                // truncated as a whole. A split that kept the full-width prefix would paint it past
                // the band's edge, and truncating the prefix instead would leave a caption naming
                // nothing.
                let split = !item.wraps && prefix_w > 0.0 && budget - prefix_w >= MIN_LEGIBLE_W;
                let prefix_w = if split { prefix_w } else { 0.0 };
                let fit_prefix = if split { "" } else { &entry.prefix };
                // Prose is broken across lines instead of being cut, and every line but the
                // first draws through a run slot of its own: a retained run is addressed by its
                // part, so a second line drawn through the first line's run would replace it.
                // Taken out for the duration of the draw and put back after, like the texts and
                // the hit rectangles above: the runs live on `self`, so a borrow cannot span the
                // draw — and cloning a sentence per caption per frame is what the cache exists to
                // avoid.
                if item.wrap_ix < self.caption_wraps.len() {
                    let lines = std::mem::take(&mut self.caption_wraps[item.wrap_ix]);
                    for (k, (line, line_w)) in lines.iter().enumerate() {
                        // Re-asked for every line, at the y that line will be drawn at: the guard
                        // above was answered for the caption's FIRST line, and a block that passed
                        // it there would otherwise paint its tail over the time axis.
                        //
                        // Which lines are lost depends on the direction, exactly as it does for
                        // whole captions: a band filling DOWNWARD keeps its head and drops the
                        // tail, while one filling UPWARD is anchored at its bottom, so the lines
                        // that fall off are at the TOP and the ones after them come back on-pane.
                        if k > 0 {
                            match downward {
                                true if y + item.line_h() > limit_y => break,
                                false if y < limit_y => {
                                    y += item.line_h();
                                    continue;
                                }
                                _ => {}
                            }
                        }
                        let line_x = match centred {
                            true => anchor_x + (cell_w - line_w).max(0.0) * 0.5,
                            false => anchor_x,
                        };
                        let part = match k {
                            0 => item.part,
                            k => WRAP_PART_BASE + k - 1,
                        };
                        let metrics = self.draw_caption_run(
                            ctx,
                            idx,
                            item.row,
                            part,
                            line,
                            item.size,
                            line_x,
                            y,
                            ax,
                            value_color,
                        )?;
                        crate::diag::bump(&crate::diag::CHART_CAPTION_DRAW);
                        let w = metrics.width.as_f32();
                        drawn_w = drawn_w.max(w);
                        let box_left = if rightwards { line_x } else { line_x - w };
                        grow_column_band(
                            &mut self.panes[idx].column_bands,
                            item,
                            [box_left, y, w, metrics.line_height.as_f32()],
                        );
                        if item.plate {
                            match plates.iter_mut().find(|(row, _)| *row == item.row) {
                                Some((_, box_)) => {
                                    box_.add(box_left, w, y, metrics.line_height.as_f32())
                                }
                                None => {
                                    let mut box_ = CaptionBox::default();
                                    box_.add(box_left, w, y, metrics.line_height.as_f32());
                                    plates.push((item.row, box_));
                                }
                            }
                        }
                        y += item.line_h();
                    }
                    self.caption_wraps[item.wrap_ix] = lines;
                    continue;
                }
                // The bar's track is part of what this caption occupies, so its room is taken out
                // of the budget BEFORE the text is fitted to what is left. Added afterwards, the
                // track would be drawn past the width the band actually allotted — over whatever
                // the layout put beside it — and the plate grown from the same width with it.
                // A BUTTON is as wide as its backing, not as its label: the plate is what a reader
                // aims at and what the module beside it has to clear. Reserved here, out of the
                // same budget the bar track comes from, so a cramped band truncates the label
                // rather than drawing a plate over its neighbour.
                let act_w = match entry.action.and_then(LabelAction::market) {
                    Some(_) => ACTION_PLATE_W,
                    None => 0.0,
                };
                let (text, value_w) = fit_caption(
                    &mut self.caption_fit_memo,
                    ctx,
                    fit_prefix,
                    &entry.text,
                    item,
                    budget - prefix_w - reserve - act_w,
                );
                if text.is_empty() {
                    continue;
                }
                // Where this caption sits inside its column. Only a centred band moves it: the
                // other two anchor every caption to the same edge, which is what makes a block
                // read as a block there. The bar column is excluded from that centring — it is a
                // reserve, not text, and centring against it would push every figure off-centre by
                // half a track.
                let item_x = match (centred, reserve > 0.0) {
                    // A module with bars is a column of its own: its figures start where its text
                    // column starts, centred band or not.
                    (_, true) => text_anchor_x,
                    (true, false) => text_anchor_x + (text_w - (prefix_w + value_w)).max(0.0) * 0.5,
                    (false, false) => text_anchor_x,
                };
                if split {
                    // A right-anchored caption ends at `anchor_x`, so BOTH runs are placed from
                    // that edge backwards: the value takes the last `value_w`, and the prefix the
                    // `prefix_w` before it. Placing the prefix at the value's own left edge — one
                    // subtraction short — draws the two on top of each other.
                    let prefix_x = if cell_ax < 0.5 {
                        item_x
                    } else {
                        item_x - value_w - prefix_w
                    };
                    // An arbitrage line's prefix is the VENUE's name, and clicking it opens this
                    // coin there. The rectangle is recorded from the placement above rather than
                    // recomputed later: a click has to hit what was actually drawn.
                    if let Some((code, dex)) = entry.venue.clone() {
                        hits.push(ArbHit {
                            x: prefix_x,
                            y,
                            w: prefix_w,
                            h: item.line_h(),
                            code,
                            dex,
                            reachable: entry.reachable,
                        });
                    }
                    // A venue this terminal cannot open is drawn faded: the column still states
                    // its price — that is what the column is for — but the name is not a target,
                    // and a reader should see which ones are.
                    let prefix_color = match entry.venue.is_some() && !entry.reachable {
                        true => caption_fg.opacity(DISABLED_OPACITY),
                        false => caption_fg,
                    };
                    self.draw_caption_run(
                        ctx,
                        idx,
                        item.row,
                        PREFIX_PART_BASE + item.part,
                        &entry.prefix,
                        item.size,
                        prefix_x,
                        y,
                        // The prefix always draws LEFT-anchored from the point computed above:
                        // right-anchoring it would place its right edge where its left edge belongs.
                        0.0,
                        prefix_color,
                    )?;
                    crate::diag::bump(&crate::diag::CHART_CAPTION_DRAW);
                }
                let value_x = match (split, cell_ax < 0.5) {
                    (true, true) => item_x + prefix_w,
                    _ => item_x,
                };
                let _ = value_w;
                // A BUTTON is measured and not drawn. The chart's own pass cannot host a GPUI
                // element, so what it does here is reserve the room the caption layout gives the
                // button and publish the rectangle; the panel puts the application's own control in
                // it — see `panels::chart::market_actions`. Drawing the label here as well would
                // print it twice, in two different fonts.
                let (text_w, line_h) = match entry
                    .action
                    .and_then(LabelAction::market)
                    .map(|mark| mark.action)
                {
                    // A SQUARE button reserves its own height and measures nothing: the lock is one
                    // glyph whose whole meaning is its shape, and a box that followed the glyph's
                    // width would stop being square the moment the face or the size changed.
                    Some(action) if action.square() => (item.line_h(), item.line_h()),
                    Some(_) => (
                        self.measure_caption_run(ctx, idx, item.row, item.part, &text, item.size),
                        item.line_h(),
                    ),
                    None => {
                        let metrics = self.draw_caption_run(
                            ctx,
                            idx,
                            item.row,
                            item.part,
                            &text,
                            item.size,
                            value_x,
                            y,
                            cell_ax,
                            value_color,
                        )?;
                        crate::diag::bump(&crate::diag::CHART_CAPTION_DRAW);
                        (metrics.width.as_f32(), metrics.line_height.as_f32())
                    }
                };
                let w = text_w + prefix_w;
                // The proportion bar, placed from the SAME measurement the text was drawn at, so it
                // cannot drift from the figure it belongs to. It extends the caption's own width,
                // which is what keeps the module beside it from being drawn over the track.
                if let Some(bar) = entry.bar {
                    let bar_h = (line_h * BAR_H_RATIO).max(BAR_H_MIN);
                    let bar_y = y + (line_h - bar_h) * 0.5;
                    // The module's OWN vertical, computed once above — not this line's right edge.
                    // Following the figure is what made the tracks jump between lines: `Bv 1.36K`
                    // and `Sv 917.36` are different widths, so their bars started in different
                    // places and the pair stopped being comparable.
                    bars.push(CaptionBar {
                        dst: [bar_x, bar_y, BAR_W, bar_h],
                        fill: bar.fill.clamp(0.0, 1.0),
                        sell: bar.sell,
                    });
                }
                // What this line OCCUPIES includes the module's bar column: the plate behind it and
                // the right-click target are grown from this, and a module whose tracks fell
                // outside both would be drawn over by its neighbour.
                let occupied = w + reserve + act_w;
                drawn_w = drawn_w.max(occupied);
                let box_left = match (reserve > 0.0, rightwards) {
                    // With bars the text column starts at `item_x` either way, and the track sits
                    // after it — so the block runs from there.
                    (true, _) => item_x,
                    (false, true) => item_x,
                    (false, false) => item_x - w,
                };
                if entry.action == Some(LabelAction::ToggleStrategyFilters) {
                    self.panes[idx]
                        .filter_header_hits
                        .push(crate::chartdx::FilterHeaderHit {
                            rect: [box_left, y, w, line_h],
                            row: item.row,
                            cfg: self.chart_labels.clone(),
                        });
                }
                grow_column_band(
                    &mut self.panes[idx].column_bands,
                    item,
                    [box_left, y, w, line_h],
                );
                // The module's right-click target grows with every line of it — the heading, the
                // figures and the bars beside them — so the menu opens from anywhere on the block.
                // Independent of the plate: a module with its backing switched off is still a
                // target, and tying the two would make the menu unreachable for it.
                if entry.volume_menu {
                    match vol_hits.iter_mut().find(|(row, _)| *row == item.row) {
                        Some((_, box_)) => box_.add(box_left, w, y, line_h),
                        None => {
                            let mut box_ = CaptionBox::default();
                            box_.add(box_left, w, y, line_h);
                            vol_hits.push((item.row, box_));
                        }
                    }
                }
                // Where the button GOES — exactly the room this caption CHARGED the line for, and
                // not a box grown around the text afterwards. The advance below adds
                // `ACTION_PLATE_W` on the side the line fills towards, so the rectangle has to grow
                // the same way: a symmetric box would overhang the module before it by half the pad
                // and leave the other half of the reserve empty.
                if let Some(mark) = entry.action.and_then(LabelAction::market) {
                    let left = match rightwards {
                        true => item_x,
                        false => item_x - w - ACTION_PLATE_W,
                    };
                    act_draws.push(ActionDraw {
                        mark,
                        size: item.size,
                        rect: [
                            left,
                            y - super::super::caption::ACTION_PAD_Y,
                            w + ACTION_PLATE_W,
                            line_h + super::super::caption::ACTION_PAD_Y * 2.0,
                        ],
                        row: item.row,
                        part: item.part,
                    });
                }
                // Grown into the box of the MODULE this caption belongs to. A module whose
                // plate is switched off never opens one, so its captions grow nothing — and
                // neither does a button, which has a backing of its own and would otherwise be
                // drawn on two plates at once.
                if item.plate && entry.action.and_then(LabelAction::market).is_none() {
                    match plates.iter_mut().find(|(row, _)| *row == item.row) {
                        Some((_, box_)) => box_.add(box_left, w, y, line_h),
                        None => {
                            let mut box_ = CaptionBox::default();
                            box_.add(box_left, w, y, line_h);
                            plates.push((item.row, box_));
                        }
                    }
                }
                y += item.line_h();
            }
            // The cursor moves by whichever is wider: what the column was measured at, or what it
            // actually drew. A centred caption sits INSIDE that width and never adds to it.
            let advance = drawn_w.max(cell_w);
            cursor = if rightwards {
                anchor_x + advance
            } else {
                anchor_x - advance
            };
            budget -= advance;
        }
        // Whichever way the line walked, this is how much of the band it spent.
        Ok((cursor - start_x).abs())
    }
}
