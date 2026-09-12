//! Unit tests for chart text values, currency units, and label sizing.
//!
//! Explicit imports, never `use super::*`: the parent re-exports `gpui::*`, whose own `test`
//! would shadow the built-in `#[test]` attribute and make it expand recursively (CONTRIBUTING.md).
use crate::chartdx::text::{cursor_ref_price, fmt_prospective_order_size, volume_scale_label};

/// Removing the suffix would again present quote turnover as a bare coin count (issue #486).
#[test]
fn volume_scale_label_identifies_dollar_turnover() {
    assert_eq!(
        volume_scale_label(10_800.0, "USDT", 200.0, |_| 48.0).as_deref(),
        Some("10.8K$")
    );
    assert_eq!(
        volume_scale_label(10_800.0, "USDC", 200.0, |_| 48.0).as_deref(),
        Some("10.8K$")
    );
    assert_eq!(
        volume_scale_label(10_800.0, "USD", 200.0, |_| 48.0).as_deref(),
        Some("10.8K$")
    );
}

/// Hard-coding dollars would mislabel BTC and EUR markets, including small BTC turnover.
#[test]
fn volume_scale_label_preserves_the_markets_non_dollar_unit() {
    assert_eq!(
        volume_scale_label(0.125, "BTC", 200.0, |_| 72.0).as_deref(),
        Some("0.125 BTC")
    );
    assert_eq!(
        volume_scale_label(93_200.0, "EUR", 200.0, |_| 72.0).as_deref(),
        Some("93.2K EUR")
    );
}

/// Measuring only the number would clip a long quote; never truncate its currency identity.
#[test]
fn volume_scale_label_requires_room_for_the_complete_currency() {
    let measure = |text: &str| {
        assert_eq!(text, "10.8K LONGQUOTE");
        112.0
    };
    assert_eq!(
        volume_scale_label(10_800.0, "LONGQUOTE", 112.0, measure).as_deref(),
        Some("10.8K LONGQUOTE")
    );
    assert!(volume_scale_label(10_800.0, "LONGQUOTE", 111.0, measure).is_none());
    assert!(volume_scale_label(10_800.0, "", 200.0, |_| 0.0).is_none());
}

const LAST: f32 = 100.0;
const BOOK: Option<(f32, f32)> = Some((99.5, 100.5));

#[test]
fn above_the_last_price_measures_from_the_best_ask() {
    assert_eq!(cursor_ref_price(BOOK, LAST, 110.0), 100.5);
}

#[test]
fn below_the_last_price_measures_from_the_best_bid() {
    assert_eq!(cursor_ref_price(BOOK, LAST, 90.0), 99.5);
}

#[test]
fn the_last_price_itself_counts_as_above() {
    // The boundary must land on one side deterministically; matching the cursor block's own
    // `price >= last`, it takes the ask.
    assert_eq!(cursor_ref_price(BOOK, LAST, LAST), 100.5);
}

#[test]
fn without_a_book_the_reference_is_the_last_price() {
    // No book for the market yet, or the order book switched off for the window: `market.rs`
    // clears `book_best` in both cases rather than leaving a frozen bid/ask behind.
    assert_eq!(cursor_ref_price(None, LAST, 110.0), LAST);
    assert_eq!(cursor_ref_price(None, LAST, 90.0), LAST);
}

#[test]
fn a_one_sided_book_reads_the_same_from_either_direction() {
    // `best_bid_ask` reports the single populated side in both positions, so both directions
    // measure from it rather than one of them silently reverting to the last price.
    let one_sided = Some((100.5, 100.5));
    assert_eq!(cursor_ref_price(one_sided, LAST, 110.0), 100.5);
    assert_eq!(cursor_ref_price(one_sided, LAST, 90.0), 100.5);
}

/// The chart-specific order-size helper must preserve meaningful fractions without fixed zeros.
///
/// Replacing the shared formatter with fixed hundredths makes these literal display contracts red.
#[test]
fn prospective_order_size_label_uses_shared_compact_formatter() {
    assert_eq!(fmt_prospective_order_size(1_500.0), "1.5k");
    assert_eq!(fmt_prospective_order_size(10_000.0), "10k");
    assert_eq!(fmt_prospective_order_size(1_000_000.0), "1m");
    assert_eq!(fmt_prospective_order_size(50.0), "50");
}

