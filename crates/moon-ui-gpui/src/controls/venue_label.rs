//! THE caption for a venue a core is connected to.
//!
//! Every core list in the terminal — the pickers, the Profit Monitor, the workspace rail, the Log
//! source menu, Settings → Connections — names an exchange through this one function, so a venue is
//! spelled identically wherever it appears and two cores on it cannot render as two rows.
//!
//! The name a core reports is NOT the caption. It is a free-form string whose spelling belongs to
//! the core build (`Binance Quarterly`, `FBybit`, `Gate.io`), and reading it as identity is what
//! left Binance COIN-M unbranded. The caption is built from the platform code through
//! `moon_core::venue`, and the reported name is the fallback for exactly one case: an ordinal newer
//! than this build, where the directory has no brand to name.

use moon_core::feed::ExchangeId;
use moon_core::venue::{CoreVenue, Venue, venue};
use rust_i18n::t;

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
///
/// Returns:
///     Display caption, never empty.
pub(crate) fn venue_label(venue: &CoreVenue) -> String {
    let base = match venue.resolved() {
        Some(known) => directory_caption(known),
        // An ordinal this build does not know: the core's own caption is the only name available.
        // It is wire data like the DEX name and gets the same treatment — a venue with nothing
        // printable to show says so rather than drawing an empty cell.
        None => sanitized_wire_text(&venue.reported, REPORTED_MAX_CHARS)
            .unwrap_or_else(unidentified_label),
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
///
/// Returns:
///     Display caption, never empty.
pub(crate) fn venue_section_label(venue: Option<&CoreVenue>) -> String {
    venue.map(venue_label).unwrap_or_else(unidentified_label)
}

/// Format the caption for a venue known only by its identity.
///
/// Used where the identity outlives every core that carried it — a selected Log exchange source
/// whose members have all disconnected. The DEX name cannot be recovered here ([`ExchangeId`] keeps
/// only its hash), so a HIP-3 venue captions as its brand alone rather than as nothing.
///
/// Args:
///     id: Venue identity held by a selection.
///
/// Returns:
///     Display caption, never empty.
pub(crate) fn venue_id_label(id: ExchangeId) -> String {
    venue(id.code)
        .map(directory_caption)
        .unwrap_or_else(unidentified_label)
}

/// Compose the caption the directory itself supplies: brand, then market kind.
///
/// Args:
///     known: Directory entry for a platform ordinal.
///
/// Returns:
///     `Binance Quarterly`, `Bybit Futures`, and so on.
fn directory_caption(known: Venue) -> String {
    format!(
        "{} {}",
        known.brand.display(),
        t!(known.kind.label_key()).as_ref()
    )
}

/// The one wording for "nothing names this venue", shared by every surface.
///
/// Returns:
///     Localized unidentified-exchange caption.
fn unidentified_label() -> String {
    t!("common.exchange_unknown").to_string()
}

/// Reduce a wire-supplied name to what a caption may show.
///
/// Unprintable characters are dropped by `moon_core::venue`, at the edge where the value is built,
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
        .filter(|c| !c.is_control() && !moon_core::venue::is_invisible_format(*c))
        .take(max_chars)
        .collect();
    let clean = clean.trim();
    (!clean.is_empty()).then(|| clean.to_string())
}

#[cfg(test)]
mod tests;
