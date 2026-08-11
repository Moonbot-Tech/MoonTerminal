use moon_core::feed::ExchangeId;
use moon_core::venue::CoreVenue;

use crate::controls::venue_label::{venue_id_label, venue_label};

/// Build one core's venue as the session publishes it.
fn core_venue(code: u8, dex: &str, reported: &str) -> CoreVenue {
    CoreVenue {
        id: ExchangeId::with_dex(code, dex),
        dex: dex.to_string(),
        reported: reported.to_string(),
    }
}

/// A known platform code captions from the directory, never from what the core called itself.
///
/// Breakage: falling back to `reported` for a known code brings back the core's own spelling, so
/// `Binance Quarterly` and `FBinance` stop matching the rest of the interface. Code 6 is the case
/// that motivated this module — its reported name is `Binance Quarterly`, which no brand table
/// built from name fragments ever matched.
#[test]
fn known_codes_caption_from_the_directory() {
    assert_eq!(
        venue_label(&core_venue(6, "", "Binance Quarterly")),
        "Binance Quarterly"
    );
    assert_eq!(
        venue_label(&core_venue(4, "", "Binance Futures")),
        "Binance Futures"
    );
    assert_eq!(venue_label(&core_venue(3, "", "Binance")), "Binance Spot");
    assert_eq!(venue_label(&core_venue(5, "", "HTX")), "HTX Spot");
    assert_eq!(venue_label(&core_venue(2, "", "FBybit")), "Bybit Futures");
    assert_eq!(venue_label(&core_venue(14, "", "OKEx")), "OKX Spot");
}

/// HIP-3 cores share a brand and a kind, so only the DEX name tells their rows apart.
///
/// Breakage: dropping the suffix makes three Hyperliquid futures rows read identically while they
/// still group separately, which looks like duplicated rows rather than distinct venues.
#[test]
fn hip3_cores_carry_their_dex_name() {
    assert_eq!(
        venue_label(&core_venue(13, "xyz", "Hyper Futures")),
        "Hyperliquid Futures · xyz"
    );
    assert_eq!(
        venue_label(&core_venue(13, "", "Hyper Futures")),
        "Hyperliquid Futures"
    );
}

/// A venue known only by its identity must still caption, because a selection outlives the cores
/// that carried it — every core on the selected Log exchange can disconnect while it stands.
///
/// Breakage: falling back to a live member's caption leaves the trigger reading "not identified"
/// while a real venue filter is active, which is indistinguishable from having no filter.
#[test]
fn a_venue_captions_from_its_identity_alone() {
    assert_eq!(venue_id_label(ExchangeId::new(6)), "Binance Quarterly");
    assert_eq!(venue_id_label(ExchangeId::new(2)), "Bybit Futures");
    // The DEX name is not recoverable from the hash, so a HIP-3 venue keeps its brand and loses
    // only the suffix — never the whole caption.
    assert_eq!(
        venue_id_label(ExchangeId::with_dex(13, "xyz")),
        "Hyperliquid Futures"
    );
    assert_ne!(venue_id_label(ExchangeId::new(99)), "");
}

/// The DEX name is wire data that lands in a row label; it must be clamped and stripped of control
/// characters before it is drawn.
///
/// Breakage: concatenating it raw lets one malformed value stretch every picker row, and a newline
/// or a bidi override inside a label corrupts the row it is drawn in.
#[test]
fn a_wire_dex_name_is_clamped_and_stripped_before_display() {
    let long = "x".repeat(64);
    let label = venue_label(&core_venue(13, &long, ""));
    assert!(
        label.len() < "Hyperliquid Futures · ".len() + long.len(),
        "an overlong DEX name must be clamped: {label}"
    );
    assert_eq!(
        venue_label(&core_venue(13, "xy\nz\u{202e}", "")),
        "Hyperliquid Futures · xyz",
        "control characters must be dropped rather than drawn"
    );
    assert_eq!(
        venue_label(&core_venue(13, " \u{7} ", "")),
        "Hyperliquid Futures",
        "a DEX name with nothing printable adds no separator"
    );
}

/// A DEX this build has never heard of must caption itself, with no table to add it to.
///
/// Breakage: any per-DEX list — a name map, a brand alias, a hardcoded set — makes a newly deployed
/// HIP-3 venue render as plain `Hyperliquid Futures` beside the others it must be told apart from.
/// Hyperliquid already carries more than a dozen, and builders add them without asking us.
#[test]
fn an_unseen_hip3_dex_captions_itself() {
    for dex in ["ventuals", "felix", "kinetiq", "not-shipped-yet"] {
        let label = venue_label(&core_venue(13, dex, ""));
        assert!(
            label.starts_with("Hyperliquid Futures · "),
            "{dex} must keep the venue caption: {label}"
        );
        assert!(label.ends_with(dex), "{dex} must name itself: {label}");
    }
    // 15 bytes is the protocol maximum for a DEX name (`DexInfo` is a Pascal short string of one
    // length byte plus fifteen), and the display clamp sits above it deliberately — a name a core
    // can actually send must never be drawn clipped.
    let longest = "123456789012345";
    assert_eq!(longest.len(), 15);
    assert_eq!(
        venue_label(&core_venue(13, longest, "")),
        format!("Hyperliquid Futures · {longest}")
    );
    // Two different DEXes must never produce one caption.
    assert_ne!(
        venue_label(&core_venue(13, "ventuals", "")),
        venue_label(&core_venue(13, "felix", ""))
    );
}

/// An ordinal newer than this build has no brand, so the core's own caption is all there is.
///
/// Breakage: resolving an unknown ordinal to a neighbouring brand would label a new exchange as the
/// wrong one; returning an empty string would leave a blank row with no way to tell which core it is.
#[test]
fn unknown_codes_fall_back_to_the_reported_name() {
    assert_eq!(
        venue_label(&core_venue(99, "", "Kraken Futures")),
        "Kraken Futures"
    );
    assert_eq!(
        venue_label(&core_venue(99, "", "  Kraken  ")),
        "Kraken",
        "a padded caption must be trimmed, not shown as-is"
    );
    assert_ne!(
        venue_label(&core_venue(200, "", "")),
        "",
        "an unidentifiable venue still needs a caption"
    );
}
