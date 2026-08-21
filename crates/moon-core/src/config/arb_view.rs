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
//! Nothing here comes from the protocol. The core reports a numeric platform code and a price; the
//! name, the colour, the order and the visibility are all this file's, which is why a venue can be
//! renamed at all — a Hyperliquid deployer reaches us as an index and nothing else.

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
///
/// `Eq` deliberately, like the two types around it: the caption cache holds the roster behind an
/// `Rc` and compares it per revision, and `Rc`'s pointer shortcut only applies when the payload is
/// `Eq`. Without it every pane deep-compares twenty venues, names and all, several times a second.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ArbVenueCfg {
    /// Protocol platform code. THE identity of the row — the name is a label the user may change,
    /// so keying anything on it would lose the row's settings the moment it was renamed.
    pub code: u8,
    /// Whether the row is printed at all.
    pub visible: bool,
    /// User's own name for the venue. Empty means the default spelling, which is also what a
    /// Hyperliquid deployer starts from before anybody names it.
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

    /// What this row is CALLED: the user's name, or the venue's default spelling.
    pub fn label(&self) -> String {
        if self.name.is_empty() {
            return self.venue().default_name();
        }
        self.name.clone()
    }
}

/// The whole arbitrage column: its roster, and what each row prints.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
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
        for cfg in self.venues.iter().filter(|v| v.visible) {
            if let Some(quote) = quotes.iter().find(|q| q.venue.code() == cfg.code) {
                out.push(ArbRow {
                    quote,
                    label: cfg.label(),
                    color: cfg.color,
                });
            }
        }
        for quote in quotes.iter().filter(|q| self.row(q.venue).is_none()) {
            out.push(ArbRow {
                quote,
                label: quote.venue.default_name(),
                color: None,
            });
        }
        if out.len() > ARB_MAX_ROWS {
            log::warn!(
                "арбитраж: площадок {}, печатаем первые {ARB_MAX_ROWS}",
                out.len()
            );
            out.truncate(ARB_MAX_ROWS);
        }
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
