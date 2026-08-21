//! Arbitrage column presentation: which venues the chart lists, what it prints for each, in what
//! order and in what colour.
//!
//! GLOBAL, not per tab, and that is a deliberate exception to how the caption modules beside it
//! work. A caption module answers "what does THIS chart show" and is worth setting per tab; the
//! arbitrage roster answers "which venues do I care about, and what colour is Gate to me" — the
//! answer a trader gives once and expects on every chart. It also travels in its own portable file
//! for the same reason `theme.toml` and `detects_view.toml` do: it is worth copying to another
//! machine on its own.
//!
//! The core reports a numeric platform code and a price; the colour, the order and the visibility
//! are this file's alone. NAMES are shared: this build spells the exchanges it knows, a Hyperliquid
//! deployer is named by the core's own `known_dexes` list when it has one, and the user's own name
//! overrides both — which is what a deployer whose core sent no list still needs.

use serde::{Deserialize, Serialize};

use super::{paths, toml_io};
use crate::market::ArbVenue;

/// Longest venue name kept; anything longer is cut on write.
///
/// A venue name is a column head drawn over candles, in a column sized by its widest row. Twelve
/// characters is already wider than every default name, and an unbounded string would push the
/// prices themselves off the pane.
pub const ARB_NAME_MAX: usize = 12;

/// Most rows one arbitrage column prints.
///
/// The ceiling exists because each row is a retained text run addressed by its INDEX (see
/// `ROW_RUN_STRIDE`), and because a column longer than this stops being readable beside a chart.
/// Venues past it are dropped with a warning rather than silently.
///
/// Clear of what a READ can actually produce — every venue this build names plus the deployer
/// indices it scans — so the ceiling is a guard against a hand-edited roster, never something a
/// normal core walks into. A ceiling below that would drop a venue from a full column and log it
/// from the caption rebuild, several times a second.
pub const ARB_MAX_ROWS: usize = 32;

/// What one row of the column prints beside the venue's name.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArbShow {
    /// The venue's price and how far it is from this market, which is what the reference terminal
    /// prints and what an arbitrage is actually read as.
    #[default]
    PriceAndSpread,
    /// Only the price.
    Price,
    /// Only the difference.
    Spread,
}

impl ArbShow {
    pub const ALL: [ArbShow; 3] = [ArbShow::PriceAndSpread, ArbShow::Price, ArbShow::Spread];

    pub fn shows_price(self) -> bool {
        matches!(self, ArbShow::PriceAndSpread | ArbShow::Price)
    }

    pub fn shows_spread(self) -> bool {
        matches!(self, ArbShow::PriceAndSpread | ArbShow::Spread)
    }

    pub fn locale_key(self) -> &'static str {
        match self {
            ArbShow::PriceAndSpread => "arb.show.price_and_spread",
            ArbShow::Price => "arb.show.price",
            ArbShow::Spread => "arb.show.spread",
        }
    }
}

/// One venue's row in the column.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ArbVenueCfg {
    /// Protocol platform code. THE identity of the row — the name is a label the user may change,
    /// so keying anything on it would lose the row's settings the moment it was renamed.
    pub code: u8,
    /// Whether the row is printed at all.
    pub visible: bool,
    /// User's own name for the venue, overriding every other spelling.
    ///
    /// Empty means "whatever names it best": the core's own deployer name when there is one, and
    /// this build's spelling otherwise. Left empty by default, so a core that starts reporting a
    /// deployer's real name shows it without anyone editing the roster.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// Fixed `0xRRGGBB` for the row, or `None` for the chart's caption colour.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<u32>,
}

impl ArbVenueCfg {
    /// A venue at its defaults: shown, unnamed, in the theme's colour.
    pub fn new(venue: ArbVenue) -> Self {
        Self {
            code: venue.code(),
            visible: true,
            name: String::new(),
            color: None,
        }
    }

    pub fn venue(&self) -> ArbVenue {
        ArbVenue::from_code(self.code)
    }

