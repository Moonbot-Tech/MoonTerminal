//! Core window and chart interface settings.

/// Moonbot's "Интерфейс" page: what that program's OWN windows and charts show, plus the handful of
/// input rules Moonbot files under the same tab.
///
/// Nothing here changes this terminal — it has its own chart, its own panels and its own theme.
/// These are carried so the expert window can read and set what the other program does, which is
/// the point of mirroring that dialog rather than approximating it. Most of it is appearance, but
/// not all: `buy_on_enter` and `dbl_click_panic_sell` change how a keypress and a click are
/// answered on a LIVE trading bot, and they are here because that is the page Moonbot puts them on.
///
/// Spread across four wire sections, as the page itself is: `trading` for the rules about what is
/// drawn on an order, `visual` for chart and order-book appearance, `ui` for the main-window
/// switches, and four flags of `signals` — the connectivity alert and three chart-window rules
/// Moonbot files under this tab.
///
/// Field names follow the WIRE, not Moonbot's caption, like every other block here — so the button
/// Moonbot calls "MoonBonus" is [`Self::hide_cashback_button`], which is what the section calls
/// it.
///
/// Moonbot's page has more controls than this, and the page draws the rest disabled;
/// `core_expert::pages::interface` says which and why, so that inventory has one home rather than
/// one per crate.
///
/// Eleven of the fields below were among those dead rows until Moonbot's own binary was read,
/// because the wire's prose does not place them. Two joins place them, and both are mechanical.
/// moonproto's field names were derived from Moonbot's CONTROL names — `bGlassOpacity` to
/// `glass_opacity`, `cbFreePositionCheck` to `free_position_check` — so a control found in the form
/// is a field found in the section. And the exe's localisation table lays out English, Russian and
/// Spanish in that order, followed by that row's HINT in the same three, so the caption a trader
/// reads and the sentence explaining it travel together.
///
/// The hint is what settles most of them, and it is evidence of a different kind from the names: it
/// says what the flag DOES, in Moonbot's own words, where the wire's prose has now been wrong eight
/// times. Each field below carries its own, where one exists.
#[derive(Debug, Clone)]
pub struct InterfaceSettings {
    /// `trading.buy_on_enter`: the Enter key buys.
    pub buy_on_enter: bool,
    /// `trading.dbl_click_panic_sell`: the panic-sell button needs a double click.
    pub dbl_click_panic_sell: bool,
    /// `trading.chart_split_zones`: draw the split-zone lines on the chart.
    pub chart_split_zones: bool,
    /// `trading.draw_stop`: draw the stop-loss line on the chart.
    pub draw_stop: bool,
    /// `trading.pending_orders_spread` and `trading.pending_orders_spread_h_delta`: the spread a
    /// pending order is placed at, and the hDelta term added to it.
    pub pending_orders_spread: f64,
    pub pending_orders_spread_h_delta: f64,
    /// `visual.hide_forum_label`.
    pub hide_forum_label: bool,
    /// `visual.scrolling_charts`.
    pub scrolling_charts: bool,
    /// `visual.startup_load_charts`: open the saved charts when the core starts.
    pub startup_load_charts: bool,
    /// `visual.hide_right_chart_panel`.
    pub hide_right_chart_panel: bool,
    /// `visual.left_chart_info`: the chart's info panel sits on the LEFT.
    ///
    /// Moonbot's checkbox says the opposite ("Информация на графике справа"), so the expert window
    /// negates it. Kept in the wire's polarity here, where the wire's name is the contract.
    pub left_chart_info: bool,
    /// `visual.show_iceberg`.
    pub show_iceberg: bool,
    /// `visual.show_orders_captions` and `visual.orders_captions_lower`.
    pub show_orders_captions: bool,
    pub orders_captions_lower: bool,
    /// `visual.hide_pnl`.
    pub hide_pnl: bool,
    /// `visual.hide_buy_button`.
    pub hide_buy_button: bool,
    /// `visual.hide_cashback_button` — the button Moonbot's dialog calls "MoonBonus".
    pub hide_cashback_button: bool,
    /// `visual.remember_chart_buttons`.
    pub remember_chart_buttons: bool,
    /// `visual.show_filters.scale_tool`.
    pub scale_tool: bool,
    /// `visual.icon_selection`: index of the tray-icon variant. Shown, not chosen — the protocol
    /// carries no table to name the variants with.
    pub icon_selection: i32,
    /// `visual.colors.price_line_width`.
    pub price_line_width: i32,
    /// `visual.panic_sell_opacity`, in whole per cent.
    pub panic_sell_opacity: i32,
    /// `visual.glass_opacity`, `visual.book_cumulative_opacity` and `visual.book_orders_opacity` in
    /// whole per cent, plus `visual.book_orders_width` in pixels: the three opacities of Moonbot's
    /// "Прозрачность зон стакана" group, and how wide an order level is drawn.
    ///
    /// `glass_opacity` is the "Границы" track, and Moonbot's own form says so by POSITION rather
    /// than by name: under the group label `lOrderBookSettings` its three tracks sit on one row,
    /// left to right at x=14, 152 and 290 — `bGlassOpacity`, `bBookCumulative`, `bBookOrders` —
    /// against the caption run "Границы", "Заливка", "Ордера" in that order.
    ///
    /// Worth stating because the wire disagrees: its own doc calls this one the orderbook PANEL
    /// opacity, and its default of 5 beside a fill of 100 reads oddly for a border. That prose has
    /// already proved wrong about three other fields in this section, and the three defaults do
    /// compose as independent parts — a solid fill, invisible levels, a faint border — where a
    /// panel-wide 5 would make the fill's 100 meaningless.
    pub glass_opacity: i32,
    pub book_cumulative_opacity: i32,
    pub book_orders_opacity: i32,
    pub book_orders_width: i32,
    /// `signals.play_signal_sound`: play a sound on NETWORK problems — a disconnect or high
    /// latency, throttled by the core to once every five seconds.
    ///
    /// Named after the wire like everything else here, and the wire's own doc warns that the name
    /// is historical: it is a connectivity alert, not a signal sound. It sits in THIS block rather
    /// than in [`crate::feed::SignalsSettings`] beside the wire fields it neighbours, because an area is a PAGE:
    /// Moonbot draws this switch on the Interface page and the compact popup draws it nowhere, so
    /// leaving it in the signals block would have let that popup's OK write its own frozen copy of
    /// a control it never showed.
    pub play_signal_sound: bool,
    /// `signals.signal_sound`: WHICH sound `play_signal_sound` plays, as a 1-based ordinal into
    /// Moonbot's sound list like the two price-approach sounds in [`crate::feed::SignalsSettings`].
    ///
    /// The wire doc files this one under "incoming signal notifications", and that reading was
    /// what kept the picker beside the switch dead for a while. It is the switch's own sound: the
    /// Interface page draws exactly one sound picker in that row, and nothing else in the section
    /// is left for it to belong to. Here rather than in `SignalsSettings` for the reason the switch
    /// is: an area is a PAGE, and the compact popup never draws this pair.
    pub signal_sound: i32,
    /// `ui.confirm_close`: ask before closing Moonbot.
    pub confirm_close: bool,
    /// `ui.hide_demo_button`.
    pub hide_demo_button: bool,
    /// `signals.auto_show_on_signal`: bring Moonbot's own window up when a signal arrives.
    ///
    /// No hint on this row. It rests on the control: `CheckBox5` in the main-window group carries
    /// the design-time text "Auto Show on signal", which is the wire's name for the field.
    pub auto_show_on_signal: bool,
    /// `visual.show_market_captions`: Moonbot's "Подсказки на графике".
    ///
    /// Placed by POSITION, the way [`Self::glass_opacity`] was, because its own control carries a
    /// stale placeholder instead of its text. The page reproduces Moonbot's column; the two rows
    /// under this one are `cbShowMarketUSD` at (11, 112) and `cbShowIceberg` at (11, 136) and both
    /// are settled independently; and nineteen of that group's twenty controls are accounted for by
    /// a row of this page. So the slot at (11, 88) is `cbShowMarketCaptions`.
    ///
    /// Which does NOT make the wire's "market name captions" what it does: this row's own hint says
    /// "display order replacement status and activity messages on the chart area". The control kept
    /// a name it outgrew and the wire's field inherited that name. The name is the join; the hint is
    /// the meaning.
    pub show_market_captions: bool,
    /// `visual.show_usd_on_charts`: Moonbot's "Показывать профит в $" — its hint, "show profit in $
    /// on market charts and in the orders list".
    ///
    /// The weakest join of these, and worth saying why it holds anyway. The control is
    /// `cbShowMarketUSD`, which does NOT derive this field's name — the rule above would give
    /// `show_market_usd`. What carries it is that the control's design-time text is the caption
    /// itself, "Show profit in $", and that this is the only USD field in the whole snapshot.
    pub show_usd_on_charts: bool,
    /// `visual.show_detects_tool`: the detect buttons get a WINDOW of their own — the row's hint is
    /// "show alert buttons in a separate window".
    ///
    /// The wire calls it a button on the chart toolbar, and that is the reading the hint refutes.
    /// The control is `cbDetectsTool`, whose own design-time text reads "Separate alert window",
    /// and it sits in the main-window group rather than the chart one. Its nearest rival,
    /// `visual.show_filters.show_detects`, loses on the name: this control is a detects TOOL.
    pub show_detects_tool: bool,
    /// `visual.auto_request_charts`: pull chart history from Moonbot's server — its hint,
    /// "auto-load charts from the MoonServer (if unchecked, you can still load one manually)".
    pub auto_request_charts: bool,
    /// `visual.new_markets_max_scale`: a new market's chart opens compressed along TIME rather than
    /// zoomed in — the hint is "open new charts in max. time scale (6 hours)", which is what
    /// Moonbot's "В сжатом виде" means and why its own English for the row is "Open in max scale".
    pub new_markets_max_scale: bool,
    /// `ui.new_markets_on_top`: a new market's chart opens above the others — the hint is "open new
    /// charts on top of the charts workspace".
    ///
    /// The wire says "newly listed markets at the top of the LIST", and the hint is what refutes
    /// that: the row is about charts. The control is `cbNewMarketsOnTop`, in the chart group beside
    /// `cbNewMarketsMaxScale`.
    pub new_markets_on_top: bool,
    /// `signals.use_last_detect_caption`: the last detect's caption becomes the chart's title — the
    /// hint states the whole rule, "update chart's caption with last detect info; if unchecked, the
    /// very first detect will be used".
    pub use_last_detect_caption: bool,
    /// `signals.full_screen_prevent_signals`: in full screen, a signal opens no second chart —
    /// Moonbot's "Только 1 график в Full Screen".
    ///
    /// No hint on this row. It rests on both sides being unique: `cbFullScreenPreventSIgnals` is the
    /// only full-screen control in the dialog, and this is the only full-screen field in the
    /// snapshot.
    pub full_screen_prevent_signals: bool,
    /// `trading.pending_buy_price`: DRAW a pending order's buy price on the chart.
    ///
    /// The wire documents a sell-calculation rule instead — "use pending-buy price instead of the
    /// current ask for sell calculations" — and the hint settles it outright: "draw the buy price of
    /// a pending order as an additional line on a chart; the main order's line is its conditional
    /// price". The control is `cbPendingBuyPrice`, in the chart group at (11, 475).
    ///
    /// Worth this much text because it is the one row of these whose two readings differ in
    /// CONSEQUENCE: cosmetic under the hint, live sell pricing under the wire's prose. Under either
    /// reading the box is Moonbot's own box carrying Moonbot's own caption, so this window stays a
    /// faithful mirror — but the hazard is named here rather than left for a trader to find.
    pub pending_buy_price: bool,
    /// `trading.cashback_settings.hide_info`: hide the cashback TABLE — Moonbot's "Скрыть табличку
    /// Candy", drawn beside the button [`Self::hide_cashback_button`] hides.
    ///
    /// Two controls one word apart: `bHideCashBack` is the button and `bHideCashBackInfo` is this
    /// one, and the second name is the one that carries "info".
    pub hide_cashback_info: bool,
}

