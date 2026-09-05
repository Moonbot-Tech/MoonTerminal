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
//!
//! [`CoreConfigEditRow`] breaks that layering in ONE place: it borrows `FieldMask` from
//! `feed::live::shared_config`. `FieldMask` is a set of AREA flags over the types below and carries
//! no transport of its own, so it belongs HERE beside [`CoreConfigArea`] rather than in the
//! sequencer that happens to have introduced it. Moving it is the fix; until then this import is a
//! stated exception rather than a precedent.

use crate::feed::FieldMask;

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
    /// Exit rules, iceberg and blacklist fields spread across `trading` — the part of Moonbot's
    /// "Основные" page BOTH gear faces draw.
    pub general: GeneralSettings,
    /// The rest of that page, which only the expert window draws.
    pub order_rules: OrderRulesSettings,
    /// `trading.auto_manage_lev` and `trading.auto_lev_control`.
    pub leverage: LeverageSettings,
    /// Moonbot's own window and chart appearance, spread across `trading`, `visual` and `ui`.
    pub interface: InterfaceSettings,
    /// Moonbot's autobuy page: the signal sources and the message filter.
    pub auto_buy: AutoBuySettings,
    /// Moonbot's Telegram page: the signal channels and the rules over them.
    pub telegram: TelegramSettings,
    /// Moonbot's "Специальные" page: the engine switches, logging and screenshot rules.
    pub special: SpecialSettings,
    /// Moonbot's Hotkeys page: the mouse gestures that place and move orders.
    pub gestures: GestureSettings,
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
/// The rest of that page is [`OrderRulesSettings`], a SEPARATE area for one reason: the compact
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

/// Hand-written for the reason [`SpecialSettings`]'s is: `take_profit_pct` comes off the wire as
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

