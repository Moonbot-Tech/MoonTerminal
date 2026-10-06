//! Editing and query operations on the label configuration.

use super::*;

impl ChartLabelsCfg {
    /// A configuration with no rows at all.
    ///
    /// Public because "print nothing" is a legitimate choice a user can reach by removing every
    /// row, and the popup's reset needs the same value the loader produces for an empty list.
    pub fn empty() -> Self {
        Self {
            rows: std::array::from_fn(|_| ChartLabelRow::default()),
        }
    }

    /// Repair anything a hand-edited file — or the popup — could state that the layout cannot
    /// honour.
    ///
    /// Three repairs, and the last one is what keeps indices meaningful:
    ///
    /// 1. A size multiplier outside the drawable range is clamped into it.
    /// 2. A name longer than [`LABEL_ROW_NAME_MAX`] is cut, on a character boundary.
    /// 3. Holes are closed — captions inside a row, and blank rows in the list — so that "the
    ///    leading N are the used ones" holds everywhere, which is what the popup, the draw order
    ///    and the run pool all read.
    ///
    /// `layout.toml` and `charts.json` are both hand-editable and this configuration is
    /// materialized into specs by ⧉, so an unrepaired value would outlive the file it came from.
    pub fn sanitize(&mut self) {
        for row in &mut self.rows {
            // Trimmed before anything reads it, so "is this row named?" has ONE answer: the
            // popup's list, `is_blank` and the caption that prints the name all ask it separately.
            //
            // Trim, CUT, trim again — in that order and not the other one. Cutting a trimmed name
            // can land the boundary on a space, and a repair that leaves work for its own next run
            // is a value that never equals itself: the panel's settings signature would then report
            // a change on every notification, which is exactly what the `nan` guard below prevents.
            row.gap = row.gap.min(LABEL_GAP_MAX);
            let repaired = {
                let cut: String = row.name.trim().chars().take(LABEL_ROW_NAME_MAX).collect();
                cut.trim_end().to_string()
            };
            if row.name != repaired {
                row.name = repaired;
            }
            compact(&mut row.parts, ChartLabelPart::is_used);
            for part in &mut row.parts {
                // A window the field cannot be read over is repaired to one it can: switching a
                // caption's field must not leave it asking for a figure that never arrives, and a
                // hand-edited file must not either.
                let choices = part.field.window_choices();
                if !choices.contains(&part.window) {
                    part.window = choices.first().copied().unwrap_or_default();
                }
                // A zero or unbounded custom span is repaired rather than dropped: the reader asked
                // for a custom period, and falling back to the window would silently answer a
                // different question than the one on screen.
                part.span.sanitize();
                // A timeframe on a caption that counts nothing down, dropped for the reason the
                // span below it is: switching a caption's field must not leave a parameter behind
                // that nothing reads and a later field change would suddenly obey.
                if !part.field.uses_tf() {
                    part.tf = LabelTf::Auto;
                }
                // A span on a caption that reads no period at all is dropped, like the window
                // repair above: switching a caption's field must not leave a period behind that
                // nothing reads but a later field would suddenly obey.
                if !part.field.uses_window() {
                    part.span = LabelSpan::Window;
                    // Same repair as the span: an anchor left on a caption that reads no period is
                    // a setting nothing honours, waiting to surprise a later field change.
                    part.anchor = SpanAnchor::Now;
                }
                // A hand-edited `nan` is dropped, not clamped: it would survive the clamp, and a
                // configuration that does not equal ITSELF turns every comparison downstream — the
                // panel's settings signature, the engine's change check — into a false change on
                // every notification.
                part.style.size_mult = part.style.size_mult.and_then(|mult| {
                    mult.is_finite()
                        .then(|| mult.clamp(LABEL_SIZE_MULT_MIN, LABEL_SIZE_MULT_MAX))
                });
            }
        }
        // Close holes between rows, keeping their order. A row that lost its last caption and was
        // never named is blank, and drops out here.
        compact(&mut self.rows, |row| !row.is_blank());
    }

    /// Index of the first blank row, or `None` when every row is taken.
    pub fn first_free_row(&self) -> Option<usize> {
        self.rows.iter().position(ChartLabelRow::is_blank)
    }

    /// How many leading rows hold something.
    pub fn used_rows(&self) -> usize {
        self.first_free_row().unwrap_or(CHART_LABEL_ROWS)
    }

