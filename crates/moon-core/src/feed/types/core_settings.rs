//! Projection of the core's full safe-share configuration onto the settings surfaces the terminal
//! actually draws.
//!
//! MoonProto carries core settings over TWO channels. The compact `ClientSettingsCommand` feeds the
//! toolbar's frequently changed controls and is projected by [`super::ClientSettings`]. The full
//! `SharedConfig` — around 530 fields across six sections — carries everything else, and the
//! runtime already requests and retains it after `Ready` whether or not anything reads it. This
//! module projects the slice of that snapshot the gear popup renders; adding a tab means adding
//! fields here and to `feed::live::shared_config`'s read/write pair, never touching the transport.
//!
//! [`CoreConfig::manual`] is the one exception to "adding a tab": it is a BLOCK, not a UI tab —
//! nothing renders it as a gear-popup page. It still follows the same "one field per protocol
//! section rather than per UI tab" rule below, because the manual-trading toolbar and header
//! controls need the same comparable, transport-agnostic shape every other section gets.
//!
//! Nothing here depends on moonproto: the mapping between these types and the wire sections lives
//! in `feed::live::shared_config`, so the UI layer stays transport-agnostic like the rest of
//! `feed::types`.

/// Slice of the core's safe-share configuration the terminal renders.
///
/// One field per protocol section rather than per UI tab: a tab is free to mix sections (the
/// AutoStart tab draws `trading.auto_start` beside `visual.blink_config`), while a section is a
/// stable address that survives the UI being rearranged.
#[derive(Debug, Clone, PartialEq)]
pub struct CoreConfig {
    /// `trading.auto_start` and `trading.auto_start_2`.
    pub auto_start: AutoStartSettings,
    /// `visual.blink_config`.
    pub btc_blink: BtcBlinkSettings,
    /// `signals` — the price-approach alert sounds.
    pub signals: SignalsSettings,
    /// Exit rules, iceberg and blacklist fields spread across `trading`.
    pub general: GeneralSettings,
    /// `trading.auto_manage_lev` and `trading.auto_lev_control`.
    pub leverage: LeverageSettings,
    /// Moonbot's own window and chart appearance, spread across `trading`, `visual` and `ui`.
    pub interface: InterfaceSettings,
    /// Moonbot's autobuy page: the signal sources and the message filter.
    pub auto_buy: AutoBuySettings,
    /// Moonbot's Telegram page: the signal channels and the rules over them.
    pub telegram: TelegramSettings,
    /// Core-owned manual-trading configuration: order-size presets, manual-strategy buttons, and
    /// the platform hotkey layout. A BLOCK, not a tab — see the module doc.
    pub manual: ManualSettings,
}

/// Moonbot's two price-approach alerts: a sound when the last price comes within N per cent of an
/// order's sell price, and the same for its buy price.
///
/// Field names follow the wire names in `moonproto::shared_config::SignalsSection`, like every
/// other block here, which is why the SELL alert's sound is spelled `signal_sound_2`: that is what
/// the section calls it. The pairing is read off the section's own field order, where each sound
/// sits with the alert flag and level it belongs to — `signal_sound_2` between `sell_alert_level`
/// and `play_sell_alert`, `buy_signal_sound` beside `play_buy_alert` and `buy_alert_level`. The
/// wire's own doc for `signal_sound_2` says only "the second alert tier", so the pairing is the
/// one thing here that layout evidence rather than a stated contract settles; a core whose two
/// rows come back swapped in the popup is the symptom, and the fix is to swap them here.
///
/// The two levels are WHOLE PER CENT, as the wire carries them and as Moonbot's own spinner shows
/// them — not fractions. Zero is a legitimate value and means "when the price has reached the
/// order's price", not "off"; the flags are what switch each alert off.
///
/// The sounds are 1-BASED ordinals into Moonbot's own sound list, not names: the protocol carries
/// no table to label them with. The terminal's copy of that list, in the order Moonbot shows it,
/// lives beside the player in `moon-ui-gpui`'s `media::sound`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SignalsSettings {
    /// Play a sound when the price approaches an order's SELL price.
    pub play_sell_alert: bool,
    /// How near, in whole per cent, the price must come for the sell alert to fire.
    pub sell_alert_level: i32,
    /// Sound for the sell alert, as a 1-based ordinal into Moonbot's sound list.
    pub signal_sound_2: i32,
    /// Play a sound when the price approaches an order's BUY price.
    pub play_buy_alert: bool,
    /// How near, in whole per cent, the price must come for the buy alert to fire.
    pub buy_alert_level: i32,
    /// Sound for the buy alert, as a 1-based ordinal into Moonbot's sound list.
    pub buy_signal_sound: i32,
}

