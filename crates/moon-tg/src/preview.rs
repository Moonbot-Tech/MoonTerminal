//! Typed synthetic previews over the production renderers' shared layout and value helpers.

use crate::notify::{render::card_lines, trades::ClosedTrade};
use moon_core::config::{CardLayout, ReportLayout};

/// Semantic color of a value, independent of the desktop or Telegram palette.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tone {
    #[default]
    Plain,
    Link,
    Gain,
    Loss,
}

/// One raw text run, shared by the production HTML encoder and native preview.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub tone: Tone,
}
impl Span {
    /// Plain text does not carry HTML, so user names cannot become preview markup.
    pub(crate) fn plain(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            bold: false,
            italic: false,
            tone: Tone::Plain,
        }
    }
    /// Encode only presentation tags around escaped text for production Telegram HTML.
    pub(crate) fn html(&self) -> String {
        let mut text = crate::html::escape(&self.text);
        if self.bold {
            text = format!("<b>{text}</b>");
        }
        if self.italic {
            text = format!("<i>{text}</i>");
        }
        text
    }
}
/// One card line with field order and separators already resolved by the production walk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreviewLine {
    pub spans: Vec<Span>,
}
/// Structural row kind keeps group headers, totals and spacers distinct in the native preview.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreviewRowKind {
    Data,
    Group,
    Total,
    Spacer,
}
/// One report row with the real labels and column values, without an HTML parser.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreviewRow {
    pub kind: PreviewRowKind,
    pub cells: Vec<Span>,
    pub band: bool,
    pub first_right: bool,
}
/// A report table carries the same column and row order as the production renderer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreviewTable {
    pub title: String,
    pub caption: String,
    pub header: Vec<String>,
    pub rows: Vec<PreviewRow>,
}

/// Render a synthetic closed trade as raw typed spans using the production layout walk.
pub fn preview_card(layout: &CardLayout) -> Vec<PreviewLine> {
    card_lines(&sample_trade(), false, &layout.sanitized())
}

/// Render a synthetic grouped report with the saved table appearance and no live data.
pub fn preview_report(layout: &ReportLayout) -> PreviewTable {
    crate::report::preview_table(layout)
}

/// Plausible fixed money and price inputs; only the sample's local date follows the clock.
fn sample_trade() -> ClosedTrade {
    let close_utc = moon_core::util::time::now_unix_ms_i64() / 1000;
    ClosedTrade {
        coin: "SOLUSDT".into(),
        core_name: "Margo".into(),
        strategy: "Dropdown 3%".into(),
        close_utc,
        open_utc: close_utc - 7 * 60,
        quote: Some(moon_core::db::QuoteCurrency::usdt()),
        profit_native: Some(12.40),
        volume_native: Some(250.0),
        profit_usd: Some(12.40),
        volume_usd: Some(250.0),
        profit_pct: Some(4.96),
        buy_price: Some(142.31),
        sell_price: Some(143.02),
        ..Default::default()
    }
}

/// Synthetic civil clock encoded as UTC solely for local-date preview captions, never storage.
pub(crate) fn sample_clock() -> chrono::DateTime<chrono::Utc> {
    let millis = moon_core::util::time::now_unix_ms_i64()
        .saturating_add(moon_core::util::time::local_utc_offset_ms());
    chrono::DateTime::from_timestamp_millis(millis).unwrap_or(chrono::DateTime::UNIX_EPOCH)
}

#[cfg(test)]
mod tests;
