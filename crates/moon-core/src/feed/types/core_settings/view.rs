//! Moonbot's own appearance: the chart overlays, volumes, candles and annotations its window
//! paints, and the layout of its other windows (report grid, news, cashback, transfer, monitors).
//!
//! None of it changes this terminal, which draws its own chart and panels. It is projected because
//! on a core that runs without its window — a Linux server — the settings would otherwise be
//! unreachable, and because a trader moving a core between machines expects the window to come
//! back the way they left it.
//!
//! One AREA, not a page: the expert window draws these on its Interface tab under Moonbot's own
//! rows, and the compact popup draws none of them, so no mask but the expert window's names it.
//!
//! Eight of these fields are TAIL fields of the `visual` section — the five candle settings and
//! the three AI-card ones. A core built before them sends a shorter block and moonproto fills its
//! own defaults, so an edit of one of them there never echoes and the write exhausts its retries,
//! the same trade-off [`crate::feed::OrderRulesSettings::deltas_by_trades`] states. Left untouched
//! they cost nothing: the value sent is the value read.
//!
//! What is deliberately NOT here: the candle palettes, the drawing tools' colour arrays, the
//! report's column widths and filters, the market table's column arrays and the INI colour
//! sections are composite values a row of controls cannot edit honestly; fields moonproto's
//! reference marks legacy, learned or remembered dialog state are kept as the core sends them;
//! and `visual.manual_charts_full_screen` stays where `apply_interface` explains.