/// Automatic start, stop, restart, and panic-sell rules — the Moonbot "AutoStart" settings tab.
///
/// Field names follow the wire names in `moonproto::shared_config::{AutoStartConfig,
/// AutoStartConfig2}` so a value can be traced to its section without a translation table. The one
/// deliberate departure is the work-time window: see [`AutoStartSettings::work_time_from_min`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AutoStartSettings {
    // --- Enable on launch ---
    /// Start the market runtime when the core launches.
    pub auto_start: bool,
    /// Enable the detection engine on start.
    pub auto_detect_on: bool,
    /// Enable strategies on start.
    pub strategies_on: bool,
    /// Restore the last running state across restarts instead of applying the three flags above.
    pub remember_state: bool,
    /// Auto-update the core to new Moonbot releases.
    pub auto_update: bool,
    /// Wire `dont_wait_sells`. The Moonbot checkbox reads "wait if open sells exist", so the UI
    /// shows the INVERSE of this field; the projection keeps the wire polarity.
    pub dont_wait_sells: bool,

    // --- Work-time window ---
    /// Restrict trading to a time-of-day window.
    pub work_time: bool,
    /// Window start as MINUTES since midnight, converted from the wire's fraction-of-day.
    ///
    /// The wire stores a `f64` fraction (0.9999 ≈ 23:59) whose precision exceeds one minute, so a
    /// blind round-trip through minutes would rewrite a value the user never touched. Applying an
    /// edit therefore writes the fraction back ONLY when the minute value actually changed; see
    /// `feed::live::shared_config::apply_auto_start`.
    pub work_time_from_min: u16,
    /// Window end as minutes since midnight, under the same round-trip rule as
    /// [`Self::work_time_from_min`].
    pub work_time_to_min: u16,

    // --- Loss cap over a trade window ---
    /// Stop when the cumulative loss over the last [`Self::stop_trades`] trades exceeds
    /// [`Self::auto_stop_loss`].
    pub auto_stop_if_loss: bool,
    /// Loss threshold in quote currency for the trade-window rule.
    pub auto_stop_loss: f64,
    /// Number of trades in the loss-calculation window.
    pub stop_trades: i32,
    /// Also panic-sell every order when the trade-window rule fires.
    pub sell_if_loss: bool,

    // --- Loss cap over an hourly window ---
    /// Stop when the loss over [`Self::stop_hours`] hours exceeds [`Self::auto_stop_hours_val`].
    pub auto_stop_if_loss_hours: bool,
    /// Loss threshold in quote currency for the hourly rule.
    pub auto_stop_hours_val: f64,
    /// Hours to look back for the hourly loss calculation.
    pub stop_hours: i32,
    /// Minimum trades in the hourly window before the rule can fire.
    pub stop_hours_trades: i32,
    /// Exclude emulator orders from both loss calculations.
    pub ignore_emulator: bool,

    // --- Session reset (AutoStartConfig2) ---
    /// Reset the session profit counters periodically.
    pub reset_session: bool,
    /// Hours between session resets.
    pub rs_hours: i32,
    /// Maximum session cap in quote currency.
    pub max_session_cap: i32,

    // --- Global panic sell ---
    /// Panic-sell everything on a BTC move.
    pub panic_btc: bool,
    /// Hourly BTC drop (%) that triggers the panic sell.
    pub panic_btc_delta: f64,
    /// Hourly BTC rise (%) that triggers the panic sell.
    pub panic_btc_delta_up: f64,
    /// Panic-sell everything on an average market drop.
    pub panic_market: bool,
    /// Hourly average market drop (%) that triggers the panic sell.
    pub panic_market_delta: f64,

    // --- Restart on market conditions (AutoStartConfig2) ---
    /// Restart trading once the market is back inside the band below.
    pub restart_on_market: bool,
    /// BTC delta must exceed this % to restart.
    pub btc_higher_then: f64,
    /// BTC delta must stay below this % to restart.
    pub btc_lower_then: f64,
    /// Market delta must exceed this % to restart.
    pub market_higher_then: f64,

    // --- Error watchdog ---
    /// Stop detection once the error count reaches [`Self::errors_level`].
    pub auto_stop_on_errors: bool,
    /// Error count that qualifies as persistent.
    pub errors_level: i32,
    /// Also panic-sell every order on persistent errors.
    pub sell_all_on_errors: bool,
    /// Restart after [`Self::restart_err_time`] following an error stop.
    pub restart_after_err: bool,
    /// Delay before an error-triggered restart, in the core's own unit.
    ///
    /// UNIT UNVERIFIED: moonproto documents this field as seconds, while the Moonbot page this tab
    /// reproduces labels the same box "restart after N minutes". The value is passed through
    /// unchanged, so no conversion can be wrong here — only the label, which follows Moonbot until
    /// a live core settles it.
    pub restart_err_time: i32,

    // --- Ping watchdog ---
    /// Stop detection once the ping exceeds [`Self::ping_level`].
    pub auto_stop_on_ping: bool,
    /// Ping in milliseconds that qualifies as high latency.
    pub ping_level: i32,
    /// Also panic-sell every order on a ping stop.
    pub sell_all_on_ping: bool,
    /// Restart after [`Self::restart_ping_time`] following a ping stop.
    pub restart_after_ping: bool,
    /// Delay before a ping-triggered restart, under the same unit caveat as
    /// [`Self::restart_err_time`].
    pub restart_ping_time: i32,
}