/// The rows of Moonbot's "Основные" page the compact gear popup does not draw.
///
/// A SEPARATE area from [`GeneralSettings`] although it is the same Moonbot page, and the reason is
/// the rule an area exists to serve: a surface may write only what it DREW. The compact popup draws
/// the exits and the blacklist and nothing else, so a mask naming its page must not carry these
/// seven — its OK would stamp them back from a frozen draft over whatever Moonbot changed
/// meanwhile. The expert window draws the whole page and names both areas.
///
/// So the boundary here is which surface draws a row, not what the row means, which is why two of
/// the seven are not order rules at all: the fresh-coin hold sits in Moonbot's risk frame and the
/// startup analysis across the top of the page. It is the one place this module's "an area is a
/// PAGE" model bends, and it bends towards the rule the model is FOR.
#[derive(Debug, Clone)]
pub struct OrderRulesSettings {
    /// `trading.trailing_float`: how much the trailing distance widens per per cent of price move.
    ///
    /// Moonbot's row reads "Добавить к трейлингу +X% за каждый % цены" and the wire calls the field
    /// the "trailing-stop floating percentage". The section's other trailing number,
    /// `trailing_drop`, is the distance itself and is already [`GeneralSettings::trailing_pct`], so
    /// this is the only candidate left for a row that ADDS to it.
    pub trailing_float: f64,
    /// `trading.auto_sell_partial`: per cent of a buy that has to be filled before the sell goes
    /// out. The wire's own default is 100, which it glosses as "wait for the whole fill" — a
    /// boundary Moonbot's caption ("продавать, если куплена часть > X%") words as a strict
    /// inequality. The caption is ported as Moonbot writes it; the wire's gloss is recorded here.
    pub auto_sell_partial: i32,
    /// `trading.auto_cancel_buy_order`: how long an unfilled buy is left standing.
    ///
    /// A COMPOSITE scale, and the one field on this page whose number is not what it reads as: the
    /// wire documents values below 30 as seconds and 30 and above as `value - 29` MINUTES, so 29 is
    /// twenty-nine seconds and 30 is one minute. The page prints the unit; nothing converts the
    /// value, which travels exactly as the core holds it. The scale can therefore express no delay
    /// between 30 and 59 seconds — that is the wire's own gap, not the control's.
    ///
    /// Its neighbour `trading.auto_cancel_lower_buy` is a second auto-cancel, and this is the one
    /// Moonbot's plain "Авто отмена покупки" row means: the neighbour is qualified ("a buy order
    /// placed BELOW the current price") and is counted in plain minutes, while this one carries the
    /// composite scale Moonbot's own control shows.
    pub auto_cancel_buy_order: i32,
    /// `trading.cancel_buy_on_sell_fill`: drop the standing buy once a sell of the same position
    /// fills.
    ///
    /// (Decoding of the field above lives on [`Self::auto_cancel_delay`].)
    pub cancel_buy_on_sell_fill: bool,
    /// `trading.dont_buy_new_coins`: minutes a freshly listed coin is left alone.
    pub dont_buy_new_coins: i32,
    /// `trading.deltas_by_trades`: compute the deltas from the trade stream rather than the book.
    ///
    /// Two halves, like [`GeneralSettings::exclude_blacklisted_from_deltas`]: moonproto applies its
    /// own copy to the terminal's retained analytics without asking the core
    /// (`streams().set_deltas_by_trades`), so committing this field must drive both or the two
    /// disagree until a restart.
    ///
    /// It is also a TAIL field of the `trading` section: a core built before it existed sends a
    /// shorter block and moonproto fills the default, so on such a core TICKING this row makes the
    /// echo unmatchable and the write exhausts its retry budget. Leaving it untouched costs
    /// nothing — the value sent is then the value read.
    ///
    /// The client half is applied locally either way, the same asymmetry
    /// [`GeneralSettings::exclude_blacklisted_from_deltas`] has and for the same reason: it is
    /// issued once the page has gone out, so the two halves cannot diverge the other way round.
    pub deltas_by_trades: bool,
    /// `signals.load_deep_history`: analyse every market's candle history when the core starts.
    ///
    /// The one field of this area outside `trading`. Moonbot draws this switch across the top of
    /// its "Основные" page, above the two columns.
    pub analyze_on_start: bool,
}

/// One row of Moonbot's Move grid — the four the "same hotkeys" switch governs.
///
/// Named rather than addressed by field so the mirror rule below can be written once instead of
/// once per row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MoveRow {
    /// "Move Order", the primary row.
    OpenPrimary,
    /// "Move TP", the primary row.
    TpPrimary,
    /// "Move Order" under "Дополнительные команды".
    OpenSecondary,
    /// "Move TP" under "Дополнительные команды".
    TpSecondary,
}

impl GestureSettings {
    /// The gesture one Move row actually fires on.
    ///
    /// With `same_hotkeys_for_move` set the short side follows the long one, so the SHORT field is
    /// not what the core acts on and must not be what a surface shows: a core can hold the flag
    /// with a divergent short value, and printing it would state a binding that never fires. The
    /// same resolution `config::HotkeysConfig::move_gestures` performs for the terminal's own copy.
    pub fn move_gesture(&self, row: MoveRow, short: bool) -> u8 {
        match (row, short && !self.same_hotkeys_for_move) {
            (MoveRow::OpenPrimary, false) => self.buy_move_click,
            (MoveRow::OpenPrimary, true) => self.short_buy_move_click,
            (MoveRow::TpPrimary, false) => self.sell_move_click,
            (MoveRow::TpPrimary, true) => self.short_sell_move_click,
            (MoveRow::OpenSecondary, false) => self.buy_move_click_2,
            (MoveRow::OpenSecondary, true) => self.short_buy_move_click_2,
            (MoveRow::TpSecondary, false) => self.sell_move_click_2,
            (MoveRow::TpSecondary, true) => self.short_sell_move_click_2,
        }
    }

