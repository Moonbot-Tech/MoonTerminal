//! What the Alerts panel shows: one row per figure, the column set, the two filters and the sort.
//!
//! Deliberately free of GPUI and of `Backend`, so the rules that decide WHICH figures appear and in
//! what order are testable without a window. The panel builds [`FigRow`]s from the store and hands
//! them here; everything below is plain data.

use moon_core::figures::Figure;
use moon_core::session::CoreId;
use rust_i18n::t;

/// One table row: a figure as this panel shows it.
///
/// Everything the table draws or sorts by is resolved ONCE, here, rather than re-derived per cell:
/// the coin needs a market-source lookup, the tool name a locale lookup, and the strategy name a
/// walk of the core's strategy list — all three would otherwise run per column per repaint.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct FigRow {
    pub(super) core: CoreId,
    pub(super) core_name: String,
    /// Market key, used to open the chart and to address the figure. Never displayed raw.
    pub(super) market: String,
    /// Coin as the CORE names it. The raw market name overflows its cell on any exchange that
    /// spells one out, and on Hyperliquid it is an index (`@156`) rather than a name at all.
    pub(super) coin: String,
    /// Localized tool name, from the tool's own registry row.
    pub(super) figure: String,
    pub(super) price: f64,
    pub(super) time_ms: i64,
    /// Whether the figure's TOOL is one Moonbot knows — the `TChartObject` blob has a type for it,
    /// so this figure can carry a core alert. This is what the Kind column shows, and it is a
    /// property of the tool, not of where the figure was drawn.
    pub(super) alertable: bool,
    /// Whether the figure came FROM a core (drawn in Moonbot) rather than being drawn here. NOT a
    /// column: it decides whether the Alert box may be unticked, and what the delete button warns
    /// about, both of which the box says on hover.
    pub(super) from_server: bool,
    /// Whether the figure is armed as a core alert.
    pub(super) armed: bool,
    /// Whether arming is even possible: the core must know the tool, and a figure shared across
    /// cores has no single core to arm it on.
    pub(super) can_arm: bool,
    /// Whether the figure is shared with every core on its market. Carried only so the Alert box
    /// can say WHICH of the two reasons it is disabled for; sharing itself is set from the chart.
    pub(super) shared: bool,
    /// Whether this row's core is connected and can actually be sent a command. A chart alert is
    /// attempted once and never retried, so ticking the box on a core that is reconnecting would
    /// mark the figure armed here and leave Moonbot unaware of it.
    pub(super) core_online: bool,
    pub(super) id: u64,
    pub(super) strategy_id: u64,
    /// Resolved strategy name, or the em-dash placeholder.
    pub(super) strategy: String,
}

impl FigRow {
    /// Builds a row from a stored figure and the names its columns need.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        core: CoreId,
        core_name: String,
        market: String,
        coin: String,
        fig: &Figure,
        strategy: String,
        core_online: bool,
    ) -> Self {
        Self {
            core,
            core_name,
            market,
            coin,
            figure: t!(fig.tool().def().locale_key).to_string(),
            price: fig.kind.anchor_price(),
            // Fractional on the wire so a re-upsert can write Moonbot's own instant back
            // unchanged; the column shows whole minutes, so truncation costs the display nothing.
            time_ms: fig.created_ms as i64,
            alertable: fig.tool().def().alertable,
            from_server: fig.from_server,
            armed: fig.alert,
            can_arm: fig.can_alert(),
            shared: fig.shared,
            core_online,
            id: fig.id,
            strategy_id: fig.strategy_id,
            strategy,
        }
    }

    /// Whether this row matches a typed coin query, in any case.
    ///
    /// Matches the COIN as well as the raw market name: the user types `sol`, and on an exchange
    /// whose market is `@156` the raw name would never match it.
    pub(super) fn matches(&self, query: &str) -> bool {
        query.is_empty() || contains_ci(&self.coin, query) || contains_ci(&self.market, query)
    }

    /// Whether this row IS the figure a settings panel is aimed at.
    ///
    /// One place to compare the three-part identity, so a caller cannot drop a leg of it and match
    /// a different core's figure that happens to share an id.
    pub(super) fn is(&self, t: &crate::figstyle::FigStyleTarget) -> bool {
        self.core == t.core && self.market == t.market && self.id == t.id
    }
}