/// BTC price blink and alarm settings from `visual.blink_config`.
///
/// Drawn at the bottom of the Moonbot AutoStart tab even though it lives in the visual section, and
/// it is the ONLY channel that carries these two controls — the compact settings snapshot has no
/// counterpart for them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BtcBlinkSettings {
    /// Highlight the BTC rate when it moves past either threshold.
    pub blink_btc: bool,
    /// Hourly BTC drop (%) that triggers the highlight.
    pub blink_btc_delta: f64,
    /// Hourly BTC rise (%) that triggers the highlight.
    pub blink_btc_delta_up: f64,
    /// Play a sound alongside the highlight.
    pub alarm_btc: bool,
    /// Sound variant, an opaque Moonbot ordinal.
    pub alarm_type: u8,
}

/// Exit rules and risk limits the gear popup's General tab edits.
///
/// The stop, trailing and V-Stop rules carry their own enable flag here, unlike the compact
/// `ClientSettings` projection where a zero value has to stand in for "off": the safe-share section
/// keeps `trailing_stop` and `panic_if_vol_drop` beside their levels, which is what lets a disabled
/// rule remember the level it was disabled at.
#[derive(Debug, Clone, PartialEq)]
pub struct GeneralSettings {
    /// `trading.use_g_take_profit` and `trading.g_take_profit`: sell at entry plus this percentage.
    pub take_profit_on: bool,
    pub take_profit_pct: f64,
    /// `trading.trailing_stop` and `trading.trailing_drop`: sell when price falls this far below the
    /// peak.
    pub trailing_on: bool,
    pub trailing_pct: f32,
    /// `trading.panic_if_vol_drop` and `trading.vol_drop_level`: sell when the BID volume at the buy
    /// price drops by this whole percentage.
    pub vstop_on: bool,
    pub vol_drop_level: i32,
    /// `trading.buy_iceberg` / `trading.sell_iceberg`.
    pub buy_iceberg: bool,
    pub sell_iceberg: bool,
    /// `trading.use_coins_black_list` and `trading.coins_black_list_text`.
    pub blacklist_on: bool,
    pub blacklist_text: String,
    /// `trading.exclude_black_list_delta`.
    ///
    /// The terminal ALSO keeps a client-side filter of the same name (moonproto applies it to the
    /// retained market analytics without asking the core), so committing this field drives both.
    pub exclude_blacklisted_from_deltas: bool,
}

/// Moonbot's "Телеграм" page: which channels a signal may come from, and the rules over them.
///
/// An area is a PAGE — see [`InterfaceSettings`]. This one is `signals` plus the one `trading` flag
/// Moonbot files under the same tab, and it does not overlap [`AutoBuySettings`]: that page owns
/// how a message is PARSED, this one owns where messages come from.
///
/// Moonbot's own dialog shows one channel box. The wire keeps a primary channel and a list of
/// additional ones, so this block keeps them apart and the page shows the primary first. Adding a
/// channel appends to [`Self::pump_channels`] and removing takes from it; the primary is shown but
/// not removable here, because which of the additional channels would take its place is a rule the
/// protocol does not state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelegramSettings {
    /// `signals.pump_channel`: the primary signal channel.
    pub pump_channel: String,
    /// `signals.pump_channels`: the additional channels used in multi-channel mode.
    pub pump_channels: Vec<String>,
    /// `signals.multi_channels`: accept signals from more than one channel at once.
    pub multi_channels: bool,
    /// `signals.more_then_1_channel`: buy only a token seen in two channels.
    pub more_then_1_channel: bool,
    /// `signals.listen_moon_channel`: listen to Moonbot's own signal channel.
    pub listen_moon_channel: bool,
    /// `trading.use_moon_bl`: use the Moonbot-curated cloud blacklist.
    pub use_moon_bl: bool,
}