/// Hand-written for the same reason [`crate::feed::ManualSettings`]'s is: the two spreads are `f64` read off the
/// wire, and a core holding a non-finite one must still compare equal to ITSELF. Under a derived
/// `PartialEq` it would not — IEEE says `NaN != NaN` — and
/// `feed::live::shared_config::edit_satisfied` would then be permanently false for that core, so
/// every OK on it would burn all three attempts and give up.
impl PartialEq for InterfaceSettings {
    fn eq(&self, other: &Self) -> bool {
        // Destructured for the reason [`crate::feed::GeneralSettings`]'s is.
        let Self {
            buy_on_enter,
            dbl_click_panic_sell,
            chart_split_zones,
            draw_stop,
            pending_orders_spread,
            pending_orders_spread_h_delta,
            hide_forum_label,
            scrolling_charts,
            startup_load_charts,
            hide_right_chart_panel,
            left_chart_info,
            show_iceberg,
            show_orders_captions,
            orders_captions_lower,
            hide_pnl,
            hide_buy_button,
            hide_cashback_button,
            remember_chart_buttons,
            scale_tool,
            icon_selection,
            price_line_width,
            panic_sell_opacity,
            glass_opacity,
            book_cumulative_opacity,
            book_orders_opacity,
            book_orders_width,
            play_signal_sound,
            signal_sound,
            confirm_close,
            hide_demo_button,
            auto_show_on_signal,
            show_market_captions,
            show_usd_on_charts,
            show_detects_tool,
            auto_request_charts,
            new_markets_max_scale,
            new_markets_on_top,
            use_last_detect_caption,
            full_screen_prevent_signals,
            pending_buy_price,
            hide_cashback_info,
        } = self;
        pending_orders_spread
            .total_cmp(&other.pending_orders_spread)
            .is_eq()
            && pending_orders_spread_h_delta
                .total_cmp(&other.pending_orders_spread_h_delta)
                .is_eq()
            && *buy_on_enter == other.buy_on_enter
            && *dbl_click_panic_sell == other.dbl_click_panic_sell
            && *chart_split_zones == other.chart_split_zones
            && *draw_stop == other.draw_stop
            && *hide_forum_label == other.hide_forum_label
            && *scrolling_charts == other.scrolling_charts
            && *startup_load_charts == other.startup_load_charts
            && *hide_right_chart_panel == other.hide_right_chart_panel
            && *left_chart_info == other.left_chart_info
            && *show_iceberg == other.show_iceberg
            && *show_orders_captions == other.show_orders_captions
            && *orders_captions_lower == other.orders_captions_lower
            && *hide_pnl == other.hide_pnl
            && *hide_buy_button == other.hide_buy_button
            && *hide_cashback_button == other.hide_cashback_button
            && *remember_chart_buttons == other.remember_chart_buttons
            && *scale_tool == other.scale_tool
            && *icon_selection == other.icon_selection
            && *price_line_width == other.price_line_width
            && *panic_sell_opacity == other.panic_sell_opacity
            && *glass_opacity == other.glass_opacity
            && *book_cumulative_opacity == other.book_cumulative_opacity
            && *book_orders_opacity == other.book_orders_opacity
            && *book_orders_width == other.book_orders_width
            && *play_signal_sound == other.play_signal_sound
            && *signal_sound == other.signal_sound
            && *confirm_close == other.confirm_close
            && *hide_demo_button == other.hide_demo_button
            && *auto_show_on_signal == other.auto_show_on_signal
            && *show_market_captions == other.show_market_captions
            && *show_usd_on_charts == other.show_usd_on_charts
            && *show_detects_tool == other.show_detects_tool
            && *auto_request_charts == other.auto_request_charts
            && *new_markets_max_scale == other.new_markets_max_scale
            && *new_markets_on_top == other.new_markets_on_top
            && *use_last_detect_caption == other.use_last_detect_caption
            && *full_screen_prevent_signals == other.full_screen_prevent_signals
            && *pending_buy_price == other.pending_buy_price
            && *hide_cashback_info == other.hide_cashback_info
    }
}