/// The retained chart render path must call the chart-specific compact order-size helper.
///
/// Restoring the former fixed-hundredths expression fails this source-wiring oracle even when the
/// helper's direct tests remain green.
#[test]
fn prepare_wires_compact_order_size_label() {
    let source = include_str!("prepare.rs");

    assert!(source.contains("let text = fmt_prospective_order_size(usd);"));
    assert!(!source.contains("format!(\"{usd:.2}\")"));
}

/// The chart time-step preparation must use the width-derived target rather than a fixed count.
///
/// Breakage this pins: restoring the former literal `6.0` target would under-label wide plots and
/// crowd narrow plots even though the pure axis helper's direct tests remain green.
#[test]
fn prepare_wires_width_derived_time_label_target() {
    let source = include_str!("prepare.rs");
    let compact: String = source.split_whitespace().collect();
    let call = "moon_chart::axes::nice_time_step(window_ms/1000.0,";

    assert!(
        compact.contains(&format!(
            "{call}moon_chart::axes::time_label_target(plot_w),)"
        )),
        "prepared time steps must receive the plot-width target"
    );
    assert!(
        !compact.contains(&format!("{call}6.0)")),
        "prepared time steps must not restore the fixed six-label target"
    );
}

/// The cursor's volume readout is drawn larger than an order-line label at every slider setting.
///
/// Breakage this pins: routing the readout back through the plain label size — the shape it had
/// before — makes the two equal again, and the extra clamp keeps the bump from pushing the largest
/// setting past the bound the label size is held to.
#[test]
fn the_volume_readout_sits_above_the_label_size_and_stays_clamped() {
    use crate::chartdx::text::{READOUT_FONT_BUMP, label_font_px, readout_font_px};

    for delta in [-6.0, -2.0, 0.0, 3.0, 12.0] {
        assert_eq!(
            readout_font_px(delta),
            label_font_px(delta) + READOUT_FONT_BUMP,
            "the readout follows the slider, one fixed step above the label"
        );
    }
    // Both ends stay inside the bound `label_font_px` exists to hold.
    assert_eq!(readout_font_px(1_000.0), 40.0);
    assert_eq!(readout_font_px(-1_000.0), 6.0 + READOUT_FONT_BUMP);
}

/// The bottom-volume scale labels are larger and heavier than ordinary axis text.
///
/// Breakage this pins: restoring the axis size or the regular face for the two reference-line
/// labels, which are read against the bars rather than against an empty gutter.
#[test]
fn volume_scale_labels_are_larger_and_bolder_than_the_axis() {
    let source = include_str!("mod.rs");

    assert!(source.contains("const VOLUME_SCALE_FONT_SIZE: f32 = FONT_SIZE + 1.5;"));
    assert!(source.contains("const VOLUME_SCALE_WEIGHT: FontWeight = FontWeight::SEMIBOLD;"));

    let prepare: String = include_str!("prepare.rs")
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    assert!(
        prepare.contains("self.draw_volume_scale_text(ctx,&label,label_x,y,label_ax,0.5,ink,)?")
            || prepare
                .contains("self.draw_volume_scale_text(ctx,&label,label_x,y,label_ax,0.5,ink)?"),
        "the band's scale labels must take the volume-scale face, not the axis one"
    );
    // The edge the labels hug is the reader's choice, and the anchor follows the edge: a label at
    // the right edge anchored by its left corner would run into the price gutter.
    assert!(
        prepare.contains("ifvolume_scale_right{(plot_right-4.0,1.0)}else{(plot_left+4.0,0.0)}"),
        "the scale labels must anchor by the edge they sit at"
    );
}

/// A reference line too close to the band floor keeps its label off the plot's bottom edge.
///
/// Breakage this pins: the former fixed 6px threshold, which was already under half the axis line
/// height and would let the larger scale face hang past the plot once it grew.
#[test]
fn a_scale_label_is_skipped_rather_than_hung_past_the_band_floor() {
    use crate::chartdx::text::volume_scale_label_fits;

    // The max line sits at the band's own top, so it fits as soon as the band is tall enough.
    assert!(volume_scale_label_fits(72.0, 1.0));
    // The average line rides low on a shallow band: no room, so no label.
    assert!(!volume_scale_label_fits(40.0, 0.1));
    assert!(!volume_scale_label_fits(0.0, 1.0));
    // The boundary is the label's own half line height, not a constant.
    let half = (11.5 + 1.5 + 4.0) * 0.5;
    assert!(volume_scale_label_fits(half, 1.0));
    assert!(!volume_scale_label_fits(half - 0.01, 1.0));
}