/// Moonbot's "АвтоПокупка" page: where a buy signal may come from, and which messages count.
///
/// An area is a PAGE, not a wire section — see [`InterfaceSettings`] for why. This one reads from
/// `signals`, from its `signal_config` sub-record, and from one field of `trading`, which is exactly
/// how Moonbot's own page is put together.
///
/// It deliberately does NOT overlap [`SignalsSettings`]: that block is the two price-approach alert
/// sounds, which the compact popup draws and this page does not. One wire field belongs to one
/// area, or a write from either surface would put the other's frozen copy back.
///
/// The three-button "search mode" is two wire flags per source, and the UI writes both together —
/// the shape [`LeverageSettings`] uses for isolated-versus-cross, and for the same reason: Moonbot's
/// own control is exclusive, so a packet carrying half the choice would leave the core in a state
/// that dialog cannot show. The wire's factory default sets both flags at once, which is a value
/// that dialog normalises rather than one its user can reach; the page therefore stages nothing on
/// a click that changes no mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutoBuySettings {
    /// `signals.monitor_clipboard`: watch the clipboard for token names at all.
    ///
    /// Moonbot's "захватывать буфер", by elimination: its own dialog puts no switch in the group's
    /// caption, and this is the only clipboard control left once the others have their fields.
    pub monitor_clipboard: bool,
    /// `signals.clipboard_auto_buy`: buy when the clipboard yields a token.
    pub clipboard_auto_buy: bool,
    /// `signals.lower_case_token_cbd` / `signals.look_full_link_cbd` /
    /// `signals.advanced_filter_clipboard`: the clipboard source's search mode.
    ///
    /// The last two are the mode pair described above: written together, never singly.
    pub lower_case_token_cbd: bool,
    pub look_full_link_cbd: bool,
    pub advanced_filter_clipboard: bool,
    /// `signals.telegram_auto_buy`: buy when a Telegram signal matches.
    pub telegram_auto_buy: bool,
    /// `signals.lower_case_token_tlg` / `signals.look_full_link_tlg` / `signals.advanced_filter`:
    /// the Telegram source's search mode, in the same shape.
    pub lower_case_token_tlg: bool,
    pub look_full_link_tlg: bool,
    pub advanced_filter: bool,
    /// `signals.dont_buy_reply`: ignore a signal that is a reply to another message.
    pub dont_buy_reply: bool,
    /// `signals.msg_keywords_long` and `signals.msg_keywords_short`: comma-separated words that
    /// mark a message as a long or a short signal.
    pub msg_keywords_long: String,
    pub msg_keywords_short: String,
    /// `signals.msg_black_words`: words whose presence cancels the signal.
    pub msg_black_words: String,
    /// `signals.msg_token_tags`: the tag prefixes a ticker is looked for behind, e.g. `#,$`.
    pub msg_token_tags: String,
    /// `signals.lower_price_words`: words that mean "wait for a lower price" rather than "buy now".
    pub lower_price_words: String,
    /// `signal_config.use_keywords` and `signal_config.buy_key_dist`: require a keyword, and how
    /// many words may stand between it and the token.
    pub use_keywords: bool,
    pub buy_key_dist: i32,
    /// `signal_config.use_black_words`.
    pub use_black_words: bool,
    /// `signal_config.use_words_count` and `signal_config.words_count`: cap the message length.
    pub use_words_count: bool,
    pub words_count: i32,
    /// `signal_config.use_lower_price_words` and `signal_config.x_lower_price`: the "wait for a
    /// dip" filter and the offset it buys at.
    pub use_lower_price_words: bool,
    pub x_lower_price: i32,
    /// `signal_config.x_found_price`: the offset applied to a price read out of the message.
    pub x_found_price: i32,
    /// `signal_config.buy_if_price_found`: buy only when the message carries a price.
    pub buy_if_price_found: bool,
    /// `signal_config.use_price` and `signal_config.use_stops`: take the buy price, and the stops
    /// and take-profit, from the message.
    pub use_price: bool,
    pub use_stops: bool,
    /// `signal_config.only_1_token`: buy only when the message names exactly one token.
    pub only_1_token: bool,
    /// `signal_config.use_token_tags`, `signal_config.tokens_no_tags`, `signal_config.token_links`
    /// and `signal_config.special_formats`: how a ticker may be recognised.
    pub use_token_tags: bool,
    pub tokens_no_tags: bool,
    pub token_links: bool,
    pub special_formats: bool,
    /// `trading.auto_cancel_lower_buy`: minutes after which a buy left below the market is
    /// cancelled. On Moonbot's page it sits under the dip filter, which is why it is here rather
    /// than with the other `trading` fields.
    pub auto_cancel_lower_buy: i32,
}

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
/// drawn on an order, `visual` for chart and order-book appearance, `ui` for the two main-window
/// switches, and one flag of `signals` for the connectivity alert. Field names follow the WIRE, not Moonbot's caption, like every other block here — so
/// the button Moonbot calls "MoonBonus" is [`Self::hide_cashback_button`], which is what the
/// section calls it.
///
/// Moonbot's page has about twice this many controls. The rest are drawn disabled by the expert
/// window because the safe-share snapshot does not carry them at all — its own sound pickers, its
/// window style, the report form — or because which wire field backs them could not be established
/// from the section's own documentation, and a mirrored control wired to the wrong field is worse
/// than one that plainly does nothing.
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
    /// `visual.book_cumulative_opacity` and `visual.book_orders_opacity`, in whole per cent, and
    /// `visual.book_orders_width` in pixels: the order-book zone fill, its order levels, and how
    /// wide a level is drawn.
    pub book_cumulative_opacity: i32,
    pub book_orders_opacity: i32,
    pub book_orders_width: i32,
    /// `signals.play_signal_sound`: play a sound on NETWORK problems — a disconnect or high
    /// latency, throttled by the core to once every five seconds.
    ///
    /// Named after the wire like everything else here, and the wire's own doc warns that the name
    /// is historical: it is a connectivity alert, not a signal sound. It sits in THIS block rather
    /// than in [`SignalsSettings`] beside the wire fields it neighbours, because an area is a PAGE:
    /// Moonbot draws this switch on the Interface page and the compact popup draws it nowhere, so
    /// leaving it in the signals block would have let that popup's OK write its own frozen copy of
    /// a control it never showed.
    pub play_signal_sound: bool,
    /// `ui.confirm_close`: ask before closing Moonbot.
    pub confirm_close: bool,
    /// `ui.hide_demo_button`.
    pub hide_demo_button: bool,
}