/// Whether `haystack` contains `needle`, case-insensitively and without allocating.
///
/// Byte-wise, which is exact for what it compares — market keys and tickers are ASCII, including
/// the `@156` an index-named market carries. This runs for every figure in the store on every
/// rebuild, and the obvious `haystack.to_uppercase().contains(..)` allocates per figure. Neither
/// side is pre-folded, so a query carrying non-ASCII is compared as typed rather than mapped into
/// bytes the haystack could never hold.
fn contains_ci(haystack: &str, needle: &str) -> bool {
    let (hay, pat) = (haystack.as_bytes(), needle.as_bytes());
    if pat.is_empty() {
        return true;
    }
    hay.windows(pat.len()).any(|w| w.eq_ignore_ascii_case(pat))
}

/// Which KIND of figure — by whether Moonbot has a type for it — the Kind filter asks about.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(super) enum KindFilter {
    #[default]
    All,
    /// Only tools Moonbot knows. These are the figures an alert can be put on.
    Moonbot,
    /// Only tools that exist here alone. These can never carry a core alert.
    Terminal,
}

impl KindFilter {
    /// Takes the FACT rather than a row, so the filter can run on a borrowed `Figure` before a row
    /// is built for it — and so a row and a figure cannot be judged by two different rules.
    pub(super) fn accepts(self, alertable: bool) -> bool {
        match self {
            Self::All => true,
            Self::Moonbot => alertable,
            Self::Terminal => !alertable,
        }
    }

    pub(super) fn to_u8(self) -> u8 {
        match self {
            Self::All => 0,
            Self::Moonbot => 1,
            Self::Terminal => 2,
        }
    }

    pub(super) fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::Moonbot,
            2 => Self::Terminal,
            _ => Self::All,
        }
    }
}

/// Whether a figure is armed as an alert, as the Alert filter asks about it.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(super) enum ArmFilter {
    #[default]
    All,
    Armed,
    Unarmed,
}

impl ArmFilter {
    /// Takes the FACT rather than a row; see [`SourceFilter::accepts`].
    pub(super) fn accepts(self, armed: bool) -> bool {
        match self {
            Self::All => true,
            Self::Armed => armed,
            Self::Unarmed => !armed,
        }
    }

    pub(super) fn to_u8(self) -> u8 {
        match self {
            Self::All => 0,
            Self::Armed => 1,
            Self::Unarmed => 2,
        }
    }

    pub(super) fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::Armed,
            2 => Self::Unarmed,
            _ => Self::All,
        }
    }
}

/// One table column. The enum order is the canonical left-to-right order; `key()` is what is
/// persisted, so the enum may be reordered without invalidating a saved layout.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum AlCol {
    /// The Alert checkbox — the column that WRITES rather than reports.
    Alert,
    Core,
    Coin,
    Figure,
    /// Whether Moonbot knows this tool — and therefore whether an alert can be put on it.
    Kind,
    Price,
    Time,
    Strategy,
    /// Settings and delete. Not sortable and never hidden.
    Actions,
}

impl AlCol {
    pub(super) const ALL: [AlCol; 9] = [
        AlCol::Alert,
        AlCol::Core,
        AlCol::Coin,
        AlCol::Figure,
        AlCol::Kind,
        AlCol::Price,
        AlCol::Time,
        AlCol::Strategy,
        AlCol::Actions,
    ];

    pub(super) fn key(self) -> &'static str {
        match self {
            Self::Alert => "alert",
            Self::Core => "core",
            Self::Coin => "coin",
            Self::Figure => "figure",
            Self::Kind => "kind",
            Self::Price => "price",
            Self::Time => "time",
            Self::Strategy => "strategy",
            Self::Actions => "actions",
        }
    }

    pub(super) fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.key() == key)
    }

    /// The column's bit in the visibility mask. The enum order IS the canonical order, so the
    /// discriminant is the bit index — a scan of `ALL` would answer the same at O(n) and would need
    /// a fallback for a variant missing from the list, which is a wrong answer waiting to happen.
    pub(super) fn bit(self) -> u32 {
        1 << (self as u32)
    }

    /// Localized header. `Actions` has none: two glyph buttons need no title, and a header over
    /// them would only take width from the columns that carry text.
    pub(super) fn title(self) -> String {
        match self {
            Self::Alert => t!("alerts.col.alert").to_string(),
            Self::Core => t!("alerts.col.core").to_string(),
            Self::Coin => t!("alerts.col.coin").to_string(),
            Self::Figure => t!("alerts.col.figure").to_string(),
            Self::Kind => t!("alerts.col.kind").to_string(),
            Self::Price => t!("alerts.col.price").to_string(),
            Self::Time => t!("alerts.col.time").to_string(),
            Self::Strategy => t!("alerts.col.strategy").to_string(),
            Self::Actions => String::new(),
        }
    }

    /// Base width in logical pixels, which `MoonDataTable` also uses as the auto-layout weight.
    pub(super) fn width(self) -> f32 {
        match self {
            Self::Alert => 52.0,
            Self::Core => 90.0,
            Self::Coin => 78.0,
            Self::Figure => 120.0,
            Self::Kind => 84.0,
            Self::Price => 96.0,
            Self::Time => 92.0,
            Self::Strategy => 160.0,
            Self::Actions => 76.0,
        }
    }

    /// Whether the header offers a sort. Only the action buttons do not: there is no order to put
    /// two identical glyphs in.
    pub(super) fn sortable(self) -> bool {
        !matches!(self, Self::Actions)
    }

    /// Whether the column may be switched off. The actions are the panel's only way to delete a
    /// figure or reach its settings, so they stay.
    pub(super) fn hideable(self) -> bool {
        !matches!(self, Self::Actions)
    }

    /// Whether [`Self::width`] is a FLOOR rather than a starting point.
    ///
    /// True for the action column alone. Every other column holds text, which a narrower width
    /// merely truncates — the reader still sees that something was cut. This one holds two
    /// fixed-size glyph buttons in a cell that is exactly its column's width with `overflow_hidden`,
    /// so a width below what they need does not shrink them: it CLIPS one away entirely, leaving a
    /// row that silently offers no delete button at all.
    pub(super) fn width_is_a_floor(self) -> bool {
        matches!(self, Self::Actions)
    }
}