    /// Set one Move row's gesture, carrying the mirror the flag demands.
    ///
    /// Writing the long side while `same_hotkeys_for_move` is set writes the short side too, which
    /// is what Moonbot's dialog does and what `moon-ui-gpui`'s own hotkeys settings tab does with
    /// the terminal's copy. It REPAIRS a divergence rather than preventing one: a core can already
    /// hold the flag over stale short values, and only an actual edit rewrites them.
    ///
    /// Writing the SHORT side while that flag is set does nothing, deliberately: [`Self::move_gesture`]
    /// could never return such a value, and a setter whose write its own reader cannot see is a trap
    /// for the next caller. Surfaces disable that control instead — this is the guard behind them.
    pub fn set_move_gesture(&mut self, row: MoveRow, short: bool, value: u8) {
        if short && self.same_hotkeys_for_move {
            return;
        }
        let mirror = !short && self.same_hotkeys_for_move;
        match row {
            MoveRow::OpenPrimary => {
                if short {
                    self.short_buy_move_click = value;
                } else {
                    self.buy_move_click = value;
                }
                if mirror {
                    self.short_buy_move_click = value;
                }
            }
            MoveRow::TpPrimary => {
                if short {
                    self.short_sell_move_click = value;
                } else {
                    self.sell_move_click = value;
                }
                if mirror {
                    self.short_sell_move_click = value;
                }
            }
            MoveRow::OpenSecondary => {
                if short {
                    self.short_buy_move_click_2 = value;
                } else {
                    self.buy_move_click_2 = value;
                }
                if mirror {
                    self.short_buy_move_click_2 = value;
                }
            }
            MoveRow::TpSecondary => {
                if short {
                    self.short_sell_move_click_2 = value;
                } else {
                    self.sell_move_click_2 = value;
                }
                if mirror {
                    self.short_sell_move_click_2 = value;
                }
            }
        }
    }

    /// Turn the "one set for Long and Short" switch, copying the long gestures onto the short ones
    /// when it goes on — the same thing Moonbot's own checkbox does.
    pub fn set_same_hotkeys(&mut self, on: bool) {
        self.same_hotkeys_for_move = on;
        if on {
            self.short_buy_move_click = self.buy_move_click;
            self.short_sell_move_click = self.sell_move_click;
            self.short_buy_move_click_2 = self.buy_move_click_2;
            self.short_sell_move_click_2 = self.sell_move_click_2;
        }
    }
}

impl OrderRulesSettings {
    /// The auto-cancel delay decoded from its composite scale, as `(amount, in minutes)`.
    ///
    /// Beside the field rather than in the page that prints it: the scale is a property of the wire
    /// value, and a second surface showing this number would otherwise reinvent it.
    pub fn auto_cancel_delay(&self) -> (i32, bool) {
        if self.auto_cancel_buy_order < 30 {
            (self.auto_cancel_buy_order, false)
        } else {
            (self.auto_cancel_buy_order - 29, true)
        }
    }
}

/// Hand-written for the reason [`GeneralSettings`]'s is: `trailing_float` comes off the wire as
/// `f64`.
impl PartialEq for OrderRulesSettings {
    fn eq(&self, other: &Self) -> bool {
        // Destructured for the reason [`GeneralSettings`]'s is.
        let Self {
            trailing_float,
            auto_sell_partial,
            auto_cancel_buy_order,
            cancel_buy_on_sell_fill,
            dont_buy_new_coins,
            deltas_by_trades,
            analyze_on_start,
        } = self;
        trailing_float.total_cmp(&other.trailing_float).is_eq()
            && *auto_sell_partial == other.auto_sell_partial
            && *auto_cancel_buy_order == other.auto_cancel_buy_order
            && *cancel_buy_on_sell_fill == other.cancel_buy_on_sell_fill
            && *dont_buy_new_coins == other.dont_buy_new_coins
            && *deltas_by_trades == other.deltas_by_trades
            && *analyze_on_start == other.analyze_on_start
    }
}