    /// Append a row holding one caption, returning its index.
    ///
    /// A row is never created empty: an empty row is blank, and a blank row does not survive
    /// [`Self::sanitize`] — which every write goes through.
    pub fn push_row(
        &mut self,
        field: ChartLabelField,
        zone: LabelZone,
        align: LabelAlign,
    ) -> Option<usize> {
        let ix = self.first_free_row()?;
        let mut row = ChartLabelRow::new(zone, align);
        row.push_part(field);
        self.rows[ix] = row;
        Some(ix)
    }

    /// Append a row that is already built, returning its index, or `None` when there is no room.
    ///
    /// The door the module EDITOR comes back through when it was opened on a module that does not
    /// exist yet: a new module is only worth a slot once it holds something, and a slot taken
    /// before the editor opened would be a module the user then cancelled out of. A blank row is
    /// refused here rather than pushed and swept away by [`Self::sanitize`], so the caller can tell
    /// "no room" from "nothing to add".
    pub fn push_prepared(&mut self, row: ChartLabelRow) -> Option<usize> {
        if row.is_blank() {
            return None;
        }
        let ix = self.first_free_row()?;
        self.rows[ix] = row;
        Some(ix)
    }

    /// Append a row built from a preset: its fields, in its band, under its name.
    ///
    /// The NAME is not stored — the preset is, and the terminal looks it up in the dictionary every
    /// time it prints it. A localized string baked into a saved profile keeps speaking the language
    /// it was created in, which is what the shipped default did before this.
    pub fn push_preset(&mut self, preset: LabelPreset) -> Option<usize> {
        let ix = self.first_free_row()?;
        let mut row = ChartLabelRow::new(preset.zone(), preset.align());
        row.preset = Some(preset);
        row.flow = preset.flow();
        for field in preset.fields() {
            if !row.push_part(*field) {
                break;
            }
        }
        for part in row.parts.iter_mut().filter(|p| p.field.uses_window()) {
            if let Some(window) = preset.window() {
                part.window = window;
            }
            part.span = preset.span();
            part.anchor = preset.anchor();
        }
        self.rows[ix] = row;
        Some(ix)
    }

    /// Remove one row, closing the gap so the remaining draw order is preserved.
    pub fn remove_row(&mut self, ix: usize) {
        remove_at(&mut self.rows, ix);
    }

    /// Swap a row with its neighbour, moving it earlier (`up`) or later in the draw order.
    ///
    /// Rows in the same band stack in this order, so it is the only thing that decides which of two
    /// rows sits closer to the plot's edge. Returns whether anything moved.
    pub fn move_row(&mut self, ix: usize, up: bool) -> bool {
        let used = self.used_rows();
        move_at(&mut self.rows, used, ix, up)
    }

    /// Whether any DRAWN caption's field satisfies `pred`.
    ///
    /// The sync paths gate their work on this: collecting open-position figures walks a core's
    /// whole order array, and reading the delta snapshot takes the market-source lock. Neither is
    /// worth doing for a configuration that prints none of it — which is the default.
    pub fn any_drawn(&self, pred: impl Fn(ChartLabelField) -> bool) -> bool {
        self.drawn_parts().any(|p| pred(p.field))
    }

    /// Every caption that reaches the chart, in draw order.
    ///
    /// Stops at the first blank row rather than walking all sixteen: `sanitize` packs the used rows
    /// to the front, and the gates below run several times per market revision, per pane.
    pub fn drawn_parts(&self) -> impl Iterator<Item = &ChartLabelPart> {
        self.rows
            .iter()
            .take_while(|r| !r.is_blank())
            // A hidden ROW takes its captions with it: the gates below decide whether the sync
            // paths do work for them, and a row nobody sees must not order any.
            .filter(|r| r.visible)
            .flat_map(|r| r.parts[..r.used_parts()].iter())
            .filter(|p| p.is_drawn())
    }