    /// What this row is CALLED, in the order the names win: the user's own, then whatever the
    /// live quote carries, then this build's spelling.
    ///
    /// The middle one is the deployer case — `known_dexes` names it, and only a live read has that
    /// — which is why the label takes the quote rather than reading it off the roster alone.
    pub fn label_for(&self, quote: Option<&crate::market::ArbQuote>) -> String {
        if !self.name.is_empty() {
            return self.name.clone();
        }
        match quote.map(|q| q.dex_name.as_str()).filter(|n| !n.is_empty()) {
            Some(name) => name.to_string(),
            None => self.venue().default_name(),
        }
    }

    /// What this row is called with no live quote in hand, but with the core's deployer list.
    ///
    /// The settings window's case: it has a roster and no coin, so it cannot take a name off a
    /// quote — but the list the quote's name comes FROM is readable on its own, and the window has
    /// to agree with the chart about what a venue is called.
    pub fn label_with(&self, dex_names: &[String]) -> String {
        if !self.name.is_empty() {
            return self.name.clone();
        }
        let named = self
            .venue()
            .deployer_index()
            .and_then(|index| dex_names.get(usize::from(index)))
            .filter(|name| !name.is_empty());
        match named {
            Some(name) => name.clone(),
            None => self.venue().default_name(),
        }
    }

    /// What this row is called with nothing else to go by.
    pub fn label(&self) -> String {
        self.label_for(None)
    }
}

/// The whole arbitrage column: its roster, and what each row prints.
///
/// NOT `Eq` — it carries a percentage — so the caption cache compares it by POINTER rather than by
/// value, exactly as it already does for the caption configuration itself. A roster is replaced
/// wholesale when the settings window writes it, so pointer identity answers "did it change"
/// exactly, and answering it by value would walk two dozen venues per pane per revision.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ArbViewCfg {
    /// Venues in PRINT order. A venue absent from this list is still drawn — appended after it, at
    /// its defaults — so a core that starts reporting a deployer nobody has configured does not go
    /// unnoticed.
    pub venues: Vec<ArbVenueCfg>,
    /// What each row prints.
    pub show: ArbShow,
    /// Whether a venue that is blocked for deposits or withdrawals is marked.
    ///
    /// On by default: a spread on a coin that cannot be moved off the venue is not an opportunity,
    /// and that is exactly the row a reader must not mistake for one.
    pub mark_blocked: bool,
    /// Smallest spread, in percent, a venue is printed at all for. `0` prints every venue.
    ///
    /// A different question from the colour threshold beside it in the caption's style, and both
    /// are worth having: this one shortens the COLUMN — twelve venues quoting the same price are
    /// twelve lines of nothing — while the colour threshold keeps the line and takes the paint off
    /// it. Off by default, because a venue that has vanished from the column cannot be told from a
    /// venue that stopped reporting.
    #[serde(default)]
    pub min_abs_pct: f32,
}

impl Default for ArbViewCfg {
    /// Every venue a read can produce, shown, in the catalogue's own order.
    ///
    /// The deployer indices are here for a reason that is easy to miss: `arrange` will happily
    /// PRINT a venue the roster does not list, but only a listed one can be renamed, recoloured or
    /// moved — and a Hyperliquid deployer arrives as a bare index, which is exactly the venue a
    /// user needs to rename. Listing them costs eight rows in a window and nothing on a chart,
    /// since a venue with no quote prints no line.
    fn default() -> Self {
        let venues = ArbVenue::KNOWN
            .into_iter()
            .chain((0..ArbVenue::DEPLOYERS_SCANNED).map(ArbVenue::deployer))
            .map(ArbVenueCfg::new)
            .collect();
        Self {
            venues,
            show: ArbShow::default(),
            mark_blocked: true,
            min_abs_pct: 0.0,
        }
    }
}