/// Moonbot's Hotkeys page, "Orders Controls" tab: which mouse gesture places, moves and repositions
/// an order, and how a bulk move lays out what it addresses.
///
/// An area is a PAGE — see [`InterfaceSettings`] — but this one covers a BLOCK of its page rather
/// than the whole of it. The rest of the Hotkeys page mirrors [`ManualSettings`], which no OK from
/// a settings surface may write (see `feed::live::shared_config`'s module doc), so those rows stay
/// read-only while these are live.
///
/// Every field is a raw Delphi ordinal, and the terminal already carries both lists: a gesture is
/// `config::MouseGestureBinding::ALL` indexed by the byte — moonproto's own defaults annotate
/// `buy_set_click: 1` as `Dbl_Click` and `sell_move_click: 2` as `CTRL_Click`, which is that list
/// at 1 and 2 — and a move kind is `config::MoveKind::ALL` indexed the same way, against
/// moonproto's `ReplaceMultiKind` (`TReplaceMultiKind`, Vars.pas:37), whose constants run None=0,
/// Shift=1, TopVol=2, LowVol=3, TopProfit=4, All=5, LastSet=6, LastMoved=7.
///
/// The ordinals are kept as bytes rather than decoded here for the reason the rest of this module
/// keeps wire shapes: a core holding a value this build has no name for must survive the round trip
/// untouched, and an enum would have to invent a variant for it or drop it.
///
/// `trading` carries a SECOND, single-order set of the same idea — `order_set_click`,
/// `order_replace_click_buy`, `order_replace_click_sell` — which this block does not write. The
/// evidence that Moonbot's page edits the multi-order set is structural: the page has a long
/// column, a short column and a whole second row of "additional commands", and only
/// `multi_orders` carries short and secondary twins at all. The legacy trio has none, so it cannot
/// be what those columns write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GestureSettings {
    /// `trading.multi_orders.buy_set_click`: places a long at the clicked price.
    pub buy_set_click: u8,
    /// `trading.multi_orders.short_set_click`: places a short at the clicked price.
    pub short_set_click: u8,
    /// `trading.pending_order_set_click`: places a pending long.
    ///
    /// The one field of this block outside the `multi_orders` sub-record, and not by choice:
    /// `multi_orders` carries `pending_short_set_click` and no long counterpart, so Moonbot's own
    /// pair of rows straddles the two records. The asymmetry is the wire's.
    pub pending_order_set_click: u8,
    /// `trading.multi_orders.pending_short_set_click`: places a pending short.
    pub pending_short_set_click: u8,
    /// `trading.multi_orders.same_hotkeys_for_move`: the short columns follow the long ones.
    pub same_hotkeys_for_move: bool,
    /// The primary Move Order row, as `trading.multi_orders.buy_move_click` /
    /// `short_buy_move_click` / `replace_buy_kind`: its long gesture, its short gesture, and which
    /// orders the pair addresses. Read the pair of gestures through [`Self::move_gesture`] rather
    /// than directly — the short one is not always what fires.
    pub buy_move_click: u8,
    /// Short half of the primary Move Order row.
    pub short_buy_move_click: u8,
    /// Which orders the primary Move Order row addresses, and how the core lays them out.
    pub replace_buy_kind: u8,
    /// Long half of the primary Move TP row.
    pub sell_move_click: u8,
    /// Short half of the primary Move TP row.
    pub short_sell_move_click: u8,
    /// Which orders the primary Move TP row addresses.
    pub replace_sell_kind: u8,
    /// Long half of the secondary Move Order row — Moonbot's "Дополнительные команды".
    pub buy_move_click_2: u8,
    /// Short half of the secondary Move Order row.
    pub short_buy_move_click_2: u8,
    /// Which orders the secondary Move Order row addresses.
    pub replace_buy_kind_2: u8,
    /// Long half of the secondary Move TP row.
    pub sell_move_click_2: u8,
    /// Short half of the secondary Move TP row.
    pub short_sell_move_click_2: u8,
    /// Which orders the secondary Move TP row addresses.
    pub replace_sell_kind_2: u8,
}

