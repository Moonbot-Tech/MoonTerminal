//! Compact client settings and temporary-blacklist projections.

/// One row of the core's TEMPORARY blacklist (`TempBL`), which bans new entries on a symbol until
/// it expires. Selling and closing an existing position stay allowed, exactly as with the
/// permanent list.
///
/// Deliberately NOT a field of [`ClientSettings`]: that whole struct is compared for equality to
/// decide whether a queued settings write was accepted (`feed::live::client_settings`), and a
/// remainder that counts down on every snapshot would make every such comparison fail — silently
/// stalling every settings write, not just this one.
#[derive(Debug, Clone, PartialEq)]
pub struct TempBlacklistRow {
    /// Symbol exactly as the core spells it. Whether that is the coin or the whole market is the
    /// CORE's choice, not this terminal's — see `settings_diag`, the channel that reads it back.
    pub symbol: String,
    /// Remaining ban time. The wire carries a day fraction; this is the same value as a duration,
    /// which is the unit every caller here works in.
    pub remaining: std::time::Duration,
}

/// Longest temporary ban this terminal will represent, and the ceiling every wire value is clamped
/// to.
///
/// MoonBot's own presets top out at a day and its rows expire; a year is far past anything a core
/// legitimately holds, so a larger number is a corrupt packet rather than a very long ban.
pub(super) const TEMP_BLACKLIST_MAX: std::time::Duration =
    std::time::Duration::from_secs(365 * 24 * 60 * 60);

/// Every temporary-blacklist row a settings snapshot holds, decoded.
///
/// The ONE place the wire rows are read. moonproto's own `remaining_duration()` must never be
/// called instead — see [`temp_blacklist_remaining`] for what it does to a corrupt value — and a
/// rule enforced by one function survives an edit that copies of the same expression do not.
///
/// Args:
///     settings: Retained snapshot to read.
///
/// Returns:
///     The rows in wire order, each symbol spelled as the core spells it.
pub fn temp_blacklist_rows(settings: &moonproto::ClientSettingsCommand) -> Vec<TempBlacklistRow> {
    settings
        .temp_blacklist_entries()
        .map(|row| TempBlacklistRow {
            symbol: row.symbol.to_string(),
            remaining: temp_blacklist_remaining(row.remaining_days()),
        })
        .collect()
}

/// The remaining ban this snapshot holds for one symbol, or `None` when it holds no row for it.
///
/// Symbols are matched case-insensitively, which is the rule every writer here uses too: a core
/// that echoes a different case must not read as a different coin.
pub fn temp_blacklist_held(
    settings: &moonproto::ClientSettingsCommand,
    symbol: &str,
) -> Option<std::time::Duration> {
    settings
        .temp_blacklist_entries()
        .find(|row| row.symbol.eq_ignore_ascii_case(symbol))
        .map(|row| temp_blacklist_remaining(row.remaining_days()))
}

/// Turn one wire day-fraction into a remaining duration, without trusting it.
///
/// NOTE for the merge path: a row this sanitizes is also written back sanitized, because the wire
/// setter takes durations and a snapshot is always sent whole. That only ever moves a value the
/// core could not have meant — a NaN, a negative, or one past [`TEMP_BLACKLIST_MAX`] — and moving
/// it is still better than the alternative, which is a panic on the feed thread.
///
/// `TempBLTimes` is decoded by moonproto as a raw `f64::from_bits` of whatever the packet carried,
/// and its own `remaining_duration()` guards NaN and negatives but NOT overflow — so a corrupt or
/// hostile value reaches `Duration::from_secs_f64` and PANICS, on the per-core feed thread, on
/// every settings echo. This is the only conversion this crate performs.
///
/// Args:
///     days: Remaining time as the wire spells it, a fraction of a day.
///
/// Returns:
///     Zero for a row that has expired or carries no number at all; otherwise the duration, capped
///     at [`TEMP_BLACKLIST_MAX`] — which is also where every absurd value lands, infinity
///     included, so that a corrupt row is never mistaken for an expired one.
pub fn temp_blacklist_remaining(days: f64) -> std::time::Duration {
    // NaN is not a quantity at all, and anything at or below zero is a row the core has already
    // let expire.
    if days.is_nan() || days <= 0.0 {
        return std::time::Duration::ZERO;
    }
    let secs = days * 86_400.0;
    // Infinity lands here with every other absurd value, and on the SAME side: a corrupt row must
    // not read as "expired", because the merge would then send that expiry back and lift a ban the
    // core is holding.
    if secs >= TEMP_BLACKLIST_MAX.as_secs_f64() {
        return TEMP_BLACKLIST_MAX;
    }
    std::time::Duration::from_secs_f64(secs)
}

