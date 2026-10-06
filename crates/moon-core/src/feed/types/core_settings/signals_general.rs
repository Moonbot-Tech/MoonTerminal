//! Signal, automatic-start and general trading settings.

/// Moonbot's two price-approach alerts: a sound when the last price comes within N per cent of an
/// order's sell price, and the same for its buy price.
///
/// Field names follow the wire names in `moonproto::shared_config::SignalsSection`, like every
/// other block here, which is why the SELL alert's sound is spelled `signal_sound_2`: that is what
/// the section calls it. The pairing is read off the section's own field order, where each sound
/// sits with the alert flag and level it belongs to — `signal_sound_2` between `sell_alert_level`
/// and `play_sell_alert`, `buy_signal_sound` beside `play_buy_alert` and `buy_alert_level`. The
/// wire's own doc for `signal_sound_2` says only "the second alert tier", so it names neither
/// alert. Two arguments settle it together, and neither reaches all three fields alone:
/// `buy_signal_sound` names its own half, and Moonbot draws exactly two such rows ("Звук если до
/// цены продажи меньше N%" and "…до цены покупки…"); while the section's field ORDER is what
/// separates `signal_sound_2` from the plain `signal_sound`, which sits fifteen fields earlier —
/// each of the two alert sounds is grouped with the flag and level it belongs to, and that one is
/// not. A core whose two rows come back swapped in the popup is the
/// symptom, and the fix is to swap them here.
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
#[derive(Debug, Clone, Copy)]
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

/// Hand-written for the reason [`GeneralSettings`]'s is, and this area needs it most: EIGHT of its
/// fields come off the wire as `f64`, and one non-finite among them makes the area never equal
/// itself — so `feed::live::shared_config` can neither confirm a write naming it nor recognise one
/// as already satisfied, and every such OK burns its whole retry budget.
impl PartialEq for AutoStartSettings {
    fn eq(&self, other: &Self) -> bool {
        let Self {
            auto_start,
            auto_detect_on,
            strategies_on,
            remember_state,
            auto_update,
            dont_wait_sells,
            work_time,
            work_time_from_min,
            work_time_to_min,
            auto_stop_if_loss,
            auto_stop_loss,
            stop_trades,
            sell_if_loss,
            auto_stop_if_loss_hours,
            auto_stop_hours_val,
            stop_hours,
            stop_hours_trades,
            ignore_emulator,
            reset_session,
            rs_hours,
            max_session_cap,
            panic_btc,
            panic_btc_delta,
            panic_btc_delta_up,
            panic_market,
            panic_market_delta,
            restart_on_market,
            btc_higher_then,
            btc_lower_then,
            market_higher_then,
            auto_stop_on_errors,
            errors_level,
            sell_all_on_errors,
            restart_after_err,
            restart_err_time,
            auto_stop_on_ping,
            ping_level,
            sell_all_on_ping,
            restart_after_ping,
            restart_ping_time,
        } = self;
        *auto_start == other.auto_start
            && *auto_detect_on == other.auto_detect_on
            && *strategies_on == other.strategies_on
            && *remember_state == other.remember_state
            && *auto_update == other.auto_update
            && *dont_wait_sells == other.dont_wait_sells
            && *work_time == other.work_time
            && *work_time_from_min == other.work_time_from_min
            && *work_time_to_min == other.work_time_to_min
            && *auto_stop_if_loss == other.auto_stop_if_loss
            && auto_stop_loss.total_cmp(&other.auto_stop_loss).is_eq()
            && *stop_trades == other.stop_trades
            && *sell_if_loss == other.sell_if_loss
            && *auto_stop_if_loss_hours == other.auto_stop_if_loss_hours
            && auto_stop_hours_val
                .total_cmp(&other.auto_stop_hours_val)
                .is_eq()
            && *stop_hours == other.stop_hours
            && *stop_hours_trades == other.stop_hours_trades
            && *ignore_emulator == other.ignore_emulator
            && *reset_session == other.reset_session
            && *rs_hours == other.rs_hours
            && *max_session_cap == other.max_session_cap
            && *panic_btc == other.panic_btc
            && panic_btc_delta.total_cmp(&other.panic_btc_delta).is_eq()
            && panic_btc_delta_up
                .total_cmp(&other.panic_btc_delta_up)
                .is_eq()
            && *panic_market == other.panic_market
            && panic_market_delta
                .total_cmp(&other.panic_market_delta)
                .is_eq()
            && *restart_on_market == other.restart_on_market
            && btc_higher_then.total_cmp(&other.btc_higher_then).is_eq()
            && btc_lower_then.total_cmp(&other.btc_lower_then).is_eq()
            && market_higher_then
                .total_cmp(&other.market_higher_then)
                .is_eq()
            && *auto_stop_on_errors == other.auto_stop_on_errors
            && *errors_level == other.errors_level
            && *sell_all_on_errors == other.sell_all_on_errors
            && *restart_after_err == other.restart_after_err
            && *restart_err_time == other.restart_err_time
            && *auto_stop_on_ping == other.auto_stop_on_ping
            && *ping_level == other.ping_level
            && *sell_all_on_ping == other.sell_all_on_ping
            && *restart_after_ping == other.restart_after_ping
            && *restart_ping_time == other.restart_ping_time
    }
}

