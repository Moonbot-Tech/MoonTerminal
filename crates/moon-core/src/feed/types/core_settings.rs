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
    /// Exit rules, iceberg and blacklist fields spread across `trading`.
    pub general: GeneralSettings,
    /// `trading.auto_manage_lev` and `trading.auto_lev_control`.
    pub leverage: LeverageSettings,
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