    /// The wall clock the drawn countdown captions should be formatted against, QUANTIZED to the
    /// coarsest step they can live with — or `None` when none of them is drawn.
    ///
    /// The quantum is the whole cost control for these captions. A countdown that re-formatted on
    /// every market revision would reshape a pane's whole caption set several times a second on a
    /// busy coin to print the same string; quantizing makes the caption cache answer "unchanged"
    /// until the figure actually moves. A minute is enough while every countdown is far out, and
    /// only a countdown inside its last hour — which is when the caption starts printing seconds —
    /// buys the second-by-second step.
    ///
    /// The threshold carries a minute of SLACK past the hour so the step changes just BEFORE the
    /// display needs it. Without it the switch waits for the next minute tick, and the caption
    /// spends up to a minute printing an hour figure while the seconds it should be showing run.
    ///
    /// Args:
    ///     chart_tf_ms: The chart's own candle timeframe, which an `Авто` caption resolves to.
    ///     now_ms: Unix milliseconds.
    ///
    /// Returns:
    ///     The quantized clock, or `None` when no caption counts anything down.
    ///
    /// An `Option` rather than a zero: zero is a legal clock — a machine whose system time cannot
    /// be read reports exactly that — and a caller comparing against a sentinel would then take
    /// "the epoch" for "nothing to do" and freeze the countdown for good.
    pub fn countdown_clock_ms(&self, chart_tf_ms: i64, now_ms: i64) -> Option<i64> {
        let mut quantum: Option<i64> = None;
        for part in self.drawn_parts() {
            let step = match part.field {
                ChartLabelField::FundingIn => 60_000,
                // The ban readout prints what is left, to the MINUTE — see the terminal's
                // `fmt_ban_left`. It asks for the clock whether or not a ban is running: the
                // configuration is what this reads, and a caption that only started ticking once a
                // ban began would print the minute it was set at until the next revision arrived.
                ChartLabelField::TempBanLeft => 60_000,
                ChartLabelField::TfCloseIn => {
                    match part.tf.remaining_ms(chart_tf_ms, now_ms) < COUNTDOWN_SECOND_STEP_BELOW_MS
                    {
                        true => 1_000,
                        false => 60_000,
                    }
                }
                _ => continue,
            };
            // The FINEST step any of them asks for: a clock coarser than one caption needs would
            // freeze that caption, while a finer one merely re-formats the others for nothing.
            quantum = Some(quantum.map_or(step, |held: i64| held.min(step)));
        }
        quantum.map(|q| now_ms.div_euclid(q) * q)
    }

    /// Every distinct PERIOD the drawn volume captions ask for, in first-seen order.
    ///
    /// The sync path turns each of these into one history read, so the deduplication is the point:
    /// a module printing the buying, the selling and their total over one minute is one read, not
    /// three. Returned as the configuration's own pair rather than the market layer's span so this
    /// crate's model stays independent of what reads it.
    pub fn volume_spans(&self) -> Vec<VolumeSpanKey> {
        let mut out: Vec<VolumeSpanKey> = Vec::new();
        for part in self.drawn_parts().filter(|p| p.field.reads_volume()) {
            // A trade-count span ignores the window, so two captions asking for the same count must
            // not read twice just because their unused windows differ.
            let (span, window) = match part.span {
                LabelSpan::Window => (LabelSpan::Window, part.window),
                other => (other, LabelWindow::default()),
            };
            let key = VolumeSpanKey {
                span,
                window,
                anchor: part.anchor,
                // Liquidations come off their own ring, so a period that nothing prints `L` over
                // must not pay for reading it.
                liquidations: part.field == ChartLabelField::WindowLiquidations,
            };
            match out.iter_mut().find(|held| held.same_period(&key)) {
                // One read serves both figures over one period: the flags are unioned rather than
                // the period being listed twice.
                Some(held) => held.liquidations |= key.liquidations,
                None => out.push(key),
            }
        }
        out
    }

    /// Whether anything drawn measures around the POINTER rather than at the live edge.
    ///
    /// The gate for the extra work a measuring caption costs: its figures move with the mouse, not
    /// with the market, so they are refreshed on a path the ordinary captions never touch.
    pub fn any_cursor_anchored(&self) -> bool {
        self.drawn_parts()
            .any(|p| p.anchor == SpanAnchor::Cursor && p.field.reads_volume())
    }

    /// Whether any part uses this field, for the "already added" mark in the add menu.
    pub fn contains(&self, field: ChartLabelField) -> bool {
        self.rows
            .iter()
            .take_while(|r| !r.is_blank())
            .any(|r| r.parts[..r.used_parts()].iter().any(|p| p.field == field))
    }
}

/// One PERIOD a configuration asks the retained history for, and what it wants out of it.
///
/// The sync path turns each of these into at most one read of the trades and one of the
/// liquidations, so it is deduplicated by the period itself — the two figures over one minute are
/// one entry with both flags, not two entries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VolumeSpanKey {
    pub span: LabelSpan,
    /// The window the span defers to; meaningless unless `span` is [`LabelSpan::Window`].
    pub window: LabelWindow,
    /// Where the period sits: at the live edge, or around the pointer.
    pub anchor: SpanAnchor,
    /// Whether anything printed over this period reads the LIQUIDATION ring.
    pub liquidations: bool,
}

impl VolumeSpanKey {
    /// Whether two keys describe the same stretch of time, ignoring which figures want it.
    fn same_period(&self, other: &Self) -> bool {
        self.span == other.span && self.window == other.window && self.anchor == other.anchor
    }
}
