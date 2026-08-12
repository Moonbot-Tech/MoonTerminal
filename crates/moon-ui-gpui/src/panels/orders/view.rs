//! Order view-model enums and the persisted per-panel view state.

/// Primary sort key selected by the menu's mutually exclusive options.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum PrimarySort {
    SellFirst,
    BuyFirst,
    Creation,
    /// Put profitable positions first by sorting locally calculated PnL in descending order.
    ProfitFirst,
}

impl PrimarySort {
    /// Stable persistence code for `docks.json`, ported from egui's `to_u8`/`from_u8`.
    pub(super) fn to_u8(self) -> u8 {
        match self {
            PrimarySort::Creation => 0,
            PrimarySort::SellFirst => 1,
            PrimarySort::BuyFirst => 2,
            PrimarySort::ProfitFirst => 3,
        }
    }
    pub(super) fn from_u8(v: u8) -> Self {
        match v {
            1 => PrimarySort::SellFirst,
            2 => PrimarySort::BuyFirst,
            3 => PrimarySort::ProfitFirst,
            _ => PrimarySort::Creation,
        }
    }
}

/// Persisted per-view mode for lifting rows associated with Main to the top of the list.
///
/// The sort menu exposes two mutually exclusive checked options, either of which can be disabled.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum MainOnTop {
    /// Keep the regular sort order.
    Off,
    /// Lift every order for a market open on Main, across all cores.
    AllTicker,
    /// Lift only the highlighted row for each `(core, market)` pair open on Main.
    Highlighted,
}

impl MainOnTop {
    pub(super) fn to_u8(self) -> u8 {
        match self {
            MainOnTop::Off => 0,
            MainOnTop::AllTicker => 1,
            MainOnTop::Highlighted => 2,
        }
    }
    pub(super) fn from_u8(v: u8) -> Self {
        match v {
            1 => MainOnTop::AllTicker,
            2 => MainOnTop::Highlighted,
            _ => MainOnTop::Off,
        }
    }
}

/// Order-kind filter: all, real, or emulated.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum OrderKind {
    All,
    Real,
    Emu,
}

impl OrderKind {
    /// Stable persistence code for `docks.json`, ported from egui's `to_u8`/`from_u8`.
    pub(super) fn to_u8(self) -> u8 {
        match self {
            OrderKind::All => 0,
            OrderKind::Real => 1,
            OrderKind::Emu => 2,
        }
    }
    pub(super) fn from_u8(v: u8) -> Self {
        match v {
            1 => OrderKind::Real,
            2 => OrderKind::Emu,
            _ => OrderKind::All,
        }
    }
}

/// Order-table columns in canonical order.
///
/// A column's position in [`OrdCol::ALL`] is its bit number in
/// [`OrdersViewState::columns`]. [`OrdCol::key`] returns a stable `docks.json` persistence key
/// independent of enum order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum OrdCol {
    Core,
    Side,
    Token,
    Size,
    Buy,
    CurP,
    /// Take-profit price: the exit leg's target price from `sell_price`.
    TpPrice,
    Fill,
    Pnl,
    PnlPct,
    /// Estimated PnL at the take-profit price: `(tp - entry) * qty * direction`.
    PnlTp,
    Sl,
    Ts,
    Vstop,
    Strat,
    /// Strategy user-assigned name (`StrategyName`), distinct from the `Strat` kind column.
    StratName,
}

impl OrdCol {
    // Keep SL/TS/Vstop on the right beside Strat, leaving the important price and PnL fields
    // together. TP price sits beside Buy/Cur.P, and PNL TP sits beside PnL/PnL%.
    pub(super) const ALL: [OrdCol; 16] = [
        OrdCol::Core,
        OrdCol::Side,
        OrdCol::Token,
        OrdCol::Size,
        OrdCol::Buy,
        OrdCol::CurP,
        OrdCol::TpPrice,
        OrdCol::Fill,
        OrdCol::Pnl,
        OrdCol::PnlPct,
        OrdCol::PnlTp,
        OrdCol::Sl,
        OrdCol::Ts,
        OrdCol::Vstop,
        OrdCol::Strat,
        OrdCol::StratName,
    ];

    /// Stable key used by `docks.json` persistence and menu elements.
    pub(super) fn key(self) -> &'static str {
        match self {
            OrdCol::Core => "core",
            OrdCol::Side => "side",
            OrdCol::Token => "token",
            OrdCol::Size => "size",
            OrdCol::Sl => "sl",
            OrdCol::Ts => "ts",
            OrdCol::Vstop => "vstop",
            OrdCol::Buy => "buy",
            OrdCol::CurP => "cur.p",
            OrdCol::TpPrice => "tp.p",
            OrdCol::Fill => "fill",
            OrdCol::Pnl => "pnl",
            OrdCol::PnlPct => "pnl.pct",
            OrdCol::PnlTp => "pnl.tp",
            OrdCol::Strat => "strat",
            OrdCol::StratName => "strat_name",
        }
    }

    /// Column bit in the visibility mask, derived from its position in [`Self::ALL`].
    ///
    /// The mask is [`u32`] so that the count of columns can grow past 16 without overflowing the
    /// `1 << idx` shift.
    pub(super) fn bit(self) -> u32 {
        let idx = OrdCol::ALL
            .iter()
            .position(|c| *c == self)
            .unwrap_or_default();
        1u32 << idx
    }

    pub(super) fn from_key(key: &str) -> Option<OrdCol> {
        OrdCol::ALL.iter().copied().find(|c| c.key() == key)
    }
}

/// Default view mask with every column visible.
pub(super) const ALL_COLUMNS_MASK: u32 = (1u32 << OrdCol::ALL.len()) - 1;

/// Per-panel table view state: order kind, current-market filter, sorting, and visible columns.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct OrdersViewState {
    pub(super) kind: OrderKind,
    pub(super) only_current_market: bool,
    pub(super) primary: PrimarySort,
    pub(super) newest_first: bool,
    /// Optional header-click override. `None` preserves the legacy primary/newest menu order.
    pub(super) header_sort: Option<(OrdCol, bool)>,
    /// Whether to lift no Main rows, every matching market row, or only highlighted rows.
    pub(super) main_on_top: MainOnTop,
    /// Visible-column bit mask, where each bit comes from [`OrdCol::bit`]. Persisted as keys.
    pub(super) columns: u32,
}

impl OrdersViewState {
    /// Return whether a column is visible in the current view.
    pub(super) fn shows(&self, col: OrdCol) -> bool {
        self.columns & col.bit() != 0
    }

    /// Return visible columns in canonical order.
    pub(super) fn visible_columns(&self) -> Vec<OrdCol> {
        OrdCol::ALL
            .iter()
            .copied()
            .filter(|c| self.shows(*c))
            .collect()
    }
}

impl Default for OrdersViewState {
    fn default() -> Self {
        Self {
            // By default, show only real orders and put executed entries first so active positions
            // and their exit legs are immediately visible. The sort and kind menus persist changes
            // to `docks.json`.
            kind: OrderKind::Real,
            only_current_market: false,
            primary: PrimarySort::SellFirst,
            newest_first: true,
            header_sort: None,
            main_on_top: MainOnTop::Highlighted,
            columns: ALL_COLUMNS_MASK,
        }
    }
}
