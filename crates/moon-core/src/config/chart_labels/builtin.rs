//! The shipped label sets and their builders.

use super::*;

/// The instrument block both shipped sets open with: the coin, optionally its core, the venue.
///
/// One builder rather than two copies, so the band, the alignment and the stacking cannot drift
/// apart between the live default and the trade window's — which is exactly what a reader compares
/// when they look at the two charts side by side.
///
/// Args:
///     with_core: Whether to name the core between the coin and the venue.
///
/// Returns:
///     The row, ready to place.
fn instrument_row(with_core: bool) -> ChartLabelRow {
    let mut row = ChartLabelRow::new(LabelZone::ZoneTop, LabelAlign::Right);
    row.preset = Some(LabelPreset::Instrument);
    row.flow = LabelFlow::Column;
    row.push_part(ChartLabelField::Coin);
    if with_core {
        row.push_part(ChartLabelField::Core);
    }
    row.push_part(ChartLabelField::Venue);
    row
}

/// The shipped strategy-filter column: ChartTop, left, stacked, spaced off the module above it.
///
/// One builder so the live default and the one-shot insert into existing profiles cannot drift:
/// a migrated chart and a fresh one must place the same module in the same band.
pub fn strategy_filters_row() -> ChartLabelRow {
    let mut row = ChartLabelRow::new(LabelZone::ChartTop, LabelAlign::Left);
    row.preset = Some(LabelPreset::StrategyFilters);
    row.flow = LabelFlow::Column;
    row.gap = 8;
    row.push_part(ChartLabelField::StrategyFilters);
    row
}

/// The two market buttons every live chart drew before they became captions: `Panic Sell` and
/// `Cancel Buy`, side by side along the plot's bottom edge, pushed right.
///
/// One builder for the same reason [`instrument_row`] is one — the live set and the comparison's
/// both ship them, and a pair whose order or band drifted between the two would move the button
/// under the reader's pointer when they switched tabs.
fn action_rows() -> Vec<ChartLabelRow> {
    action_rows_at(Some(LabelAlign::Right), Some(LabelAlign::Right))
}

/// The market buttons as caption modules, each placed where it is asked for.
///
/// `None` means the button is not drawn at all, which is what the old layout's `Hide` said. The
/// public form of [`action_rows`] because the two callers must not disagree: the shipped set
/// places the pair, and the terminal's one-shot migration places whatever each tab had chosen, and
/// a second copy of the ordering rule below is how the migrated chart ends up mirroring the
/// shipped one.
///
/// The ORDER is the rule: a band places its first module OUTERMOST, so a right-aligned pair reads
/// panic-then-cancel and every other alignment cancel-then-panic — which in all three cases puts
/// `Cancel Buy` to the LEFT of `Panic Sell`, exactly where the chart drew them. Two buttons on the
/// same side share one line; the second continues the first one's rather than opening its own.
///
/// Args:
///     cancel: Where `Cancel Buy` goes, or `None` to leave it out.
///     panic: Where `Panic Sell` goes, or `None` to leave it out.
///
/// Returns:
///     Between zero and two rows, in the order they are to be placed.
pub fn action_rows_at(cancel: Option<LabelAlign>, panic: Option<LabelAlign>) -> Vec<ChartLabelRow> {
    let row = |field, align| {
        let mut row = ChartLabelRow::new(LabelZone::ChartBottom, align);
        row.push_part(field);
        row
    };
    let mut rows: Vec<ChartLabelRow> = [
        cancel.map(|align| row(ChartLabelField::ActCancelBuy, align)),
        panic.map(|align| row(ChartLabelField::ActPanicSell, align)),
    ]
    .into_iter()
    .flatten()
    .collect();
    // Two buttons on the SAME side are one line: the second continues the first one's instead of
    // opening its own. A right-aligned band fills from its edge inwards, so the pair is reversed
    // there — which puts `Cancel Buy` to the left of `Panic Sell` either way, exactly where the
    // chart drew them.
    if rows.len() == 2 && cancel == panic {
        if cancel == Some(LabelAlign::Right) {
            rows.reverse();
        }
        rows[1].placement = LabelFlow::Row;
    }
    rows
}