/// Moonbot's own appearance — see the module doc. Every field names its wire slot first.
#[derive(Debug, Clone)]
pub struct ViewSettings {
    /// `signals.show_cross`: Show the chart crosshair in the Moonbot GUI. Does not change signal
    /// detection or order execution.
    pub show_cross: bool,
    /// `signals.base_vol_ind_type`: Chart Bv/Sv display interval: -2 = last N minutes, -1 = last N
    /// trades, 0 = pre-pump view, or 1, 3, 5, 15, 30 minutes. N comes from `base_vol_n`. This is
    /// not a minimum-volume trading filter.
    pub base_vol_ind_type: i32,
    /// `signals.base_vol_n`: N for the chart's N-trades or N-minutes Bv/Sv display, selected by
    /// `base_vol_ind_type`. Market initialization enforces at least 2; the minutes view uses at
    /// most 1440. Not a 24-hour-volume threshold.
    pub base_vol_n: i32,
    /// `trading.price_range`: Chart price-scale preset, not an order-price filter. 1 = automatic;
    /// 2/3/4/5/6 select progressively different fixed spans. Normal spot spans are 20/50/100/10/5%;
    /// futures spans are 10/20/50/5/2%. `x9_mode` and `scale_plus_index` can replace these spans.
    pub price_range: i32,
    /// `trading.x9_mode`: Wide chart-price scale mode: standard buttons use 100%, 200%, 500%, 1000%
    /// and 2000% spans. Does not create a nine-order grid or split orders.
    pub x9_mode: bool,
    /// `trading.draw_all_buy`: Show completed/partially filled buy markers on charts, including
    /// historical ones. This controls drawing, not the set of active pending orders.
    pub draw_all_buy: bool,
    /// `trading.show_order_comment`: Show the comment editor in the pending-order dialog. Does not
    /// control all strategy labels on charts.
    pub show_order_comment: bool,
    /// `trading.multi_orders.show_orders_num`: Show the worker/order number alongside order labels
    /// on charts. Does not change order count or grouping.
    pub show_orders_num: bool,
    /// `trading.multi_orders.done_opacity`: Base opacity of completed order lines, 0..=1. Some
    /// closed/unfilled lines are faded further by the renderer; not a retention period.
    pub done_opacity: f32,
    /// `trading.alert_config.keep_time`: How long a figure/price-alert button remains active, in
    /// seconds; GUI 10..=3600. A strategy alert can supply its own KeepAlert instead.
    pub alert_keep_time: i32,
    /// `trading.alert_config.sound_kind`: Sound preset 1..=11; 0 plays nothing. Strategy figure
    /// alerts can use their own named sound. Preset names are in MoonDoc's
    /// sound reference.
    pub alert_sound: i32,
    /// `trading.alert_config.repeat_count`: Repeat counter for a figure alert, GUI 1..=10. Repeats
    /// are checked at five-second intervals while the alert remains active, in addition to the
    /// initial notification; `keep_time` can end them earlier.
    pub alert_repeat: i32,
    /// `trading.report_config.active_type`: Report UI selection: 0 all non-deleted records, 1
    /// active non-deleted, 2 closed non-deleted, 3 also include deleted records. Does not select a
    /// custom query or delete records.
    pub report_active_type: u8,
    /// `trading.report_config.rep_emulator_orders`: Choose emulator-only records when true,
    /// real-only records when false. It does not merge emulator trades with real ones.
    pub report_emulator: bool,
    /// `trading.report_config.stretch`: Fit visible report columns to the available GUI width;
    /// false uses saved column widths.
    pub report_stretch: bool,
    /// `trading.report_config.only_2_orders`: In a static saved-trade chart, show the selected
    /// trade's buy/sell pair rather than unrelated order lines. Does not aggregate multi-orders
    /// into one report row.
    pub report_only_2_orders: bool,
    /// `trading.report_config.use_leverage`: Select the leverage-aware spent-amount basis for
    /// report summary percentages. Its effect depends on `trading.use_lev_for_take`; it does not
    /// change exchange leverage or stored execution prices.
    pub report_use_leverage: bool,
    /// `trading.report_config.max_lines`: Maximum rows loaded into the Moonbot report grid; the
    /// normal setter clamps to 50..=1000. Report totals use a separate query without this row
    /// limit. This is not an SDK request page size.
    pub report_max_lines: u16,
    /// `trading.report_config.pos_direction`: Report UI direction filter: 0 both, 1 long, 2 short.
    /// Does not change order direction.
    pub report_direction: u8,
    /// `trading.report_config.store_liq`: Write detected liquidation events as report records. Not
    /// a switch for recording a liquidation-price column in ordinary trades.
    pub report_store_liq: bool,
    /// `trading.report_config.store_fund`: Write received funding payments as separate report
    /// records. The current path excludes inverse-base-currency mode and global emulator mode. Does
    /// not merely store a funding-rate indicator.
    pub report_store_fund: bool,
    /// `trading.report_config.by_close_date`: Apply the report's date-range filter to closing time
    /// instead of buying/opening time. Sorting has separate state; this is not the sort-order
    /// switch.
    pub report_by_close_date: bool,
    /// `trading.spot_config.show_trades`: Draw the corresponding spot/reference market's trade dots
    /// when that stream is available. Separate from the current market's trade-dot switch.
    pub spot_show_trades: bool,
    /// `trading.spot_config.show_book`: Draw the corresponding spot/second orderbook when supported
    /// and fresh. Does not hide the primary market's orderbook.
    pub spot_show_book: bool,
    /// `trading.spot_config.book_len`: Visual width multiplier for the second/spot orderbook, in
    /// percent. Does not request this percentage of exchange depth levels.
    pub spot_book_len: f32,
    /// `trading.spot_config.show_market_avg`: Overlay the aggregate market-average history,
    /// normalized to the current chart. Weighting follows `trading.m_avg_use_vol_weight`; not
    /// necessarily volume-weighted.
    pub show_market_avg: bool,
    /// `trading.spot_config.show_min_max`: Draw historical minimum/maximum price-envelope curves
    /// from the market's stored extrema. Not simply two 24-hour high/low markers.
    pub show_min_max: bool,
    /// `trading.spot_config.show_avg_price`: Draw the market's smoothed price history from
    /// second/minute averages. Does not show the user's average position entry price.
    pub show_avg_price: bool,
    /// `trading.spot_config.spot_btc`: Select a BTC-based corresponding/reference-market feed where
    /// supported, instead of the usual matching spot comparison. Changing it can require the
    /// kernel's subscription-refresh path; not just a BTC text label.
    pub spot_btc: bool,
    /// `trading.spot_config.shift_spot`: Shift spot/reference prices vertically by the current
    /// spot/futures price difference for comparison, except BTC-reference mode. Does not compensate
    /// for panel width or alter order prices.
    pub shift_spot: bool,
    /// `trading.spot_config.show_mark_price`: Draw mark-price history when the market has enough
    /// mark-price samples. Requires a feed supporting mark price; does not configure liquidation
    /// protection.
    pub show_mark_price: bool,
    /// `trading.spot_config.show_liq`: Draw exchange liquidation-event dots on the chart. These are
    /// market events, not the liquidation-price line of the user's own position.
    pub show_liq: bool,
    /// `trading.spot_config.huge_liq`: Increase the drawn size of liquidation-event dots when
    /// `show_liq` is on. Does not filter events by liquidation amount.
    pub huge_liq: bool,
    /// `trading.spot_config.show_open_int`: Draw futures open-interest history and cursor
    /// information when available. Does not enable a trading filter.
    pub show_open_int: bool,
    /// `trading.spot_config.show_avg_price_line`: Draw the market's recent averaged-price history
    /// line. Separate from `show_avg_price`'s second/minute smoothed curves and from position entry
    /// prices.
    pub show_avg_price_line: bool,
    /// `trading.cashback_settings.sep`: Display spot and futures cashback estimates separately
    /// instead of their sum.
    pub cashback_sep: bool,
    /// `trading.cashback_settings.max_days`: Period selector, not a literal day count: 0 all stored
    /// history, 1 last 90 days, 2 last 30 days, 3 current half-month payout-period selection.
    /// Current-day data is included separately. GUI clamps the index to 0..=3.
    pub cashback_period: i32,
    /// `trading.cashback_settings.instant_bnb`: Use the stored at-the-time BNB valuation for
    /// cashback estimates. When false, recent records with currency components revalue the BNB
    /// component at the current cached BNB price; old/incomplete records fall back to stored
    /// totals. Does not buy, sell or convert BNB.
    pub cashback_instant_bnb: bool,
    /// `trading.transfer_config.show_total`: Show free quantity, total quantity and estimated USD
    /// value in each asset row. False shows free quantity only; does not transfer or reserve funds.
    pub transfer_show_total: bool,
    /// `trading.transfer_config.hide_zero`: Hide an asset only when neither its free nor total
    /// quantity is positive. Does not delete the asset or its balance.
    pub transfer_hide_zero: bool,
    /// `trading.arb_view_config.show_absolute`: Show absolute arbitrage price differences in the
    /// numeric block; requires `show_numbers`. May be combined with `show_percent`.
    pub arb_show_absolute: bool,
    /// `trading.arb_view_config.show_numbers`: Show numeric arbitrage differences when arbitrage
    /// data is active. Does not create subscriptions or enable arbitrage trading.
    pub arb_show_numbers: bool,
    /// `trading.arb_view_config.show_lines`: Draw corresponding arbitrage price lines when
    /// arbitrage data is active. Independent of numeric-block visibility.
    pub arb_show_lines: bool,
    /// `trading.arb_view_config.show_percent`: Show percentage differences in the numeric block.
    /// When both `show_percent` and `show_absolute` are false, the renderer still uses percent as
    /// its fallback.
    pub arb_show_percent: bool,
    /// `trading.arb_view_config.show_right`: Place the arbitrage numeric block on the right instead
    /// of the left of the chart.
    pub arb_show_right: bool,
    /// `visual.show_red_green_lines`: Draw bid/ask price-history lines. The renderer uses the
    /// supported book-ticker path when enabled, otherwise its ordinary price-history path.
    pub show_red_green_lines: bool,
    /// `visual.show_red_green_dots`: Draw market trade dots. Candle/combo modes can force a tick
    /// layer independently; this does not stop collecting trades.
    pub show_red_green_dots: bool,
    /// `visual.auto_close_charts`: Enable automatic cleanup of eligible inactive charts using
    /// `trading.chart_clean_up_time` and the kernel's activity/load checks. Does not close a chart
    /// immediately just because another coin was selected.
    pub auto_close_charts: bool,
    /// `visual.chart_watch_multi_frames`: Keep multiple saved-trade charts in the chart-watch
    /// viewer. False clears previous viewer frames when another saved chart is opened; not a
    /// multi-timeframe switch.
    pub chart_watch_multi_frames: bool,
    /// `visual.chart_history_len`: Extra wait in minutes after a trade before saving its chart
    /// history, GUI 0..=60. Normal saving waits at least about three seconds even at 0;
    /// chart-forwarding paths use their own 15..90-second bounds. Not a number of historical
    /// candles to request.
    pub chart_history_len: i32,
    /// `visual.show_vert_volume`: Show the vertical trade-volume histogram. Uses the fast renderer;
    /// candle mode can supply candle-volume bins. Separate from whether the trade dots themselves
    /// are shown.
    pub show_vert_volume: bool,
    /// `visual.vert_vol_time_frame`: Volume bin index: 0 automatic by chart span, 1 five seconds, 2
    /// fifteen seconds, 3 thirty seconds, 4 one minute, 5 three minutes, 6 five minutes.
    /// Candle-volume rendering can use the selected candle timeframe instead.
    pub vert_vol_time_frame: i32,
    /// `visual.vert_vol_opacity`: Vertical-volume opacity as a fraction 0..=1. GUI percentages are
    /// divided by 100 before storing.
    pub vert_vol_opacity: f32,
    /// `visual.vert_vol_height`: Vertical-volume maximum height as a fraction of chart height,
    /// 0..=1. Not a number of pixels.
    pub vert_vol_height: f32,
    /// `visual.vert_vol_kind`: Volume display: 0 overlay buy/sell bars from the same baseline, 1
    /// stack buy and sell volumes. Not bars versus area rendering.
    pub vert_vol_kind: i32,
    /// `visual.vert_vol_ind_pos`: Volume-scale placement: 0 left, 1 right. Changes label/scale
    /// layout, not the histogram time direction.
    pub vert_vol_ind_pos: i32,
    /// `visual.v_tool_show`: Enable the cursor volume tool and, with horizontal volumes, its
    /// price-range measurements. Does not enable a strategy detector.
    pub v_tool_show: bool,
    /// `visual.v_tool_time`: Cursor-tool time-window length in seconds; 0 uses the current
    /// vertical-volume interval. Keep consistent with `v_tool_time_index` when editing the preset.
    pub v_tool_time: f64,
    /// `visual.v_tool_time_index`: GUI preset index 0..=10 mapping to `v_tool_time` seconds: 0, 2,
    /// 5, 15, 30, 60, 180, 300, 900, 1800, 3600. The GUI writes both fields; the renderer reads
    /// `v_tool_time`.
    pub v_tool_time_index: i32,
    /// `visual.v_tool_opacity`: Opacity fraction 0..=1 for the cursor-tool selection area; the
    /// time-selection rectangle uses half this value.
    pub v_tool_opacity: f32,
    /// `visual.hv_show`: Show the horizontal volume profile by price. Uses the fast renderer and
    /// applies to the main chart rather than every secondary frame.
    pub hv_show: bool,
    /// `visual.hv_width`: Horizontal-volume panel width as a fraction of available chart width,
    /// 0..=1.
    pub hv_width: f32,
    /// `visual.hv_time_frame_ind`: Horizontal-volume history selector: 0 automatic; 1..=9
    /// correspond to 1, 5, 15, 30, 60, 120, 360, 720, 1440 minutes. Index 10 uses the available
    /// deep candle archive, not an exact guaranteed 48-hour window.
    pub hv_time_frame_ind: i32,
    /// `visual.hv_price_frame`: Price smoothing half-width for horizontal-volume accumulation, in
    /// percent, GUI 0..=1. The renderer uses at least the market's percentage price tick. Not a
    /// fraction of the visible chart price range.
    pub hv_price_frame: f32,
    /// `visual.hv_opacity`: Horizontal-volume stroke opacity, fraction 0..=1.
    pub hv_opacity: f32,
    /// `visual.hv_disp_vol`: Horizontal-volume label placement: 0 right with cleared background, 1
    /// left with guide line, 2 right over the chart, 3 left over the chart with guide line.
    pub hv_disp_vol: i32,
    /// `visual.hv_kind`: Horizontal-volume composition: 0 overlay buyer/seller volumes, 1 stack
    /// them. Deep-candle archive mode has combined volume rather than a true buy/sell split.
    pub hv_kind: i32,
    /// `visual.charts_auto_shift`: Keep eligible charts following the latest time automatically.
    /// Panned/history views and chart modes still influence recentering.
    pub charts_auto_shift: bool,
    /// `visual.draw_cur_price`: Current-price label placement: 0 in the right panel, 1 in the
    /// orderbook spread, 2 hidden. Not a solid/dashed line selector.
    pub draw_cur_price: i32,
    /// `visual.scale_plus_index`: Extra price-span preset: 0 normal scale; 1..=9 select 100%, 200%,
    /// 500%, 1000%, 2000%, 5000%, 10000%, 20000%, 50000%. Used in the expanded price-scale mode,
    /// not as a generic zoom-step size.
    pub scale_plus_index: i32,
    /// `visual.vo_sort_kind`: Order-panel sort: 0 entries first, 1 exits first, 2 creation order.
    /// Applies to GUI order controls, not API delivery order.
    pub vo_sort_kind: u8,
    /// `visual.vo_older_first`: Show older order controls first; false reverses the normal order
    /// when not grouping by market.
    pub vo_older_first: bool,
    /// `visual.vo_sort_by_market`: Group GUI order controls by market. The VDS layout suppresses
    /// this grouping; it can override the older/newer ordering preference.
    pub vo_sort_by_market: bool,
    /// `visual.show_strat_numbers`: Prefix strategy names in the strategy tree with their numeric
    /// identifier. Does not change strategy IDs.
    pub show_strat_numbers: bool,
    /// `visual.hide_ny_elka`: Hide the seasonal New Year decoration/greetings when that seasonal UI
    /// would otherwise be shown.
    pub hide_ny_elka: bool,
    /// `visual.spy_hide_mode`: Visually blur or suppress selected balance/profit/order information
    /// in Moonbot's GUI. Not a data-access or secrecy control: the underlying values remain
    /// available to the program/API.
    pub spy_hide_mode: bool,
    /// `visual.chart_candles_style`: Candle bodies: 0 filled, 1 outlines, 2 outlines where raw
    /// ticks are available and filled outside that region. Does not select a candle interval.
    pub chart_candles_style: u8,
    /// `visual.chart_candles_tick_opacity`: Candle opacity in the region with raw ticks, integer
    /// percent 0..=100. Does not set opacity of the trade dots themselves.
    pub chart_candles_tick_opacity: u8,
    /// `visual.chart_candles_neutral_ticks`: Use the neutral candle color in the region with raw
    /// ticks regardless of bullish/bearish direction. Not specifically a doji-color rule.
    pub chart_candles_neutral_ticks: bool,
    /// `visual.chart_candles_outline_width`: Candle outline thickness setting 1..=3; renderer
    /// scales it for DPI and limits it to the actual candle width.
    pub chart_candles_outline_width: u8,
    /// `visual.chart_candles_tick_wicks`: Also draw candle wicks in the raw-tick region. Outside
    /// that region wicks are drawn independently of this flag.
    pub chart_candles_tick_wicks: bool,
    /// `visual.ai_card_provider`: Provider index in this core: 0 OpenAI, 1 Anthropic, 2 Google
    /// Gemini, 3 xAI Grok, 4 DeepSeek, 5 Perplexity, 6 OpenRouter. Out-of-range selects 0. API keys
    /// are separate private settings and are not included in SharedConfig.
    pub ai_card_provider: u8,
    /// `visual.ai_card_model`: Model ID for the selected provider. The core uses it only if present
    /// in that provider's current cached/fallback list; otherwise it chooses the first available
    /// model. Do not hardcode a universal model list in a settings editor.
    pub ai_card_model: String,
    /// `visual.ai_card_prompt`: AI-card prompt template; blank uses the built-in prompt for the GUI
    /// language. Supported placeholders: {symbol}, {exchange}, {price}, {deltas}, {volume}, {mcap},
    /// {news}. Saving the template does not send an AI request.
    pub ai_card_prompt: String,
    /// `visual.custom_draw_config.f_show`: Show/enable interaction with chart drawing objects.
    /// Hiding them does not delete saved objects.
    pub draw_show: bool,
    /// `visual.custom_draw_config.f_opacity`: Global drawing-stroke opacity, 0..=1. Order-line
    /// opacity has its own tool field.
    pub draw_opacity: f32,
    /// `visual.show_filters.need_show`: Show the strategy-filter/session annotation area and
    /// eligible MoonShot bounds on charts. Does not enable the underlying strategy conditions.
    pub filters_show: bool,
    /// `visual.show_filters.filters`: Show strategy-filter annotations when `need_show` is enabled.
    pub filters_strategies: bool,
    /// `visual.show_filters.sessions`: Show session annotations when `need_show` is enabled.
    pub filters_sessions: bool,
    /// `visual.show_filters.hide_zero`: Hide inactive or near-zero session annotations, using the
    /// session limits, counters and penalty state. This is not a report filter for exactly zero
    /// profit.
    pub filters_hide_zero: bool,
    /// `visual.show_filters.m_shot_area`: Show eligible MoonShot price boundaries in the annotation
    /// area; requires `need_show`.
    pub filters_moonshot: bool,
    /// `visual.show_filters.scroll_filters`: Allow Alt+mouse-wheel scrolling inside the visible
    /// filter area.
    pub filters_scroll: bool,
    /// `visual.show_filters.show_detects`: Show detection annotations on charts. Separate from the
    /// `need_show` filter-area switch and from detection execution.
    pub filters_detects: bool,
    /// `visual.charts_settings.auto_arrange`: Automatically arrange charts in monitoring windows.
    pub monitor_auto_arrange: bool,
    /// `visual.charts_settings.visible`: Saved visibility of monitoring windows, not all
    /// main-window charts.
    pub monitor_visible: bool,
    /// `visual.charts_settings.wide`: Use the wide layout of charts in monitoring windows.
    pub monitor_wide: bool,
    /// `visual.charts_settings.stay_on_top`: Keep monitoring windows above other windows.
    pub charts_stay_on_top: bool,
    /// `visual.charts_settings.max_charts`: Maximum chart frames per monitoring window. GUI
    /// minimum: 10. Not the number of monitoring windows; use the received value rather than SDK
    /// Default (0).
    pub monitor_max_charts: i32,
    /// `visual.charts_settings.refresh`: Recentring interval selector: 0=5 s, 1=15 s, 2=30 s, 3=60
    /// s, 4=300 s, 5=900 s. Recentres charts around the latest price; not FPS or a data-download
    /// interval.
    pub monitor_refresh: i32,
    /// `visual.heatmap_config.show`: Show the heatmap area. Trade-size heatmap requires fast charts
    /// and market-maker detection; selecting any CPU/trade/app/draw diagnostic overlay replaces it
    /// with system data. Not a new market subscription.
    pub heatmap_show: bool,
    /// `visual.heatmap_config.use_q`: Calculate relative heatmap intensity from trade quantity
    /// (normalized by a common average price), instead of each trade's notional at its own price.
    pub heatmap_use_q: bool,
    /// `visual.heatmap_config.height`: Heatmap height as a fraction of the chart; GUI 0.05..=0.8.
    pub heatmap_height: f32,
    /// `visual.heatmap_config.cpu`: Show the CPU history overlay. A display switch, not a detailed
    /// profiler.
    pub heatmap_cpu: bool,
    /// `visual.heatmap_config.trades`: Show trade-feed latency history, not trade count or
    /// throughput.
    pub heatmap_trades: bool,
    /// `visual.heatmap_config.app_latency`: Show application-latency history.
    pub heatmap_app_latency: bool,
    /// `visual.heatmap_config.draw_latency`: Show chart-rendering latency history.
    pub heatmap_draw_latency: bool,
    /// `visual.news_form_config.stay_on_top`: Keep the news window above other windows.
    pub news_stay_on_top: bool,
    /// `visual.news_form_config.exact_time`: Show exact news time instead of relative age.
    pub news_exact_time: bool,
    /// `visual.news_form_config.font_size`: News-card font size; GUI 12..=16.
    pub news_font_size: u8,
    /// `visual.news_form_config.update_orig`: Use text in the current GUI language when available;
    /// false uses the original text. Not a server-side news-edit permission.
    pub news_translate: bool,
    /// `visual.news_form_config.sound`: News sound-list index: 0=off; positive indices select the
    /// kernel's named-sound list. This is not a boolean or the ordinary 1..11 alert-preset list.
    pub news_sound: u8,
    /// `visual.news_form_config.theme`: News theme: 0=Light Paper, 1=Light Default, 2=Light
    /// Classic, 3=Light Quartz, 4=Light Mist, 5=Light Sand, 6=Dark Carbon, 7=Dark Navy, 8=Dark
    /// Slate. Separate from the main GUI style.
    pub news_theme: i32,
    /// `visual.news_form_config.coin_card_font_size`: Coin-card font size: non-positive means
    /// default 13; positive values are clamped to 8..=30.
    pub news_coin_card_font: i32,
    /// `visual.news_form_config.full_tags`: Show all tags on a news card. When false and tag
    /// selections exist, show only selected matching tags; does not alter the news itself.
    pub news_full_tags: bool,
    /// `visual.news_form_config.feed_mode`: Show the news feed without the selected-tag visibility
    /// filter. Token filters still apply. Not a scrolling/layout mode.
    pub news_feed_mode: bool,
    /// `theme.current_style`: Main GUI style: 0=Native, 1=Light, 2=Diamond, 3=Dark, 4=Dark
    /// OldSchool. Styles 3/4 use the dark color set; 0/1/2 use the light one.
    pub current_style: i32,
    /// `ui.coins_sort_order`: Market-table sort direction multiplier: +1 or -1. Selected column is
    /// `markets_table_config.sort_col`. Not a column selector.
    pub coins_sort_order: i32,
    /// `ui.markets_table_config.sort_col`: Zero-based displayed column index; -1 restores
    /// market-index order. The current GUI has 28 displayed columns, listed in MoonDoc; not the
    /// internal metric ID.
    pub markets_sort_col: i32,
    /// `ui.main_button_index_1`: First configurable toolbar button: 1=Help, 2=Reports, 3=Alerts,
    /// 4=Assets/Transfer, 6=BackTest. Other values normalize to Help.
    pub main_button: u8,
}