/// Moonbot's "Специальные" page: the engine's own switches, its logging and its screenshot rules.
///
/// An area is a PAGE — see [`InterfaceSettings`]. This one is `trading` with its
/// `send_shots_config` and `orders_control` sub-records.
///
/// Moonbot's page has about twice this many controls, and the expert window draws all of them
/// disabled where they are not here. The reasons are stated on the page itself, row by row; the
/// ones that concern this block are: the Remote
/// block and the hang watchdog carry a bot token, a UDP password and a control VDS address, which
/// safe-share excludes outright; a few rows have no wire field at all; and the iceberg pair belongs
/// to [`GeneralSettings`], which the compact popup edits — one wire field belongs to one area.
#[derive(Debug, Clone)]
pub struct SpecialSettings {
    /// `trading.log_level` and `trading.auto_delete_logs`: how much is written, and for how long.
    pub log_level: i32,
    pub auto_delete_logs: i32,
    /// `trading.chart_clean_up_time`: minutes of inactivity after which a chart is dropped.
    pub chart_clean_up_time: i32,
    /// `trading.max_orders` and `trading.unlimited_orders`: the cap on open buys, and its removal.
    pub max_orders: i32,
    pub unlimited_orders: bool,
    /// `trading.random_price`: add a small random offset to an order's price.
    pub random_price: bool,
    /// `trading.correct_order_price`: snap an order price to the venue's tick.
    pub correct_order_price: bool,
    /// `trading.use_book_ticker`: take best bid/ask from the stream rather than by polling.
    pub use_book_ticker: bool,
    /// `trading.m_avg_use_vol_weight`: weight the moving average by volume.
    pub m_avg_use_vol_weight: bool,
    /// `trading.auto_buy_bnb`, `trading.auto_buy_bnb_level` and `trading.auto_buy_bnb_volume`: buy
    /// BNB for commissions when the balance falls below the level, and how much.
    pub auto_buy_bnb: bool,
    pub auto_buy_bnb_level: f64,
    pub auto_buy_bnb_volume: f64,
    /// `trading.auto_reduce_order`: shrink an order that exceeds the free balance.
    pub auto_reduce_order: bool,
    /// `trading.auto_close_zero_pos`: close a zero-quantity ghost position.
    pub auto_close_zero_pos: bool,
    /// `trading.auto_lower_lev`: drop the leverage when the venue refuses the level asked for.
    ///
    /// Moonbot calls the row "Auto Leverage"; this is the only unclaimed leverage flag in the
    /// section, the rest of them being the `auto_manage_lev` block [`LeverageSettings`] owns.
    pub auto_lower_lev: bool,
    /// `trading.use_websocket_api`: place orders over the socket rather than REST.
    pub use_websocket_api: bool,
    /// `trading.futures_rules`: Moonbot's "Quantitative Rules" — the futures position-mode checks.
    ///
    /// Its control is `cbFuturesRules`, which is the wire's own name for the field; caption and name
    /// disagree only in wording. Unlike `free_position_check`, which this page still draws dead, the
    /// caption carries no negation — ticked means on, and no direction is left to guess. That it can
    /// turn a live safety check off is what Moonbot's own checkbox does too, and mirroring that
    /// dialog is this window's whole contract.
    pub futures_rules: bool,
    /// `trading.iceberg_step`: the price step below which an order is placed as an iceberg, as a
    /// PER CENT. The wire's own default is 0.1, meaning a tenth of a per cent.
    ///
    /// The wire calls it "iceberg slice size as a fraction of total", and that is wrong. Moonbot
    /// formats the row itself as "Ставить Iceberg если шаг цены < %s%%" — a literal per cent sign
    /// after the value — so the number is a percentage of price, not a share of the order. The
    /// control's 0..1 range suits that reading as well, which is why it did not have to move.
    pub iceberg_step: f64,
    /// `trading.sell_x2_level`: the volume percentile above which the sell quantity doubles.
    pub sell_x2_level: i32,
    /// `trading.no_trades_markets_text`: tickers that generate no signals, one per line.
    pub no_trades_markets_text: String,
    /// `trading.orders_control.liq_control`: watch how near a position is to liquidation.
    pub liq_control: bool,
    /// `trading.orders_control.ignore_replacing_bug`: ignore the engine's "replacing" order state.
    pub ignore_replacing_bug: bool,
    /// `trading.orders_control.ignore_protection`: how far the order protection is bypassed.
    ///
    /// A LEVEL on the wire, where zero means the protection is on, but Moonbot draws one checkbox
    /// over it ("Turn Off Protection"). So this page reads a positive value as on and, when the box
    /// is ticked, supplies a level only if the core holds none — a level it already holds survives.
    /// Turning the box off does set zero, so the level is lost that way; Moonbot's own dialog can
    /// express no more than that either.
    pub ignore_protection: i32,
    /// `trading.orders_control.active`: watch this bot's ORDERS — Moonbot's "Следить за ордерами
    /// этого бота", the one switch in its worker-bot block.
    ///
    /// Its neighbour `orders_control.h_pos_control` ("hanging-position detection") is deliberately
    /// NOT here: no row on that page carries its caption, and binding one checkbox to two flags
    /// would turn a feature on and off that the trader never named.
    pub orders_control_active: bool,
    /// `trading.orders_control.h_pos_report` and `trading.orders_control.h_pos_auto_sell`: what the
    /// WATCHING bot does about a hanging position — report it, and sell it.
    pub h_pos_report: bool,
    pub h_pos_auto_sell: bool,
    /// `trading.h_pos_black_list_text`: coins the watcher leaves alone.
    ///
    /// One line, comma-separated. The wire names no separator for this field, unlike its siblings
    /// that say "one per line"; the evidence is Moonbot's own dialog, which draws it as a one-line
    /// box holding "BTC, ETH, BNB, …", and its documentation, which calls it a comma-separated
    /// blacklist. `trading.h_pos_black_list_add` beside it is a SECOND such list with no row on the
    /// page, so an empty box here does not mean the watcher skips nothing.
    pub h_pos_black_list_text: String,
    /// `trading.multi_commands`: accept batched commands.
    ///
    /// Moonbot's caption is "Мультистроковые команды" and its hint for that row says what it does:
    /// "Принимать несколько команд в одном сообщении в Телеграме". Several commands in ONE Telegram
    /// message — not the "batch order operations from a thin client" the wire's doc describes. The
    /// field is right, the wire's sentence is not.
    pub multi_commands: bool,
    /// `trading.send_shots_config.may_send` and the thresholds under it: when a trade's chart is
    /// posted to Telegram, and how that chart is scaled.
    ///
    /// Two of these carry a caption the wire words differently.
    ///
    /// `time_scale` is a PER CENT and the wire's "seconds of history" is wrong: Moonbot's own hint
    /// reads "Масштаб по оси времени от 100% до 400% означает, насколько график на скрине длиннее,
    /// чем заняла сама сделка" — a zoom factor, and one that states its own range. `price_scale`
    /// beside it is the same kind of number, and its hint confirms what the wire's default already
    /// implied: "Если 0 … применяется авто-масштабирование".
    ///
    /// `profit_session` is drawn as "или профит за час" while the wire calls it a session profit;
    /// the two coincide only when the session resets hourly. The binding is settled by POSITION
    /// rather than by either wording: Moonbot's group is three thresholds — "Если профит $ >",
    /// "или профит % >", "или профит за час $ >" — against this record's `profit_abs`,
    /// `profit_pers`, `profit_session`, in that order.
    pub send_shots: bool,
    pub profit_abs: i32,
    pub profit_pers: i32,
    pub profit_session: i32,
    pub send_negative: bool,
    pub send_public: bool,
    pub time_scale: i32,
    pub price_scale: i32,
}