impl Default for ChartLabelsCfg {
    /// The working set the terminal ships with, and what the popup's Reset returns to.
    ///
    /// Not a designer's guess: this is the developer's own Main tab, transcribed from its
    /// `charts.json` entry on 2026-08-25 — ten modules, each placed and spaced by hand, and adopted
    /// as the shipped set so a fresh profile opens on a chart that has been USED rather than
    /// assembled.
    ///
    /// SIZES are deliberately absent from it: every caption here overrides nothing and draws at
    /// [`LABEL_SIZE_MULT_DEFAULT`], which is both what the shipped chart is meant to look like and
    /// what keeps a profile created today following that number if it ever moves again.
    ///
    /// Named through [`ChartLabelRow::preset`] rather than by a literal, so the popup speaks the
    /// reader's language: this set ships to everyone, and typed names would have shipped the
    /// developer's Russian to every locale. Three modules needed presets of their own for that —
    /// the badge, the measuring block and the session counters.
    ///
    /// Every optional figure disappears on its own when it has nothing to report, so a chart with
    /// no position shows the instrument, the badge and the volumes and nothing else.
    fn default() -> Self {
        let mut cfg = Self::empty();

        // The instrument, in the control strip pushed right: coin, core, venue stacked as a block.
        cfg.rows[0] = instrument_row(true);

        // The two scale badges on the plot's top-right corner: the price span, then the time span.
        let mut scale = ChartLabelRow::new(LabelZone::ChartTop, LabelAlign::Right);
        scale.preset = Some(LabelPreset::Scale);
        scale.push_part(ChartLabelField::ScaleBadge);
        scale.push_part(ChartLabelField::TimeScaleBadge);
        cfg.rows[1] = scale;

        // The coin's own movement: a block of two, standing BESIDE the badge rather than under it,
        // with room between them.
        let mut deltas = ChartLabelRow::new(LabelZone::ChartTop, LabelAlign::Right);
        deltas.preset = Some(LabelPreset::CoinDeltas);
        deltas.flow = LabelFlow::Column;
        deltas.placement = LabelFlow::Row;
        deltas.gap = 24;
        deltas.push_part(ChartLabelField::Delta1h);
        deltas.push_part(ChartLabelField::Delta24h);
        cfg.rows[2] = deltas;

        // What traded over the last minute, under the badge: the period, then the two sides. The
        // sides print bare — the heading above already names the period, and repeating it on every
        // line is the same word three times in a block three lines tall.
        let mut volumes = ChartLabelRow::new(LabelZone::ChartTop, LabelAlign::Right);
        volumes.preset = Some(LabelPreset::Volumes);
        volumes.flow = LabelFlow::Column;
        volumes.gap = 6;
        volumes.push_part(ChartLabelField::WindowSpanName);
        volumes.push_part(ChartLabelField::WindowBuyVolume);
        volumes.push_part(ChartLabelField::WindowSellVolume);
        for part in volumes.parts.iter_mut().filter(|p| p.is_used()) {
            part.window = LabelWindow::M1;
        }
        volumes.parts[1].style.caption = Some(false);
        volumes.parts[2].style.caption = Some(false);
        cfg.rows[3] = volumes;

        // The same figures MEASURED around the pointer, over ten seconds, with the liquidations
        // that landed there. No bars: this block answers "what happened right HERE", and a bar is
        // read by comparing it against the line above it.
        let mut cursor = ChartLabelRow::new(LabelZone::ChartTop, LabelAlign::Right);
        cursor.preset = Some(LabelPreset::CursorVolumes);
        cursor.flow = LabelFlow::Column;
        cursor.gap = 6;
        cursor.push_part(ChartLabelField::WindowSpanName);
        cursor.push_part(ChartLabelField::WindowBuyVolume);
        cursor.push_part(ChartLabelField::WindowSellVolume);
        cursor.push_part(ChartLabelField::WindowLiquidations);
        for part in cursor.parts.iter_mut().filter(|p| p.is_used()) {
            part.span = LabelSpan::Seconds(10);
            part.anchor = SpanAnchor::Cursor;
            part.bar = false;
        }
        cursor.parts[1].style.caption = Some(false);
        cursor.parts[2].style.caption = Some(false);
        cursor.parts[3].style.caption = Some(false);
        cfg.rows[4] = cursor;

        // What is open, as one line along the plot's top-left edge.
        let mut orders = ChartLabelRow::new(LabelZone::ChartTop, LabelAlign::Left);
        orders.preset = Some(LabelPreset::Position);
        orders.placement = LabelFlow::Row;
        orders.push_part(ChartLabelField::OpenOrders);
        orders.push_part(ChartLabelField::OpenPnlMoney);
        orders.push_part(ChartLabelField::OpenPnlPct);
        orders.push_part(ChartLabelField::Exposure);
        orders.parts[2].style.caption = Some(false);
        cfg.rows[5] = orders;

        // The two session counters under it: this core's own, and the one MoonBot prints.
        let mut session = ChartLabelRow::new(LabelZone::ChartTop, LabelAlign::Left);
        session.preset = Some(LabelPreset::Session);
        session.gap = 4;
        session.push_part(ChartLabelField::SessionPnl);
        session.push_part(ChartLabelField::SessionProfit);
        cfg.rows[6] = session;

        // Funding under that, spaced off the line; the countdown prints bare, beside the rate that
        // names itself.
        let mut funding = ChartLabelRow::new(LabelZone::ChartTop, LabelAlign::Left);
        funding.preset = Some(LabelPreset::Funding);
        funding.gap = 8;
        funding.push_part(ChartLabelField::Funding);
        funding.push_part(ChartLabelField::FundingIn);
        funding.parts[1].style.caption = Some(false);
        cfg.rows[7] = funding;

        // The venue roster down the plot's left edge. Only a spread worth acting on is coloured;
        // below half a percent the column would be a wall of green and red with nothing to find.
        let mut arbitrage = ChartLabelRow::new(LabelZone::ChartTop, LabelAlign::Left);
        arbitrage.preset = Some(LabelPreset::Arbitrage);
        arbitrage.flow = LabelFlow::Column;
        arbitrage.gap = 8;
        arbitrage.push_part(ChartLabelField::ArbColumn);
        arbitrage.parts[0].style.color = Some(LabelColor::BySign);
        arbitrage.parts[0].style.color_min_pct = Some(0.5);
        cfg.rows[8] = arbitrage;

        // What fired, and what is trading: centred over the plot, where a line of the core's own
        // prose has the width to be read. It is the only caption that WRAPS, so the modules beside
        // it yield to it — as much as it asks for, and never more than a share of the plot.
        let mut detect = ChartLabelRow::new(LabelZone::ChartTop, LabelAlign::Center);
        detect.preset = Some(LabelPreset::Detect);
        detect.flow = LabelFlow::Column;
        detect.push_part(ChartLabelField::DetectStrategy);
        detect.push_part(ChartLabelField::DetectMsg);
        detect.push_part(ChartLabelField::OrderStrategy);
        cfg.rows[9] = detect;

        // Why strategies skip this coin: a column on the plot's left, under the other left
        // modules, where MoonBot painted the overlay and where the reader can move or hide it.
        cfg.rows[10] = strategy_filters_row();

        // The market buttons along the bottom edge, where every chart drew them before they became
        // captions and where a hand reaching for `Panic Sell` still expects to find one.
        for row in action_rows() {
            cfg.push_prepared(row);
        }

        cfg
    }
}