/// Hand-written for the same reason [`ManualSettings`]'s is: the two spreads are `f64` read off the
/// wire, and a core holding a non-finite one must still compare equal to ITSELF. Under a derived
/// `PartialEq` it would not — IEEE says `NaN != NaN` — and
/// `feed::live::shared_config::edit_satisfied` would then be permanently false for that core, so
/// every OK on it would burn all three attempts and give up.
impl PartialEq for InterfaceSettings {
    fn eq(&self, other: &Self) -> bool {
        self.pending_orders_spread
            .total_cmp(&other.pending_orders_spread)
            .is_eq()
            && self
                .pending_orders_spread_h_delta
                .total_cmp(&other.pending_orders_spread_h_delta)
                .is_eq()
            && self.buy_on_enter == other.buy_on_enter
            && self.dbl_click_panic_sell == other.dbl_click_panic_sell
            && self.chart_split_zones == other.chart_split_zones
            && self.draw_stop == other.draw_stop
            && self.hide_forum_label == other.hide_forum_label
            && self.scrolling_charts == other.scrolling_charts
            && self.startup_load_charts == other.startup_load_charts
            && self.hide_right_chart_panel == other.hide_right_chart_panel
            && self.left_chart_info == other.left_chart_info
            && self.show_iceberg == other.show_iceberg
            && self.show_orders_captions == other.show_orders_captions
            && self.orders_captions_lower == other.orders_captions_lower
            && self.hide_pnl == other.hide_pnl
            && self.hide_buy_button == other.hide_buy_button
            && self.hide_cashback_button == other.hide_cashback_button
            && self.remember_chart_buttons == other.remember_chart_buttons
            && self.scale_tool == other.scale_tool
            && self.play_signal_sound == other.play_signal_sound
            && self.confirm_close == other.confirm_close
            && self.hide_demo_button == other.hide_demo_button
            && self.icon_selection == other.icon_selection
            && self.price_line_width == other.price_line_width
            && self.panic_sell_opacity == other.panic_sell_opacity
            && self.book_cumulative_opacity == other.book_cumulative_opacity
            && self.book_orders_opacity == other.book_orders_opacity
            && self.book_orders_width == other.book_orders_width
    }
}

