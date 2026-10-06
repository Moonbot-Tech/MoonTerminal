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
    /// `trading.fav_markets` — the markets the trader marked, as the core spells them.
    ///
    /// A bare field rather than a section of its own, exactly like the manual block's one writable
    /// flag: it belongs to no settings PAGE this terminal draws. The core owns the list — MoonBot's
    /// own star writes the same string — so the chart's star reads and writes it rather than
    /// keeping a list of its own that only this machine would ever see.
    ///
    /// Wire shape: one comma-separated string. Read it through [`fav_markets_has`] and change it
    /// through [`fav_markets_set`] so the spelling and the separator rules live in one place.
    pub fav_markets: String,
    /// Core-owned manual-trading configuration: order-size presets, manual-strategy buttons, and
    /// the platform hotkey layout. A BLOCK, not a tab — see the module doc.
    pub manual: ManualSettings,
}

mod edit_model;
mod interface;
mod layout;
mod order_rules;
mod signals_general;
mod special;

pub use edit_model::*;
pub use interface::*;
pub use layout::*;
pub use order_rules::*;
pub use signals_general::*;
pub use special::*;

/// Report profit counters from moonproto `TProfitStateCommand`, shown as the "now" lines beside the
/// AutoStart loss caps.
///
/// These come from the core's report database, not from balances or an order stream, so they can
/// disagree with the header's session P&L by design.
///
/// The wire carries four scalars and names neither pair: `rep_total_profit`/`rep_total_trades`
/// back the hourly counter and `rep_trades_total`/`rep_count_trades` the trade window. That
/// pairing was settled on a live core (#863), where the first pair counted more trades than the
/// trades stop's cap allows; the earlier reading had the two crossed. Only the two "now" captions
/// depend on it.
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

mod change_set;
mod fav_markets;
mod fields;

pub use change_set::CoreChangeSet;
pub use fav_markets::{fav_markets_has, fav_markets_list, fav_markets_set};
pub use fields::{CORE_FIELDS, CoreField, FieldValue, ProbeBases, differing_fields, index_of};

#[cfg(test)]
mod tests;