/// Core client-settings snapshot from moonproto `ClientSettings`, flattened for toolbar TP, SL,
/// and sell presets. This is decoupled from moonproto: raw fields such as `s_price` and `sb_num`
/// are `pub(crate)` in production and are read only through the command's public helpers.
#[derive(Debug, Clone, PartialEq)]
pub struct ClientSettings {
    /// Effective take-profit percentage from `effective_take_profit_percent`. Under
    /// `fixed_sell_mode` it equals the selected S-slot percentage and must not be shown on the TP
    /// button; see `take_profit_main_pct`.
    pub take_profit_pct: f64,
    /// TP button's own take-profit value from `x_sell` or scalp, independent of `fixed_sell_mode`.
    /// The button always shows this value so selecting an S slot does not replace its displayed TP.
    pub take_profit_main_pct: f64,
    /// Extended TP range from the `x_tmode` or `s9` flag: off means 0..100%, on means 100..900%,
    /// stored on the wire as `x_sell * 10`. This determines the slider range and popup checkbox.
    pub take_profit_extended: bool,
    /// Exact main-TP encoding, including scalp mode where `x_sell == 0`.
    pub take_profit_mode: crate::config::TakeProfitMode,
    /// Whether fixed-sell mode is enabled.
    pub fixed_sell_mode: bool,
    /// Stop-loss / price-drop level, % (`price_drop_level`).
    pub stop_loss_pct: f32,
    /// Trailing-stop percentage from `trailing_drop`.
    pub trailing_drop_pct: f32,
    /// Whether global take profit is enabled through `use_g_take_profit`, with its percentage from
    /// `g_take_profit`.
    pub use_global_take_profit: bool,
    pub global_take_profit_pct: f64,
    /// Panic-on-price-drop state from `panic_if_price_drop`.
    pub panic_if_price_drop: bool,
    /// Emulator mode from `emu_mode`.
    pub emu_mode: bool,
    pub buy_iceberg: bool,
    pub sell_iceberg: bool,
    pub sign_orders: bool,
    pub use_stop_market: bool,
    /// Default VStop BID-volume drop level as an integer percentage from `vol_drop_level`.
    pub vol_drop_level: i32,
    /// Coin blacklist enabled state from `use_coins_black_list` and its text from
    /// `coins_black_list_text`.
    pub use_blacklist: bool,
    pub blacklist_text: String,
    /// Six fixed-sell presets as visible percentages for buttons S1-S6.
    pub fixed_sell_pcts: [f64; 6],
    /// Selected fixed-sell slot in 1..=6 from `selected_fixed_sell_slot`.
    pub fixed_sell_slot: usize,
    /// Whether the CORE's own manual-strategy mode is enabled, through `use_manual_strategy`.
    ///
    /// Not this terminal's mode: an order this terminal places on a strategy carries it
    /// explicitly, so this describes what the core does with an order that arrives without one — a
    /// Moonbot-placed order, or one from another client. It seeds this terminal's own mode once,
    /// on a core that has never had one stored locally. Written from here in one case only: the
    /// exit barrier ahead of a bare order switches it off, so that order stays bare
    /// (`live::client_settings`, `SettingsMutation::NoManualStrategy`).
    pub use_manual_strategy: bool,
    /// Selected manual-strategy ID from `manual_strategy_id`; `0` means none is selected. Read on
    /// the same terms as [`Self::use_manual_strategy`].
    pub manual_strategy_id: u64,
}

impl ClientSettings {
    /// Project visible core values into the group-local manual-exit contract.
    pub fn group_exit_settings(&self) -> crate::config::GroupExitSettings {
        crate::config::GroupExitSettings {
            take_profit_pct: self.take_profit_main_pct,
            take_profit_mode: self.take_profit_mode,
            fixed_sell_pcts: self.fixed_sell_pcts,
            fixed_sell_slot: self.fixed_sell_mode.then_some(self.fixed_sell_slot),
            stop_loss_pct: self.stop_loss_pct,
            stop_loss_enabled: self.panic_if_price_drop,
            use_stop_market: self.use_stop_market,
        }
    }
}