/// Automatic leverage and margin management from `trading.auto_manage_lev`.
///
/// Written through the safe-share channel rather than the `LevManage` command: the core never sends
/// a `LevManage` snapshot on its own and the protocol has no request for one, so an edit built on
/// that snapshot had nothing to start from and was dropped before it reached the wire.
#[derive(Debug, Clone, PartialEq)]
pub struct LeverageSettings {
    pub auto_max_order: bool,
    pub auto_lev_up: bool,
    pub auto_isolated: bool,
    pub auto_cross: bool,
    pub tlg_report: bool,
    /// Whether [`Self::fix_lev`] is applied as a fixed target leverage.
    pub auto_fix_lev: bool,
    pub fix_lev: i32,
    /// `trading.auto_lev_control`: Moonbot's free-form leverage control expression.
    ///
    /// Carried so a round trip cannot drop it; the terminal renders no editor for it yet.
    pub lev_control: String,
}

/// Manual-strategy quick-button visibility and hotkeys, from moonproto `ManualStratsConfig`.
#[derive(Debug, Clone, PartialEq)]
pub struct CoreStratButtons {
    /// Whether the core shows its manual-strategy quick-buttons at all.
    pub use_buttons: bool,
    /// Visibility of each of the 10 button slots.
    pub show_button: [bool; 10],
    /// Hotkey assignment for each of the 10 button slots, as a raw Delphi `TShortCut`.
    pub hot_keys: [u16; 10],
}

/// One platform-level hotkey action the core assigns a single key to, decoupled from moonproto so
/// this crate never carries a pre-built localized label: `moon-core` cannot localize
/// (`rust_i18n::i18n!` is declared once in `moon-ui-gpui/src/main.rs`), so a hotkey action reaches
/// the UI as this enum and is captioned there, the same discipline [`crate::feed::ConnFaultKind`]
/// follows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoreHotkeyAction {
    CancelBuy,
    PanicSell,
    JoinSells,
    SwitchCharts,
    ReloadBook,
    NewLong,
    NewShort,
    SplitOrder,
    ShiftBuyUp,
    ShiftBuyDown,
    ShiftSellUp,
    ShiftSellDown,
    MakeShot,
    MakeShotBot,
    ReloadChart,
    ScalePlus,
    ScaleMinus,
    SellPlus,
    SellMinus,
    SpyMode,
    ShowCharts,
    SplitOrderX,
    SwitchFigure,
    FitSells,
    PanicSellOne,
    CancelAllBuys,
    Broadcast,
}

/// Number of single-key ([`CoreHotkeyAction`]) hotkey slots on [`CoreHotkeyLayout::named`].
pub const CORE_HOTKEY_ACTION_COUNT: usize = 27;

/// Core keyboard-shortcut layout from moonproto `HotkeysConfig`, decoupled from moonproto.
///
/// Raw values are the wire `TShortCut` (`u16`, low byte VK code, high byte Delphi shift mask);
/// decoding them into a `gpui::Keystroke` string is a later phase's job, not this projection's —
/// this type only carries the numbers through so a later phase can decode, preview, and diff them.
#[derive(Debug, Clone, PartialEq)]
pub struct CoreHotkeyLayout {
    /// Buy order-size preset hotkeys, 6 slots, mirroring [`ManualSettings::order_sizes`].
    pub order_size: [u16; 6],
    /// Sell-price preset hotkeys, 6 slots.
    pub sell_preset: [u16; 6],
    /// Every other single-key hotkey the core assigns, keyed by action.
    pub named: [(CoreHotkeyAction, u16); CORE_HOTKEY_ACTION_COUNT],
}

/// Core-owned manual-trading configuration projected from moonproto `SharedConfig`, decoupled from
/// moonproto: this is the terminal-owned, comparable shape the manual-trading feature reads and
/// diffs against, since `moonproto::SharedConfig` itself has no `PartialEq`.
#[derive(Debug, Clone)]
pub struct ManualSettings {
    /// Buy order-size presets (6 slots, in quote currency) from `ui.hotkeys_config.o_size`.
    ///
    /// A non-finite or negative entry is carried through AS-IS: this is a read of a core-owned
    /// value, and repairing it here would make the terminal disagree with the core's own screen.
    pub order_sizes: [f64; 6],
    /// Selected buy-size preset slot, 0-based, from `ui.hotkeys_config.b_num` (1-based on the
    /// wire, clamped into `0..=5` here so a corrupt or unfilled config cannot index out of bounds).
    pub order_size_sel: usize,
    /// User-defined manual-strategy names, slots 1..10, from `trading.manual_strats_names`.
    pub strat_names: [String; 10],
    /// Manual-strategy button visibility and hotkeys from `trading.manual_strats_config`.
    pub strat_buttons: CoreStratButtons,
    /// Platform hotkey layout from `ui.hotkeys_config`.
    pub core_hotkeys: CoreHotkeyLayout,
    /// Whether the core ignores a manual strategy's own sell price in favor of global settings,
    /// from `trading.ignore_strat_sell_price`.
    pub ignore_strat_sell_price: bool,
    /// Whether take-profit calculations include leverage, from `trading.use_lev_for_take`.
    pub use_lev_for_take: bool,
}