/// BTC price blink and alarm settings from `visual.blink_config`.
///
/// Drawn at the bottom of the Moonbot AutoStart tab even though it lives in the visual section, and
/// it is the ONLY channel that carries these two controls — the compact settings snapshot has no
/// counterpart for them.
#[derive(Debug, Clone, Copy)]
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

/// Hand-written for the reason [`AutoStartSettings`]'s is: both BTC deltas are wire `f64`.
impl PartialEq for BtcBlinkSettings {
    fn eq(&self, other: &Self) -> bool {
        let Self {
            blink_btc,
            blink_btc_delta,
            blink_btc_delta_up,
            alarm_btc,
            alarm_type,
        } = self;
        blink_btc_delta.total_cmp(&other.blink_btc_delta).is_eq()
            && blink_btc_delta_up
                .total_cmp(&other.blink_btc_delta_up)
                .is_eq()
            && *blink_btc == other.blink_btc
            && *alarm_btc == other.alarm_btc
            && *alarm_type == other.alarm_type
    }
}

/// Exit rules and risk limits — the part of Moonbot's "Основные" page BOTH faces of the gear draw.
///
/// The stop, trailing and V-Stop rules carry their own enable flag here, unlike the compact
/// `ClientSettings` projection where a zero value has to stand in for "off": the safe-share section
/// keeps `trailing_stop` and `panic_if_vol_drop` beside their levels, which is what lets a disabled
/// rule remember the level it was disabled at.
///
/// The rest of that page is [`crate::feed::OrderRulesSettings`], a SEPARATE area for one reason: the compact
/// popup does not draw those rows, and a surface may write only what it drew.
#[derive(Debug, Clone)]
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

/// Hand-written for the reason [`crate::feed::SpecialSettings`]'s is: `take_profit_pct` comes off the wire as
/// `f64`, and a core holding a non-finite one must still compare equal to itself, or
/// `feed::live::shared_config::edit_satisfied` is false for it forever and every OK on that core
/// burns its whole retry budget.
impl PartialEq for GeneralSettings {
    fn eq(&self, other: &Self) -> bool {
        // Destructured rather than compared field by field through `self.`: a field added to the
        // struct then fails to COMPILE here instead of being silently left out of equality, which
        // would make `edit_satisfied` report an edit landed that never did.
        let Self {
            take_profit_on,
            take_profit_pct,
            trailing_on,
            trailing_pct,
            vstop_on,
            vol_drop_level,
            buy_iceberg,
            sell_iceberg,
            blacklist_on,
            blacklist_text,
            exclude_blacklisted_from_deltas,
        } = self;
        take_profit_pct.total_cmp(&other.take_profit_pct).is_eq()
            && trailing_pct.total_cmp(&other.trailing_pct).is_eq()
            && *take_profit_on == other.take_profit_on
            && *trailing_on == other.trailing_on
            && *vstop_on == other.vstop_on
            && *vol_drop_level == other.vol_drop_level
            && *buy_iceberg == other.buy_iceberg
            && *sell_iceberg == other.sell_iceberg
            && *blacklist_on == other.blacklist_on
            && *blacklist_text == other.blacklist_text
            && *exclude_blacklisted_from_deltas == other.exclude_blacklisted_from_deltas
    }
}
