//! Typed import-preview facts. The UI owns all wording and resolves the active locale at render time.

use super::schema_v7::ShortcutAction;

/// A preview caption, with raw configuration identifiers kept distinct from translated labels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreviewCaption {
    /// A configuration field name from the export, preserved verbatim.
    ConfigField(String),
    /// A MoonBot action whose name is rendered by the UI.
    Action(ShortcutAction),
    /// One-based order-size hotkey slot.
    OrderSizeSlot(usize),
    /// One-based fixed-sell hotkey slot.
    FixedSellSlot(usize),
    /// An exported color key with the theme set it belongs to.
    ColorField { key: String, light: bool },
    /// An unmapped INI entry, retained verbatim for comparison with the export.
    IniEntry {
        section: String,
        key: String,
        value: String,
    },
    /// UI theme caption.
    UiTheme,
    /// Split Order X: part count caption.
    SplitParts,
    /// Chart background caption.
    ChartBackground,
    /// Grid caption.
    Grid,
    /// Crosshair caption.
    Crosshair,
    /// Neutral labels (axes/cursor) caption.
    NeutralLabels,
    /// Rising candle caption.
    CandleUp,
    /// Falling candle caption.
    CandleDown,
    /// Neutral candle caption.
    CandleNeutral,
    /// Order book bid caption.
    BookBid,
    /// Order book ask caption.
    BookAsk,
    /// Buy line caption.
    BuyLine,
    /// Buy line (pending) caption.
    BuyPendingLine,
    /// Sell line caption.
    SellLine,
    /// Buy line (short) caption.
    BuyShortLine,
    /// Sell line (short) caption.
    SellShortLine,
    /// Trailing line caption.
    TrailingLine,
    /// Liquidation line caption.
    LiquidationLine,
    /// Order sizes B1-B6 caption.
    OrderSizes,
    /// Selected size preset caption.
    OrderSizeSelection,
    /// Fixed sell percentages (S1-S6) caption.
    FixedSellPrices,
    /// Selected fixed sell slot caption.
    FixedSellSelection,
    /// MarketsTable (columns) caption.
    MarketsTable,
    /// Mouse gestures caption.
    MouseGestures,
}

/// A before/after preview value; numeric and shortcut spellings are locale-neutral data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreviewValue {
    /// A number, numeric list, shortcut, slot identifier, or RGB hex value.
    Data(String),
    /// The light/dark color set, expressed without core-owned wording.
    ThemeLight(bool),
    /// The target group is selected after planning, so no single current value exists.
    SelectedGroup,
}

/// Why a parsed field is deliberately not transferred; parameters retain the original evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportReason {
    /// The shortcut VK has no supported mapping.
    UnknownKey { vk: u16 },
    /// The exported color could not be parsed.
    InvalidColor { value: String },
    /// The target RGB field cannot preserve meaningful alpha.
    ColorAlpha { alpha: u8 },
    /// Terminal has no such action (no core command).
    NoAction,
    /// Terminal has no separate color for order book level lines.
    NoBookLevelColor,
    /// Terminal displays closed orders using opacity, not color.
    ClosedOrderOpacity,
    /// Terminal has no equivalent style.
    NoStyle,
    /// Not in the mapping table (not applied).
    UnmappedColor,
    /// The mapping table for this section is not defined yet.
    UnmappedSection,
    /// Terminal has no equivalent setting.
    NoSetting,
    /// Table column meanings have not been mapped yet; columns are not transferred.
    UnmappedColumns,
    /// Unavailable in this export version (requires the Interop block).
    GesturesUnavailable,
}

/// A planning warning, retaining numeric parameters rather than a formatted sentence.
#[derive(Debug, Clone, PartialEq)]
pub enum ImportWarning {
    /// A valid split count is capped at the supported maximum.
    SplitPartsClamped { parts: u8, max: u8, value: u8 },
    /// MoonBot did not populate the Hotkeys block.
    HotkeysUnfilled,
    /// At least one size is nonfinite or nonpositive; the whole set is withheld.
    InvalidOrderSizes { values: [f64; 6] },
    /// The selected size index is outside 0..=5.
    OrderSizeSelection { value: i32 },
    /// At least one percentage is nonfinite or negative; the whole set is withheld.
    InvalidFixedSellPrices { values: [f32; 6] },
    /// The selected fixed-sell index is outside 0..=5.
    FixedSellSelection { value: u8 },
}