/// Hand-written: a core holding one non-finite `order_sizes` preset must still compare equal to
/// itself. A `derive`d `PartialEq` uses IEEE `f64` equality, where `NaN != NaN`, so
/// `feed::live::shared_config::edit_satisfied` would then be PERMANENTLY false for that core and
/// every gear-popup OK on it would burn all three `MAX_ATTEMPTS` and hit the give-up log — the
/// same reason `session/store.rs`'s compare-then-bump and `shell/core_settings/draft.rs`'s
/// `draft == seed` need this on the type rather than only at one call site. This is therefore an
/// equality-of-snapshots test, not an IEEE numeric comparison: `total_cmp` orders `NaN` as equal to
/// `NaN` (and total-orders signed zeros and other IEEE edge cases), which is exactly "the same bytes
/// came back" rather than "the same real number".
impl PartialEq for ManualSettings {
    fn eq(&self, other: &Self) -> bool {
        self.order_sizes
            .iter()
            .zip(other.order_sizes.iter())
            .all(|(a, b)| a.total_cmp(b).is_eq())
            && self.order_size_sel == other.order_size_sel
            && self.strat_names == other.strat_names
            && self.strat_buttons == other.strat_buttons
            && self.core_hotkeys == other.core_hotkeys
            && self.ignore_strat_sell_price == other.ignore_strat_sell_price
            && self.use_lev_for_take == other.use_lev_for_take
    }
}

/// Trust classification for a core's manual-config projection, mirroring
/// [`crate::session::store::BalanceState`] (same `has_value()`/`is_current()`/`code()` contracts,
/// same reason for `code()`) but without an `Unpriced` arm: a shared config carries no derived
/// valuation that pricing could invalidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoreConfigState {
    /// A projection exists, the connection is ready, and no stale marker remains.
    Live,
    /// A projection exists but the connection is not ready or became stale since the last one.
    Stale,
    /// No projection has ever arrived for this core.
    Awaiting,
}

impl CoreConfigState {
    /// Whether there is a usable projection to render.
    pub fn has_value(self) -> bool {
        matches!(self, CoreConfigState::Live | CoreConfigState::Stale)
    }

    /// Whether the store classifies the projection as current enough to show without a stale
    /// marker.
    pub fn is_current(self) -> bool {
        matches!(self, CoreConfigState::Live)
    }

    /// Stable small integer for hashing this state into a render signature.
    ///
    /// Exists so consumers do not invent their own numbering: the exhaustive match keeps a new
    /// variant a compile error here rather than a silently unhashed state somewhere downstream.
    pub fn code(self) -> u64 {
        match self {
            CoreConfigState::Live => 1,
            CoreConfigState::Stale => 2,
            CoreConfigState::Awaiting => 3,
        }
    }
}

/// Coarse AREA of the projection that differs from what was requested. Names a surface, carries no
/// value — used when the difference cannot be pinned to an exact field, which is every area but
/// the manual money fields below.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoreConfigArea {
    AutoBuy,
    Telegram,
    AutoStart,
    BtcBlink,
    General,
    Interface,
    Leverage,
    Manual,
    Signals,
}

/// What a shared-config echo disagreed with the terminal about, restricted to the fields THIS edit
/// actually asked to change — never the whole projection; see `feed::live::shared_config`'s module
/// doc and [`crate::feed::live::FieldMask`]. `moon-core` cannot localize (`rust_i18n::i18n!` is
/// declared once in `moon-ui-gpui/src/main.rs`), so this reaches the UI as typed data and is
/// captioned there, the same discipline [`crate::feed::ConnFaultKind`] and [`CoreHotkeyAction`]
/// follow.
#[derive(Debug, Clone, PartialEq)]
pub enum CoreConfigRejection {
    /// One or more coarse sections still differ.
    Areas(Vec<CoreConfigArea>),
}