/// Hand-written for the reason [`crate::feed::SpecialSettings`]'s is: eleven of these come off the
/// wire as floats, and a core holding a non-finite one must still compare equal to itself.
impl PartialEq for ViewSettings {
    fn eq(&self, other: &Self) -> bool {
        // Destructured for the reason [`crate::feed::GeneralSettings`]'s is: a field added to the
        // struct without a comparison here fails to compile.
        let Self {
            show_cross,
            base_vol_ind_type,
            base_vol_n,
            price_range,
            x9_mode,
            draw_all_buy,
            show_order_comment,
            show_orders_num,
            done_opacity,
            alert_keep_time,
            alert_sound,
            alert_repeat,
            report_active_type,
            report_emulator,
            report_stretch,
            report_only_2_orders,
            report_use_leverage,
            report_max_lines,
            report_direction,
            report_store_liq,
            report_store_fund,
            report_by_close_date,
            spot_show_trades,
            spot_show_book,
            spot_book_len,
            show_market_avg,
            show_min_max,
            show_avg_price,
            spot_btc,
            shift_spot,
            show_mark_price,
            show_liq,
            huge_liq,
            show_open_int,
            show_avg_price_line,
            cashback_sep,
            cashback_period,
            cashback_instant_bnb,
            transfer_show_total,
            transfer_hide_zero,
            arb_show_absolute,
            arb_show_numbers,
            arb_show_lines,
            arb_show_percent,
            arb_show_right,
            show_red_green_lines,
            show_red_green_dots,
            auto_close_charts,
            chart_watch_multi_frames,
            chart_history_len,
            show_vert_volume,
            vert_vol_time_frame,
            vert_vol_opacity,
            vert_vol_height,
            vert_vol_kind,
            vert_vol_ind_pos,
            v_tool_show,
            v_tool_time,
            v_tool_time_index,
            v_tool_opacity,
            hv_show,
            hv_width,
            hv_time_frame_ind,
            hv_price_frame,
            hv_opacity,
            hv_disp_vol,
            hv_kind,
            charts_auto_shift,
            draw_cur_price,
            scale_plus_index,
            vo_sort_kind,
            vo_older_first,
            vo_sort_by_market,
            show_strat_numbers,
            hide_ny_elka,
            spy_hide_mode,
            chart_candles_style,
            chart_candles_tick_opacity,
            chart_candles_neutral_ticks,
            chart_candles_outline_width,
            chart_candles_tick_wicks,
            ai_card_provider,
            ai_card_model,
            ai_card_prompt,
            draw_show,
            draw_opacity,
            filters_show,
            filters_strategies,
            filters_sessions,
            filters_hide_zero,
            filters_moonshot,
            filters_scroll,
            filters_detects,
            monitor_auto_arrange,
            monitor_visible,
            monitor_wide,
            charts_stay_on_top,
            monitor_max_charts,
            monitor_refresh,
            heatmap_show,
            heatmap_use_q,
            heatmap_height,
            heatmap_cpu,
            heatmap_trades,
            heatmap_app_latency,
            heatmap_draw_latency,
            news_stay_on_top,
            news_exact_time,
            news_font_size,
            news_translate,
            news_sound,
            news_theme,
            news_coin_card_font,
            news_full_tags,
            news_feed_mode,
            current_style,
            coins_sort_order,
            markets_sort_col,
            main_button,
        } = self;
        *show_cross == other.show_cross
            && *base_vol_ind_type == other.base_vol_ind_type
            && *base_vol_n == other.base_vol_n
            && *price_range == other.price_range
            && *x9_mode == other.x9_mode
            && *draw_all_buy == other.draw_all_buy
            && *show_order_comment == other.show_order_comment
            && *show_orders_num == other.show_orders_num
            && done_opacity.total_cmp(&other.done_opacity).is_eq()
            && *alert_keep_time == other.alert_keep_time
            && *alert_sound == other.alert_sound
            && *alert_repeat == other.alert_repeat
            && *report_active_type == other.report_active_type
            && *report_emulator == other.report_emulator
            && *report_stretch == other.report_stretch
            && *report_only_2_orders == other.report_only_2_orders
            && *report_use_leverage == other.report_use_leverage
            && *report_max_lines == other.report_max_lines
            && *report_direction == other.report_direction
            && *report_store_liq == other.report_store_liq
            && *report_store_fund == other.report_store_fund
            && *report_by_close_date == other.report_by_close_date
            && *spot_show_trades == other.spot_show_trades
            && *spot_show_book == other.spot_show_book
            && spot_book_len.total_cmp(&other.spot_book_len).is_eq()
            && *show_market_avg == other.show_market_avg
            && *show_min_max == other.show_min_max
            && *show_avg_price == other.show_avg_price
            && *spot_btc == other.spot_btc
            && *shift_spot == other.shift_spot
            && *show_mark_price == other.show_mark_price
            && *show_liq == other.show_liq
            && *huge_liq == other.huge_liq
            && *show_open_int == other.show_open_int
            && *show_avg_price_line == other.show_avg_price_line
            && *cashback_sep == other.cashback_sep
            && *cashback_period == other.cashback_period
            && *cashback_instant_bnb == other.cashback_instant_bnb
            && *transfer_show_total == other.transfer_show_total
            && *transfer_hide_zero == other.transfer_hide_zero
            && *arb_show_absolute == other.arb_show_absolute
            && *arb_show_numbers == other.arb_show_numbers
            && *arb_show_lines == other.arb_show_lines
            && *arb_show_percent == other.arb_show_percent
            && *arb_show_right == other.arb_show_right
            && *show_red_green_lines == other.show_red_green_lines
            && *show_red_green_dots == other.show_red_green_dots
            && *auto_close_charts == other.auto_close_charts
            && *chart_watch_multi_frames == other.chart_watch_multi_frames
            && *chart_history_len == other.chart_history_len
            && *show_vert_volume == other.show_vert_volume
            && *vert_vol_time_frame == other.vert_vol_time_frame
            && vert_vol_opacity.total_cmp(&other.vert_vol_opacity).is_eq()
            && vert_vol_height.total_cmp(&other.vert_vol_height).is_eq()
            && *vert_vol_kind == other.vert_vol_kind
            && *vert_vol_ind_pos == other.vert_vol_ind_pos
            && *v_tool_show == other.v_tool_show
            && v_tool_time.total_cmp(&other.v_tool_time).is_eq()
            && *v_tool_time_index == other.v_tool_time_index
            && v_tool_opacity.total_cmp(&other.v_tool_opacity).is_eq()
            && *hv_show == other.hv_show
            && hv_width.total_cmp(&other.hv_width).is_eq()
            && *hv_time_frame_ind == other.hv_time_frame_ind
            && hv_price_frame.total_cmp(&other.hv_price_frame).is_eq()
            && hv_opacity.total_cmp(&other.hv_opacity).is_eq()
            && *hv_disp_vol == other.hv_disp_vol
            && *hv_kind == other.hv_kind
            && *charts_auto_shift == other.charts_auto_shift
            && *draw_cur_price == other.draw_cur_price
            && *scale_plus_index == other.scale_plus_index
            && *vo_sort_kind == other.vo_sort_kind
            && *vo_older_first == other.vo_older_first
            && *vo_sort_by_market == other.vo_sort_by_market
            && *show_strat_numbers == other.show_strat_numbers
            && *hide_ny_elka == other.hide_ny_elka
            && *spy_hide_mode == other.spy_hide_mode
            && *chart_candles_style == other.chart_candles_style
            && *chart_candles_tick_opacity == other.chart_candles_tick_opacity
            && *chart_candles_neutral_ticks == other.chart_candles_neutral_ticks
            && *chart_candles_outline_width == other.chart_candles_outline_width
            && *chart_candles_tick_wicks == other.chart_candles_tick_wicks
            && *ai_card_provider == other.ai_card_provider
            && *ai_card_model == other.ai_card_model
            && *ai_card_prompt == other.ai_card_prompt
            && *draw_show == other.draw_show
            && draw_opacity.total_cmp(&other.draw_opacity).is_eq()
            && *filters_show == other.filters_show
            && *filters_strategies == other.filters_strategies
            && *filters_sessions == other.filters_sessions
            && *filters_hide_zero == other.filters_hide_zero
            && *filters_moonshot == other.filters_moonshot
            && *filters_scroll == other.filters_scroll
            && *filters_detects == other.filters_detects
            && *monitor_auto_arrange == other.monitor_auto_arrange
            && *monitor_visible == other.monitor_visible
            && *monitor_wide == other.monitor_wide
            && *charts_stay_on_top == other.charts_stay_on_top
            && *monitor_max_charts == other.monitor_max_charts
            && *monitor_refresh == other.monitor_refresh
            && *heatmap_show == other.heatmap_show
            && *heatmap_use_q == other.heatmap_use_q
            && heatmap_height.total_cmp(&other.heatmap_height).is_eq()
            && *heatmap_cpu == other.heatmap_cpu
            && *heatmap_trades == other.heatmap_trades
            && *heatmap_app_latency == other.heatmap_app_latency
            && *heatmap_draw_latency == other.heatmap_draw_latency
            && *news_stay_on_top == other.news_stay_on_top
            && *news_exact_time == other.news_exact_time
            && *news_font_size == other.news_font_size
            && *news_translate == other.news_translate
            && *news_sound == other.news_sound
            && *news_theme == other.news_theme
            && *news_coin_card_font == other.news_coin_card_font
            && *news_full_tags == other.news_full_tags
            && *news_feed_mode == other.news_feed_mode
            && *current_style == other.current_style
            && *coins_sort_order == other.coins_sort_order
            && *markets_sort_col == other.markets_sort_col
            && *main_button == other.main_button
    }
}
