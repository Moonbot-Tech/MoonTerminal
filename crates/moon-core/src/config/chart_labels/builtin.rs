//! The shipped label sets and their builders.

use super::*;

/// The instrument block used by Comparison and Trade: the coin, optionally its core, the venue.
///
/// Both sets share the same stacking and alignment so venue identity reads the same way.
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
/// Used when inserting the expanded filter column into an existing profile.
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
/// Main and Comparison ship the pair at its historical position.
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

/// The auto-figures eye for live charts, separated from the trading buttons on the other edge.
///
/// Its own builder keeps the display toggle out of the legacy trading-button migration.
fn auto_figures_eye_row() -> ChartLabelRow {
    let mut row = ChartLabelRow::new(LabelZone::ChartBottom, LabelAlign::Left);
    row.push_part(ChartLabelField::ActAutoFigures);
    row
}

/// The coin's own switches for a live chart: the temporary-blacklist lock, how long its ban has
/// left, and the favourites star — one line along the strip's bottom edge, pushed right.
///
/// Transcribed from the developer's Main chart on 2026-11-23, sizes included. The lock is the
/// chart's route to the coin menu's blacklists (`panels::chart::market_actions`); without it in
/// the shipped set a trader looking at a coin has no one-click way to put it on any list. The
/// countdown prints only while a ban runs, so a clean coin's line is the two buttons.
fn coin_switches_row() -> ChartLabelRow {
    const SIZE: f32 = 1.25;
    let mut row = ChartLabelRow::new(LabelZone::ZoneBottom, LabelAlign::Right);
    row.gap = 8;
    row.push_part(ChartLabelField::ActTempBan);
    row.push_part(ChartLabelField::TempBanLeft);
    row.push_part(ChartLabelField::ActFavorite);
    row.parts[0].style.size_mult = Some(SIZE);
    row.parts[1].style.size_mult = Some(SIZE);
    row
}

impl Default for ChartLabelsCfg {
    /// The owner's Main chart captions, also used when resetting the global caption settings.
    fn default() -> Self {
        const SIZE: f32 = 1.5;
        let mut cfg = Self::empty();

        let mut orders = ChartLabelRow::new(LabelZone::ChartTop, LabelAlign::Left);
        for field in [
            ChartLabelField::OpenOrders,
            ChartLabelField::Exposure,
            ChartLabelField::OpenPnlMoney,
            ChartLabelField::OpenPnlPct,
        ] {
            orders.push_part(field);
        }
        for part in orders.parts.iter_mut().filter(|part| part.is_used()) {
            part.style.size_mult = Some(SIZE);
        }
        orders.parts[0].style.color = Some(LabelColor::Fixed(0x8D99AE));
        cfg.push_prepared(orders);

        for (field, zone, align) in [
            (
                ChartLabelField::Coin,
                LabelZone::ZoneTop,
                LabelAlign::Center,
            ),
            (
                ChartLabelField::ScaleBadge,
                LabelZone::ChartTop,
                LabelAlign::Right,
            ),
            (
                ChartLabelField::Core,
                LabelZone::ChartTop,
                LabelAlign::Center,
            ),
        ] {
            if let Some(ix) = cfg.push_row(field, zone, align) {
                cfg.rows[ix].parts[0].style.size_mult = Some(SIZE);
            }
        }

        if let Some(ix) = cfg.push_preset(LabelPreset::StrategyFilters) {
            let row = &mut cfg.rows[ix];
            row.collapsed = true;
            row.parts[0].style.size_mult = Some(1.0);
            row.parts[0].style.caption = Some(false);
        }
        if let Some(ix) = cfg.push_preset(LabelPreset::Arbitrage) {
            let row = &mut cfg.rows[ix];
            row.zone = LabelZone::ChartBottom;
            row.parts[0].style.size_mult = Some(SIZE);
        }
        cfg.push_preset(LabelPreset::Session);

        for row in action_rows() {
            cfg.push_prepared(row);
        }
        cfg.push_prepared(auto_figures_eye_row());
        cfg.push_prepared(coin_switches_row());

        cfg
    }
}

impl ChartLabelsCfg {
    /// The working set a TRADE-DETAIL window opens with.
    ///
    /// Its own value rather than [`Self::default`] because that set is built for a LIVE chart:
    /// open positions, current exposure and session counters describe a live market. A trade
    /// window describes the closed trade and the market it was on.
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

        // The instrument block omits the core, which is already in the trade window's header.
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
    /// describe ONE market in depth would take space in every pane. Session counters belong on
    /// the main chart where they only appear once.
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
    /// than assembled. Compared with [`LABEL_SIZE_MULT_DEFAULT`], the badge is larger to make
    /// the pane's scale easy to find, and the roster is smaller so its rows fit beside the plot.
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
        // Use steps from the shared size so these two captions keep their proportions.
        const BADGE_STEP: f32 = 0.2;
        const ROSTER_STEP: f32 = 0.25;

        let mut cfg = Self::empty();

        // Every pane is the same coin, so the instrument block names each pane's venue.
        cfg.push_prepared(instrument_row(true));

        // The Y-scale badge, a step ABOVE the shared size: two panes are only comparable while
        // their scales are, and this is the caption that states one. Through the preset rather than
        // hand-built, so its band and alignment cannot drift from the catalogue's.
        if let Some(ix) = cfg.push_preset(LabelPreset::Scale) {
            cfg.rows[ix].parts[0].style.size_mult = Some(LABEL_SIZE_MULT_DEFAULT + BADGE_STEP);
        }

        // What is open on THIS venue, as one line along the plot's top-left edge: the reason a
        // comparison is usually being looked at. The captions distinguish the position figures.
        let mut orders = ChartLabelRow::new(LabelZone::ChartTop, LabelAlign::Left);
        orders.preset = Some(LabelPreset::Position);
        orders.placement = LabelFlow::Row;
        orders.push_part(ChartLabelField::OpenOrders);
        orders.push_part(ChartLabelField::OpenPnlMoney);
        orders.push_part(ChartLabelField::OpenPnlPct);
        orders.push_part(ChartLabelField::Exposure);
        cfg.push_prepared(orders);

        // The venue roster down the left edge, a step BELOW the shared size: it is the tallest
        // module the chart prints and a narrow pane has to hold all of it. Colour calls out
        // spreads of at least half a percent.
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

        // Keep both trading buttons available on every comparison pane.
        for row in action_rows() {
            cfg.push_prepared(row);
        }

        // The eye as well: a comparison's panes can draw auto figures like any live chart, and its
        // own kind ships them switched off — which is exactly when a per-coin "show" is wanted.
        cfg.push_prepared(auto_figures_eye_row());

        // Repaired here for [`Self::trade_default`]'s reason: one shape wherever it is compared.
        cfg.sanitize();
        cfg
    }
}