/// Every column visible — the default, and what a column menu's All row restores.
pub(super) const ALL_COLUMNS_MASK: u32 = (1u32 << AlCol::ALL.len()) - 1;

/// The panel's copyable view state: what is filtered, how it is sorted, which columns are shown.
///
/// One value so the whole of it can be compared, persisted and restored at once.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) struct AlertsViewState {
    pub(super) kind: KindFilter,
    pub(super) arm: ArmFilter,
    pub(super) sort: AlCol,
    /// Ascending when true. The default sort is Time descending — newest first, as the panel has
    /// always opened.
    pub(super) sort_asc: bool,
    pub(super) columns: u32,
}

impl Default for AlertsViewState {
    fn default() -> Self {
        Self {
            kind: KindFilter::All,
            arm: ArmFilter::All,
            sort: AlCol::Time,
            sort_asc: false,
            columns: ALL_COLUMNS_MASK,
        }
    }
}

impl AlertsViewState {
    pub(super) fn shows(&self, col: AlCol) -> bool {
        self.columns & col.bit() != 0
    }

    /// Visible columns in canonical order.
    pub(super) fn visible_columns(&self) -> Vec<AlCol> {
        AlCol::ALL.into_iter().filter(|c| self.shows(*c)).collect()
    }

    /// Whether both scope filters accept a figure with these two facts.
    ///
    /// Deliberately separate from the coin query: the scope can be judged on a borrowed `Figure`
    /// before anything is allocated for it, while the query needs the RESOLVED coin, which costs a
    /// market-source lookup. Splitting them is what lets the rebuild drop most of the store before
    /// paying for it.
    pub(super) fn scope_accepts(&self, alertable: bool, armed: bool) -> bool {
        self.kind.accepts(alertable) && self.arm.accepts(armed)
    }
}

/// Sorts rows by one column.
///
/// Every comparison ends on `(core, id)` so equal keys keep one stable order instead of reshuffling
/// between rebuilds — a figure list where two rows swap places on every repaint is unusable, and
/// with a coarse key such as Source or Alert most rows ARE equal.
pub(super) fn sort_rows(rows: &mut [FigRow], col: AlCol, ascending: bool) {
    rows.sort_by(|a, b| {
        let ord = match col {
            AlCol::Alert => a.armed.cmp(&b.armed),
            AlCol::Core => a.core_name.cmp(&b.core_name),
            AlCol::Coin => a.coin.cmp(&b.coin),
            AlCol::Figure => a.figure.cmp(&b.figure),
            AlCol::Kind => a.alertable.cmp(&b.alertable),
            // Not a total order on floats, but a price is never NaN here: it comes from a figure
            // node, and a node with a NaN price could not have been placed or decoded.
            AlCol::Price => a
                .price
                .partial_cmp(&b.price)
                .unwrap_or(std::cmp::Ordering::Equal),
            AlCol::Time => a.time_ms.cmp(&b.time_ms),
            AlCol::Strategy => a.strategy.cmp(&b.strategy),
            // Unsortable; the caller never asks, and answering "equal" leaves the stable tiebreak.
            AlCol::Actions => std::cmp::Ordering::Equal,
        };
        let ord = if ascending { ord } else { ord.reverse() };
        ord.then_with(|| a.core.cmp(&b.core)).then_with(|| a.id.cmp(&b.id))
    });
}

#[cfg(test)]
mod tests;