/// Hand-written for the reason [`ManualSettings`]'s is: three of these come off the wire as `f64`,
/// and a core holding a non-finite one must still compare equal to itself, or
/// `feed::live::shared_config::edit_satisfied` is false for any mask naming this area, forever.
impl PartialEq for SpecialSettings {
    fn eq(&self, other: &Self) -> bool {
        // Destructured for the reason [`GeneralSettings`]'s is.
        let Self {
            log_level,
            auto_delete_logs,
            chart_clean_up_time,
            max_orders,
            unlimited_orders,
            random_price,
            correct_order_price,
            use_book_ticker,
            m_avg_use_vol_weight,
            auto_buy_bnb,
            auto_buy_bnb_level,
            auto_buy_bnb_volume,
            auto_reduce_order,
            auto_close_zero_pos,
            auto_lower_lev,
            use_websocket_api,
            futures_rules,
            iceberg_step,
            sell_x2_level,
            no_trades_markets_text,
            liq_control,
            ignore_replacing_bug,
            ignore_protection,
            orders_control_active,
            h_pos_report,
            h_pos_auto_sell,
            h_pos_black_list_text,
            multi_commands,
            send_shots,
            profit_abs,
            profit_pers,
            profit_session,
            send_negative,
            send_public,
            time_scale,
            price_scale,
        } = self;
        auto_buy_bnb_level
            .total_cmp(&other.auto_buy_bnb_level)
            .is_eq()
            && auto_buy_bnb_volume
                .total_cmp(&other.auto_buy_bnb_volume)
                .is_eq()
            && iceberg_step.total_cmp(&other.iceberg_step).is_eq()
            && *log_level == other.log_level
            && *auto_delete_logs == other.auto_delete_logs
            && *chart_clean_up_time == other.chart_clean_up_time
            && *max_orders == other.max_orders
            && *unlimited_orders == other.unlimited_orders
            && *random_price == other.random_price
            && *correct_order_price == other.correct_order_price
            && *use_book_ticker == other.use_book_ticker
            && *m_avg_use_vol_weight == other.m_avg_use_vol_weight
            && *auto_buy_bnb == other.auto_buy_bnb
            && *auto_reduce_order == other.auto_reduce_order
            && *auto_close_zero_pos == other.auto_close_zero_pos
            && *auto_lower_lev == other.auto_lower_lev
            && *use_websocket_api == other.use_websocket_api
            && *futures_rules == other.futures_rules
            && *sell_x2_level == other.sell_x2_level
            && *no_trades_markets_text == other.no_trades_markets_text
            && *liq_control == other.liq_control
            && *ignore_replacing_bug == other.ignore_replacing_bug
            && *ignore_protection == other.ignore_protection
            && *orders_control_active == other.orders_control_active
            && *h_pos_report == other.h_pos_report
            && *h_pos_auto_sell == other.h_pos_auto_sell
            && *h_pos_black_list_text == other.h_pos_black_list_text
            && *multi_commands == other.multi_commands
            && *send_shots == other.send_shots
            && *profit_abs == other.profit_abs
            && *profit_pers == other.profit_pers
            && *profit_session == other.profit_session
            && *send_negative == other.send_negative
            && *send_public == other.send_public
            && *time_scale == other.time_scale
            && *price_scale == other.price_scale
    }
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
/// `signals`, from its `signal_config` sub-record, and from two fields of `trading`, which is exactly
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
    /// Moonbot's "Захватывать буфер", and its own hint for that row says exactly this field's
    /// meaning: "бот будет искать монету в буфере, даже если не стоит автопокупка; в этом случае
    /// бот покажет монету но не купит". Which is the difference between this flag and
    /// [`Self::clipboard_auto_buy`] beside it.
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
    /// `trading.dont_buy_forward`: ignore a signal that was FORWARDED from another chat.
    ///
    /// The wire's own doc for this field says something else — "skip buying forward contracts /
    /// pre-market tokens" — and that doc is wrong. Moonbot carries its own log line for the flag,
    /// "Нашел монету в пересланном (forward) сообщении, не буду ее покупать!", which names the
    /// behaviour and the field in one sentence; its dialog draws the row beside "Не покупать
    /// ответное", which is [`Self::dont_buy_reply`]. Two message filters, one pair.
    ///
    /// The only oddity left is the wire's: this half lives in `trading` while its twin lives in
    /// `signals`.
    pub dont_buy_forward: bool,
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
    /// than in [`SignalsSettings`] beside the wire fields it neighbours, because an area is a PAGE:
    /// Moonbot draws this switch on the Interface page and the compact popup draws it nowhere, so
    /// leaving it in the signals block would have let that popup's OK write its own frozen copy of
    /// a control it never showed.
    pub play_signal_sound: bool,
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

/// Hand-written for the same reason [`ManualSettings`]'s is: the two spreads are `f64` read off the
/// wire, and a core holding a non-finite one must still compare equal to ITSELF. Under a derived
/// `PartialEq` it would not — IEEE says `NaN != NaN` — and
/// `feed::live::shared_config::edit_satisfied` would then be permanently false for that core, so
/// every OK on it would burn all three attempts and give up.
impl PartialEq for InterfaceSettings {
    fn eq(&self, other: &Self) -> bool {
        // Destructured for the reason [`GeneralSettings`]'s is.
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
        // Destructured for the reason [`GeneralSettings`]'s is. This was the last impl in the file
        // still comparing through `self.`, which is the one shape where a new field can go missing
        // without the compiler saying so.
        let Self {
            order_sizes,
            order_size_sel,
            strat_names,
            strat_buttons,
            core_hotkeys,
            ignore_strat_sell_price,
            use_lev_for_take,
        } = self;
        order_sizes
            .iter()
            .zip(other.order_sizes.iter())
            .all(|(a, b)| a.total_cmp(b).is_eq())
            && *order_size_sel == other.order_size_sel
            && *strat_names == other.strat_names
            && *strat_buttons == other.strat_buttons
            && *core_hotkeys == other.core_hotkeys
            && *ignore_strat_sell_price == other.ignore_strat_sell_price
            && *use_lev_for_take == other.use_lev_for_take
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
    Special,
    Telegram,
    AutoStart,
    BtcBlink,
    General,
    /// The Hotkeys page's mouse-gesture block — see [`GestureSettings`].
    Gestures,
    Interface,
    Leverage,
    Manual,
    /// The part of the General page only the expert window draws — see [`OrderRulesSettings`].
    OrderRules,
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
    ///
    /// Authoritative only inside [`Self::touched`]: it is the projection of the PACKET that went
    /// out, whose other areas are whatever the core's snapshot held at that moment. Nothing renders
    /// it; it exists so the store can tell one edit from another.
    pub config: CoreConfig,
    /// Which areas of [`Self::config`] this edit actually asked to change.
    ///
    /// Carried so the store can compare two submissions WITHIN the mask. Without it the only
    /// available comparison was whole-projection equality, and an area the edit never named,
    /// drifting on the core between two attempts, made a retry look like a different edit.
    pub touched: FieldMask,
    /// The most recent rejection this edit received, if any. Retained across a retry's own
    /// `Submitted` event and cleared only by [`CoreConfigEditResult::Confirmed`] or a fresh user
    /// edit — never by a retry of the same edit; this is a store-arm rule, applied in
    /// `session::store`. What counts as "the same edit" is [`Self::touched`] plus equality within
    /// it.
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