/// Phase of a core-config edit that has not yet reached a terminal outcome, mirroring
/// [`StrategyEditPhase`]'s shape: a resolution is a one-time fact carried by
/// [`CoreConfigEditEvent::Resolved`], never a phase a retained row sits in, so `Confirmed` is not a
/// variant here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoreConfigEditPhase {
    Pending,
    /// The core never reflected this edit after `MAX_ATTEMPTS` sends. Replaces goal A's
    /// `TimedOut`: upstream's transport retries on a monotonic echo timeout rather than owning its
    /// own wall clock, so the terminal state is exhausting the retry budget, not a bare timeout.
    GaveUp,
}

/// Terminal outcome of one echo comparison for a queued core-config edit.
#[derive(Debug, Clone, PartialEq)]
pub enum CoreConfigEditResult {
    /// The core's snapshot now matches everything requested.
    Confirmed,
    /// The echo still disagrees on fields this edit touched; the queue keeps retrying up to
    /// `MAX_ATTEMPTS` — this is NOT the terminal state.
    NotApplied(CoreConfigRejection),
    /// The retry budget is exhausted; the edit is dropped from the queue.
    GaveUp,
}

/// One core-config edit's retained state, for the toolbar and popup's per-cell notices.
///
/// Boxed everywhere it travels through [`crate::feed::FeedMsg`]: [`ManualSettings`] alone makes
/// [`CoreConfig`] the largest field among this crate's other message payloads, and embedding it
/// unboxed here would make it `FeedMsg`'s own largest arm.
#[derive(Debug, Clone, PartialEq)]
pub struct CoreConfigEditRow {
    pub phase: CoreConfigEditPhase,
    pub submitted_at_ms: i64,
    /// The projection this edit asked the core to hold.
    pub config: CoreConfig,
    /// The most recent rejection this edit received, if any. Retained across a retry's own
    /// `Submitted` event and cleared only by [`CoreConfigEditResult::Confirmed`] or a fresh user
    /// edit — never by a retry of the same edit; this is a store-arm rule, applied in
    /// `session::store`.
    pub mismatches: Option<CoreConfigRejection>,
}

/// Core-config edit lifecycle event, published alongside [`crate::feed::FeedMsg::CoreConfig`] so
/// the UI can show a submitted-but-unconfirmed edit and its eventual verdict.
#[derive(Debug, Clone)]
pub enum CoreConfigEditEvent {
    /// A queued edit (or coalesced batch) was just sent and is awaiting its echo.
    Submitted(Box<CoreConfigEditRow>),
    /// The most recently submitted edit reached one echo's verdict.
    Resolved(CoreConfigEditResult),
}

/// Report profit counters from moonproto `TProfitStateCommand`, shown as the "now" lines beside the
/// AutoStart loss caps.
///
/// These come from the core's report database, not from balances or an order stream, so they can
/// disagree with the header's session P&L by design.
///
/// PAIRING UNVERIFIED: the wire carries four scalars (`rep_total_profit`/`rep_total_trades` and
/// `rep_trades_total`/`rep_count_trades`) and names neither pair, so which one backs the trade
/// window and which the hourly one is read from their position in the Moonbot page, not from the
/// protocol. Only the two "now" captions and their Reset buttons depend on it.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ProfitState {
    /// Accumulated profit over the trade-window counter, in quote currency.
    pub total_profit: f64,
    /// Trades counted in the trade-window counter.
    pub total_trades: i32,
    /// Accumulated profit over the hourly counter, in quote currency.
    pub hourly_profit: f64,
    /// Trades counted in the hourly counter.
    pub hourly_trades: i32,
}

/// Minutes in one day; the wire encodes the work-time window as a fraction of this.
const MINUTES_PER_DAY: f64 = 1440.0;

/// Convert the wire's fraction-of-day into whole minutes since midnight.
///
/// An out-of-band value (a hand-edited config, an older core writing a sentinel) clamps into
/// `0..=1439` rather than wrapping, and a non-finite one reads as midnight: this feeds a time
/// control, where midnight is a defensible reading of nonsense while a wrapped `u16` is not.
pub fn day_fraction_to_minutes(fraction: f64) -> u16 {
    if !fraction.is_finite() {
        return 0;
    }
    let minutes = (fraction * MINUTES_PER_DAY).round();
    minutes.clamp(0.0, 1439.0) as u16
}

/// Convert whole minutes since midnight into the wire's fraction-of-day.
///
/// The inverse of [`day_fraction_to_minutes`] only up to one-minute precision, which is why an
/// unchanged window is never written back; see [`AutoStartSettings::work_time_from_min`].
pub fn minutes_to_day_fraction(minutes: u16) -> f64 {
    f64::from(minutes.min(1439)) / MINUTES_PER_DAY
}

#[cfg(test)]
mod tests;