impl ChartLabelsCfg {
    /// The working set a TRADE-DETAIL window opens with.
    ///
    /// Its own value rather than [`Self::default`] because that set is built for a LIVE chart:
    /// funding, the coin's deltas, what traded in the last minute, what is open right now. Printed
    /// over a trade that closed hours ago those figures are not stale, they are about a different
    /// thing entirely — and a caption is read as describing the picture under it.
    ///
    /// So this set states what the picture IS: which coin on which venue, and what the trade was —
    /// the strategy that opened it, the line it fired on, and why it closed. Everything else the
    /// window has to say is already in its own figures rail beside the chart, which is where the
    /// prices, the size and the profit live.
    ///
    /// It is a DEFAULT, not a fixture: the reader owns this view's captions like any other's, and
    /// the moment they set a default for it, theirs is what opens. See
    /// [`super::super::chart_defaults::ChartTabKind::Trade`].
    pub fn trade_default() -> Self {
        let mut cfg = Self::empty();

        // The Y-scale badge FIRST, so it takes the plot's top-right corner and the instrument
        // block stacks under it. This window fits each trade on its own — nothing pins its scale —
        // and the badge is what states how far the pane reaches while it does, which is what a
        // reader comparing two trades needs. Pinning a step from the window's own control hides it
        // by design: the step is then written on that control instead.
        cfg.push_preset(LabelPreset::Scale);

        // The same block the live default opens with, minus the core: "what am I looking at" is
        // the same question on a frozen chart, and which core recorded the trade is already the
        // first thing the window's own header states.
        //
        // Over the PLOT rather than in the control strip, which is the one thing this set changes
        // about it. Width is shared inside a ZONE: the strip and the plot are two of them, so a
        // block in the strip and the detect line over the plot cannot see each other and neither
        // yields — they simply overlap. In one zone the figures draw first and hand the prose what
        // is left (`chartdx::text::captions::widths`).
        let mut instrument = instrument_row(false);
        instrument.zone = LabelZone::ChartTop;
        cfg.push_prepared(instrument);

        // What the trade was, centred over the plot: the detect line is the widest thing this
        // window prints, and either edge would put it under the block above.
        cfg.push_preset(LabelPreset::Trade);

        // Repaired HERE, so the built-in set has one shape wherever it is read or compared —
        // rather than at each reader, where one of them would eventually forget.
        cfg.sanitize();
        cfg
    }

