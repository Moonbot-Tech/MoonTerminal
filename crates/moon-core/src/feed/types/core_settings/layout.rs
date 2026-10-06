//! Leverage and manual trading layout settings.

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

impl CoreHotkeyAction {
    /// Every action, in the order [`CoreHotkeyLayout::named`] carries them.
    ///
    /// The enum has no iterator of its own, and the alternative is what the tree already had: a
    /// second transcription of these twenty-seven names wherever one is needed, kept in step by
    /// hand. Same reason [`crate::config::MouseGestureBinding::ALL`] and
    /// [`crate::config::MoveKind::ALL`] exist.
    pub const ALL: [Self; 27] = [
        Self::CancelBuy,
        Self::PanicSell,
        Self::JoinSells,
        Self::SwitchCharts,
        Self::ReloadBook,
        Self::NewLong,
        Self::NewShort,
        Self::SplitOrder,
        Self::ShiftBuyUp,
        Self::ShiftBuyDown,
        Self::ShiftSellUp,
        Self::ShiftSellDown,
        Self::MakeShot,
        Self::MakeShotBot,
        Self::ReloadChart,
        Self::ScalePlus,
        Self::ScaleMinus,
        Self::SellPlus,
        Self::SellMinus,
        Self::SpyMode,
        Self::ShowCharts,
        Self::SplitOrderX,
        Self::SwitchFigure,
        Self::FitSells,
        Self::PanicSellOne,
        Self::CancelAllBuys,
        Self::Broadcast,
    ];
}

/// Number of single-key ([`CoreHotkeyAction`]) hotkey slots on [`CoreHotkeyLayout::named`].
pub const CORE_HOTKEY_ACTION_COUNT: usize = CoreHotkeyAction::ALL.len();

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
        // Destructured for the reason [`crate::feed::GeneralSettings`]'s is. This was the last impl in the file
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