impl ArbViewCfg {
    pub fn load() -> Self {
        let mut cfg: Self =
            toml_io::load_or_default(&paths::arb_view_path(), "arb_view.toml", |_| {});
        cfg.sanitize();
        cfg
    }

    pub fn save(&self) {
        if let Err(e) = toml_io::save(&paths::arb_view_path(), self, "arb_view.toml") {
            log::warn!("не записал arb_view.toml: {e:#}");
        }
    }

    /// Repair a hand-edited file: cut over-long names, drop duplicate venues.
    ///
    /// Duplicates matter more than they look: two rows for one code would print the venue twice and
    /// give the second row's settings to whichever one the lookup found first.
    pub fn sanitize(&mut self) {
        // A hand-edited negative or non-finite floor would hide every venue or none unpredictably;
        // both read as "the column broke".
        if !(self.min_abs_pct.is_finite() && self.min_abs_pct >= 0.0) {
            self.min_abs_pct = 0.0;
        }
        let mut seen = std::collections::HashSet::new();
        self.venues.retain(|v| seen.insert(v.code));
        for venue in &mut self.venues {
            let cut: String = venue.name.trim().chars().take(ARB_NAME_MAX).collect();
            let cut = cut.trim_end().to_string();
            if venue.name != cut {
                venue.name = cut;
            }
        }
    }

    /// The row for one venue, or `None` when the file does not mention it.
    pub fn row(&self, venue: ArbVenue) -> Option<&ArbVenueCfg> {
        self.venues.iter().find(|v| v.code == venue.code())
    }

    /// Order the quotes for printing and drop what is hidden.
    ///
    /// THE one place the roster meets live data, so the popup, the chart and the sample line cannot
    /// disagree about which venues appear. Quotes for a venue the file does not list are appended
    /// at the end at their defaults — a deployer the core just started reporting is data the user
    /// has not seen yet, and hiding it until they configure it would hide the fact that it exists.
    ///
    /// Args:
    ///     quotes: What the core reports for this market, in any order.
    ///
    /// Returns:
    ///     Rows to print, each paired with the venue's configured label and colour.
    pub fn arrange<'a>(&'a self, quotes: &'a [crate::market::ArbQuote]) -> Vec<ArbRow<'a>> {
        let mut out: Vec<ArbRow<'a>> = Vec::new();
        // The floor, asked once per quote wherever a quote is considered.
        let floor = f64::from(self.min_abs_pct);
        let shows = move |q: &crate::market::ArbQuote| q.spread_pct.abs() >= floor;
        for cfg in self.venues.iter().filter(|v| v.visible) {
            if let Some(quote) = quotes
                .iter()
                .find(|q| q.venue.code() == cfg.code)
                .filter(|q| shows(q))
            {
                out.push(ArbRow {
                    quote,
                    label: cfg.label_for(Some(quote)),
                    color: cfg.color,
                });
            }
        }
        for quote in quotes
            .iter()
            .filter(|q| self.row(q.venue).is_none() && shows(q))
        {
            out.push(ArbRow {
                quote,
                // Unlisted, so there is no user name to prefer — but the core may still have named
                // it, and a deployer arriving as "hyna" should not print as "HL #3".
                label: match quote.dex_name.is_empty() {
                    true => quote.venue.default_name(),
                    false => quote.dex_name.clone(),
                },
                color: None,
            });
        }
        // Silently, and that is deliberate: this runs on every caption rebuild, and the venue
        // count comes from the CORE's watch mask — a deployer-heavy core would otherwise write the
        // same warning to the log several times a second. The ceiling is a guard against a column
        // taller than a pane, not a condition worth reporting.
        out.truncate(ARB_MAX_ROWS);
        out
    }
}

/// One arranged row: a live quote with the presentation the roster gives it.
#[derive(Clone, Debug, PartialEq)]
pub struct ArbRow<'a> {
    pub quote: &'a crate::market::ArbQuote,
    pub label: String,
    pub color: Option<u32>,
}

#[cfg(test)]
mod tests;