    /// The working set a COMPARISON opens with.
    ///
    /// Its own value rather than [`Self::default`] for the reason the trade window has one: a
    /// comparison is READ differently. Several panes of the same coin stand side by side, and the
    /// question asked of them is where this venue is against that one — so the figures that
    /// describe ONE market in depth are what has to go. The live default's volume block, its
    /// measuring block and its session counters are printed three or four times over on such a
    /// tab, in panes a third the width, and none of them is what the eye is there for.
    ///
    /// What stays is what a comparison is read by: which venue this pane is, how far its scale
    /// reaches, what is open on it, the venue roster, and the spread against the anchor. The last
    /// one — [`ChartLabelField::CompareDelta`] — is fed only on a book-only broom follower
    /// (`chartdx::text::captions`), so on any other pane it prints nothing and takes no room, the
    /// way every optional figure here behaves. It appears in no other shipped set for the same
    /// reason.
    ///
    /// Transcribed from the developer's own comparison tab on 2026-09-03, the way
    /// [`Self::default`] was transcribed from their main chart: a set that has been USED rather
    /// than assembled. Two SIZES come with it, which is where this set parts company with
    /// `default`'s "no sizes at all", and they go in OPPOSITE directions on purpose: against
    /// [`LABEL_SIZE_MULT_DEFAULT`] the badge is a step up and the venue roster a step down. In a
    /// pane a third of the usual width the badge is what a glance checks first, and the roster is
    /// a dozen lines that have to fit beside the plot at all. The cost is the one `default`
    /// documents — these two captions no longer follow that number if it moves — and it is
    /// accepted here because the sizes ARE the layout on a narrow pane.
    ///
    /// It follows the LOCK rather than the pane's width, and that is the intended reading: the
    /// anchor lock is a state the reader puts on and takes off, and while it is on the question
    /// being asked of the chart is "this venue against that one" whether the pane is a third of the
    /// screen or all of it. Locking a single full-width main chart therefore re-dresses it too, and
    /// unlocking hands it back the set of the kind its place gives it — nothing is written to the
    /// profile either way.
    ///
    /// It is a DEFAULT, not a fixture: the moment the reader sets one for comparisons, theirs is
    /// what opens. See [`super::super::chart_defaults::ChartTabKind::Compare`].
    pub fn compare_default() -> Self {
        // Stated as STEPS from the shared size rather than as absolutes, so this pair keeps its
        // relationship to that number if it ever moves — which is the property `default`'s "no
        // sizes at all" protects, kept here at the one place a size is worth spending.
        const BADGE_STEP: f32 = 0.2;
        const ROSTER_STEP: f32 = 0.25;

        let mut cfg = Self::empty();

        // The same block the live default opens with, in the control strip: on a tab where every
        // pane is the same coin, the venue under it is the pane's whole identity.
        cfg.push_prepared(instrument_row(true));

        // The Y-scale badge, a step ABOVE the shared size: two panes are only comparable while
        // their scales are, and this is the caption that states one. Through the preset rather than
        // hand-built, so its band and alignment cannot drift from the catalogue's.
        if let Some(ix) = cfg.push_preset(LabelPreset::Scale) {
            cfg.rows[ix].parts[0].style.size_mult = Some(LABEL_SIZE_MULT_DEFAULT + BADGE_STEP);
        }

        // What is open on THIS venue, as one line along the plot's top-left edge: the reason a
        // comparison is usually being looked at. Every figure keeps its caption, unlike the live
        // default's copy of this module — on a tab of near-identical panes the bare percentage
        // beside a bare amount is the one place a reader has to guess which is which.
        let mut orders = ChartLabelRow::new(LabelZone::ChartTop, LabelAlign::Left);
        orders.preset = Some(LabelPreset::Position);
        orders.placement = LabelFlow::Row;
        orders.push_part(ChartLabelField::OpenOrders);
        orders.push_part(ChartLabelField::OpenPnlMoney);
        orders.push_part(ChartLabelField::OpenPnlPct);
        orders.push_part(ChartLabelField::Exposure);
        cfg.push_prepared(orders);

        // The venue roster down the left edge, a step BELOW the shared size: it is the tallest
        // module the chart prints and a narrow pane has to hold all of it. Only a spread worth
        // acting on is coloured, exactly as the live default sets it.
        if let Some(ix) = cfg.push_preset(LabelPreset::Arbitrage) {
            let row = &mut cfg.rows[ix];
            row.gap = 8;
            row.parts[0].style.color = Some(LabelColor::BySign);
            row.parts[0].style.color_min_pct = Some(0.5);
            row.parts[0].style.size_mult = Some(LABEL_SIZE_MULT_DEFAULT - ROSTER_STEP);
        }

        // The spread against the anchor, in the strip's bottom band where the pane's own numbers
        // end. No preset and no name: the field names itself, and there is no module for it to be
        // one of.
        cfg.push_row(
            ChartLabelField::CompareDelta,
            LabelZone::ZoneBottom,
            LabelAlign::Right,
        );

        // The same buttons the live set ships: a comparison tab trades from its panes like any
        // other chart, and it drew both of them before they became captions.
        for row in action_rows() {
            cfg.push_prepared(row);
        }

        // Repaired here for [`Self::trade_default`]'s reason: one shape wherever it is compared.
        cfg.sanitize();
        cfg
    }
}
