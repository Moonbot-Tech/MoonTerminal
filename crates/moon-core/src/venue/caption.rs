//! THE caption for a venue a core is connected to.
//!
//! Every core list in the terminal — the pickers, the Profit Monitor, the workspace rail, the Log
//! source menu, Settings → Connections, the Telegram report and Mini App — names an exchange
//! through this one function, so a venue is spelled identically wherever it appears and two cores
//! on it cannot render as two rows.
//!
//! This crate does not localize: which locale key a caption uses is decided here, and the caller
//! passes its own translator (`tr`) that turns a key into text, as [`super::MarketKind::label_key`]
//! already does for the market kind alone.
//!
//! The name a core reports is NOT the caption. It is a free-form string whose spelling belongs to
//! the core build (`Binance Quarterly`, `FBybit`, `Gate.io`), and reading it as identity is what
//! left Binance COIN-M unbranded. The caption is built from the platform code through
//! [`super::venue`], and the reported name is the fallback for exactly one case: an ordinal newer
//! than this build, where the directory has no brand to name.

use std::borrow::Cow;

use super::{CoreVenue, Venue, venue};
use crate::feed::ExchangeId;

/// The caller's translator from a locale key to text; borrowed when the dictionary holds it.
pub type Translate = dyn for<'a> Fn(&'a str) -> Cow<'a, str>;

/// Locale key of the caption for a venue nothing can name.
const UNIDENTIFIED_KEY: &str = "common.exchange_unknown";

/// Separator between a venue and its HIP-3 DEX name.
///
/// A glyph, so it stays out of the dictionary — `locales/README.md` keeps values free of them.
const DEX_SEPARATOR: &str = " · ";

/// Longest DEX name a caption will show.
///
/// `dex_name` is wire data: it arrives from `BaseCheck` in whatever length and shape the core sent,
/// and it lands in a row label and an element id. Real HIP-3 names are short (`xyz`, `crypto`), so
/// a clamp costs nothing legitimate and keeps one malformed value from stretching a picker row.
const DEX_MAX_CHARS: usize = 24;

/// Longest reported exchange name a caption will show.
///
/// Roomier than the DEX clamp because this one stands in for a whole venue name (`Binance Quarterly`
/// is 17), but still bounded: it is wire text drawn in a row.
const REPORTED_MAX_CHARS: usize = 48;

/// Format the caption for one identified core's venue.
///
/// Hyperliquid HIP-3 cores append their DEX name: those venues share a brand and a market kind but
/// trade different universes, and without the suffix three rows read identically while grouping
/// keeps them apart.
///
/// Args:
///     venue: What the core reported it is connected to.
///     tr: The caller's translator from a locale key to text.
///
/// Returns:
///     Display caption, never empty.
pub fn venue_label(venue: &CoreVenue, tr: &Translate) -> String {
    let base = match venue.resolved() {
        Some(known) => directory_caption(known, tr),
        // An ordinal this build does not know: the core's own caption is the only name available.
        // It is wire data like the DEX name and gets the same treatment — a venue with nothing
        // printable to show says so rather than drawing an empty cell.
        None => sanitized_wire_text(&venue.reported, REPORTED_MAX_CHARS)
            .unwrap_or_else(|| tr(UNIDENTIFIED_KEY).into_owned()),
    };
    match sanitized_wire_text(&venue.dex, DEX_MAX_CHARS) {
        Some(dex) => format!("{base}{DEX_SEPARATOR}{dex}"),
        None => base,
    }
}

/// Format one exchange SECTION heading, unidentified group included.
///
/// The shared form behind every core list: a section either stands for a venue or collects the
/// cores no venue could be named for, and both spell that the same way everywhere.
///
/// Args:
///     venue: Venue the section stands for, or `None` for the unidentified group.
///     tr: The caller's translator from a locale key to text.
///
/// Returns:
///     Display caption, never empty.
pub fn venue_section_label(venue: Option<&CoreVenue>, tr: &Translate) -> String {
    match venue {
        Some(venue) => venue_label(venue, tr),
        None => tr(UNIDENTIFIED_KEY).into_owned(),
    }
}

/// Format the caption for a venue known only by its identity.
///
/// Used where the identity outlives every core that carried it — a selected Log exchange source
/// whose members have all disconnected. The DEX name cannot be recovered here ([`ExchangeId`] keeps
/// only its hash), so a HIP-3 venue captions as its brand alone rather than as nothing.
///
/// Args:
///     id: Venue identity held by a selection.
///     tr: The caller's translator from a locale key to text.
///
/// Returns:
///     Display caption, never empty.
pub fn venue_id_label(id: ExchangeId, tr: &Translate) -> String {
    match venue(id.code) {
        Some(known) => directory_caption(known, tr),
        None => tr(UNIDENTIFIED_KEY).into_owned(),
    }
}

/// Compose the caption the directory itself supplies: brand, then market kind.
///
/// Args:
///     known: Directory entry for a platform ordinal.
///     tr: The caller's translator from a locale key to text.
///
/// Returns:
///     `Binance Quarterly`, `Bybit Futures`, and so on.
fn directory_caption(known: Venue, tr: &Translate) -> String {
    format!("{} {}", known.brand.display(), tr(known.kind.label_key()))
}

/// Reduce a wire-supplied name to what a caption may show.
///
/// Unprintable characters are dropped by [`super`], at the edge where the value is built,
/// so "this venue has a name" and "this venue draws a name" cannot disagree. What is left here is
/// the DISPLAY bound: a row has a width, and `dex` in particular reaches this function straight
/// from the wire on a `CoreVenue` a caller may have built itself.
///
/// Args:
///     text: Name as the core reported it.
///     max_chars: Longest form the caption will show.
///
/// Returns:
///     The displayable name, or `None` when nothing printable remains.
fn sanitized_wire_text(text: &str, max_chars: usize) -> Option<String> {
    let clean: String = text
        .trim()
        .chars()
        .filter(|c| !c.is_control() && !super::is_invisible_format(*c))
        .take(max_chars)
        .collect();
    let clean = clean.trim();
    (!clean.is_empty()).then(|| clean.to_string())
}

#[cfg(test)]
mod tests;
