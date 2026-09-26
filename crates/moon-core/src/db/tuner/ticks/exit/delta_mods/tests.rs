//! The Delta Modifiers tab's sum as the core forms it (the core developer via LinKvo, 2026-09-24),
//! and the core's own sum read back off its record.

use super::*;
use crate::db::tuner::ticks::exit::tests::deal;
use crate::db::tuner::ticks::hook::KIND_MOONHOOK;
use crate::db::tuner::ticks::mshot::{MarketSign, Modifiers};

/// The market and the BTC deltas are read as magnitudes in the sell family; the corridor family
/// keeps their sign.
#[test]
fn the_sell_family_reads_btc_as_a_magnitude() {
    let mut d = deal(false).deltas;
    d.btc1h = -3.0;
    d.btc5m = -1.0;
    d.market1h = -2.0;
    let family = |market_sign| Modifiers {
        add_btc_1h: 1.0,
        add_btc_5m: 1.0,
        add_market_1h: 1.0,
        market_sign,
        ..Modifiers::default()
    };
    assert!((family(MarketSign::Magnitude).near_addition(&d) - 6.0).abs() < 1e-9);
    assert!((family(MarketSign::Signed).near_addition(&d) - -6.0).abs() < 1e-9);
}

/// The sum is a magnitude, capped from above by `MaxModifier` when it is set: a falling coin
/// moves the levels the same way a rising one does.
#[test]
fn the_sum_is_a_capped_magnitude() {
    let mods = Modifiers {
        add_1h: 1.0,
        ..Modifiers::default()
    };
    let p = ExitParams {
        sell_mods: mods,
        ..ExitParams::default()
    };
    let mut d = deal(false);
    d.deltas.d1h = -5.0;
    assert!((modifier_sum(&p, &d, d.buy_ms) - 5.0).abs() < 1e-9);
    let capped = ExitParams {
        max_modifier: 2.0,
        ..p
    };
    assert!((modifier_sum(&capped, &d, d.buy_ms) - 2.0).abs() < 1e-9);
}

/// A long whose snapshot sums to 2 while the core placed its take off a sum of 3.
fn hook_long() -> (Deal, ExitParams) {
    let mut d = deal(false);
    d.kind = KIND_MOONHOOK.to_string();
    d.buy_price = 100.0;
    d.tick = Some(0.0001);
    d.hook_depth_pct = Some(2.0);
    d.hook_stated_take_pct = Some(1.0);
    d.deltas.d15m = 20.0;
    d.sell_reason = REASON_TAKE.to_string();
    // 100 · 1.01 · (1 + 0.3 · 3 / 100)
    d.sell_price = 101.0 * 1.009;
    let p = ExitParams {
        sell_modifier: 0.3,
        hook_sell_level_pct: 50.0,
        sell_mods: Modifiers {
            add_15m: 0.1,
            ..Modifiers::default()
        },
        ..ExitParams::default()
    };
    (d, p)
}

/// The take a hook's untouched take closed at is the core's sum spent: read back, the fact's own
/// parameters sum to the core's number, not the snapshot's.
#[test]
fn a_take_closed_sale_gives_the_core_its_own_sum() {
    let (mut d, p) = hook_long();
    assert!((modifier_sum(&p, &d, d.buy_ms) - 2.0).abs() < 1e-9);
    d.fact_modifier = FactModifier::of(&d, &p, None);
    assert!(d.fact_modifier.is_some());
    assert!((modifier_sum(&p, &d, d.buy_ms) - 3.0).abs() < 1e-3);
}

/// The archived take wins over the sale: a sale after the line moved is not the take.
#[test]
fn the_archived_take_is_read_before_the_sale() {
    let (mut d, p) = hook_long();
    d.sell_reason = "Auto Price Down".to_string();
    d.sell_price = 100.5;
    // 100 · 1.01 · (1 + 0.3 · 4 / 100)
    let line = [(d.buy_ms, 101.0 * 1.012), (d.buy_ms + 5_000, 100.8)];
    d.fact_modifier = FactModifier::of(&d, &p, Some(&line));
    assert!((modifier_sum(&p, &d, d.buy_ms) - 4.0).abs() < 1e-3);
    // Without the line, a sale that is not the take's reads nothing.
    assert_eq!(FactModifier::of(&d, &p, None), None);
}

/// A short's take divides, and so does the reading back.
#[test]
fn a_short_reads_its_sum_through_the_division() {
    let (mut d, p) = hook_long();
    d.is_short = true;
    // 100 / 1.01 / (1 + 0.3 · 3 / 100)
    d.sell_price = 100.0 / 1.01 / 1.009;
    d.fact_modifier = FactModifier::of(&d, &p, None);
    assert!((modifier_sum(&p, &d, d.buy_ms) - 3.0).abs() < 1e-3);
}

/// The stop's printed level carries the same sum: `StopLoss − StopLossModifier · Σ`.
#[test]
fn a_printed_stop_gives_the_sum_where_no_take_does() {
    let (mut d, mut p) = hook_long();
    p.sell_modifier = 0.0;
    p.stop_loss_pct = -2.0;
    p.stop_loss_modifier = 0.2;
    // −2 − 0.2 · 3 = −2.6 %
    d.sell_price = 97.0;
    d.sell_reason = "StopLoss AutoActivated on price drop: BID = 97.3 (strategy <X>); \
                     StopLoss fixed: 97.4000 Allow"
        .to_string();
    d.fact_modifier = FactModifier::of(&d, &p, None);
    assert!((modifier_sum(&p, &d, d.buy_ms) - 3.0).abs() < 1e-3);
}

/// A variant's sum is the model's own moved by the fact's miss, scaled to its coefficients:
/// doubling every coefficient doubles the whole sum; zeroing them leaves none.
#[test]
fn a_variant_inherits_the_miss_scaled_to_its_coefficients() {
    let (mut d, p) = hook_long();
    d.fact_modifier = FactModifier::of(&d, &p, None);
    let doubled = ExitParams {
        sell_mods: Modifiers {
            add_15m: 0.2,
            ..Modifiers::default()
        },
        ..p.clone()
    };
    assert!((modifier_sum(&doubled, &d, d.buy_ms) - 6.0).abs() < 1e-3);
    let none = ExitParams {
        sell_mods: Modifiers::default(),
        ..p.clone()
    };
    assert!(modifier_sum(&none, &d, d.buy_ms).abs() < 1e-9);
    // A variant that only moves `SellModifier` keeps the core's sum.
    let other_spend = ExitParams {
        sell_modifier: 0.5,
        ..p
    };
    assert!((modifier_sum(&other_spend, &d, d.buy_ms) - 3.0).abs() < 1e-3);
}

/// Nothing is read for a trade without an `Add*` term — the core's sum is zero — nor for a take
/// the rule does not place.
#[test]
fn no_reading_without_terms_or_a_rule_take() {
    let (d, p) = hook_long();
    let bare = ExitParams {
        sell_mods: Modifiers::default(),
        ..p.clone()
    };
    assert_eq!(FactModifier::of(&d, &bare, None), None);
    let fixed = ExitParams {
        hook_sell_fixed: true,
        ..p
    };
    assert_eq!(FactModifier::of(&d, &fixed, None), None);
}

/// A level under the formula by a price step is the grid's rounding and reads zero; one far
/// under it is a level the formula does not explain and reads nothing.
#[test]
fn a_negative_reading_is_rounding_or_nothing() {
    let (mut d, p) = hook_long();
    d.sell_price = 101.0 - 0.0001;
    d.fact_modifier = FactModifier::of(&d, &p, None);
    assert!(modifier_sum(&p, &d, d.buy_ms).abs() < 1e-9);
    d.sell_price = 100.5;
    assert_eq!(FactModifier::of(&d, &p, None), None);
}

/// Coefficients of both signs never cancel the scale: the miss is spread over their magnitudes,
/// and a variant that zeroes every term still sums to nothing.
#[test]
fn mixed_signs_keep_a_scale() {
    let (mut d, mut p) = hook_long();
    p.sell_mods.add_1h = -0.1;
    d.deltas.d1h = 0.0;
    d.fact_modifier = FactModifier::of(&d, &p, None);
    assert!((modifier_sum(&p, &d, d.buy_ms) - 3.0).abs() < 1e-3);
    let none = ExitParams {
        sell_mods: Modifiers::default(),
        ..p
    };
    assert!(modifier_sum(&none, &d, d.buy_ms).abs() < 1e-9);
}

/// A level that rounds to the record's price over a wide band of sums — a coarse step, a small
/// coefficient — keeps the model's own sum: the grid's rounding is never read as a sum.
#[test]
fn a_wide_band_keeps_the_models_sum() {
    let (mut d, mut p) = hook_long();
    p.sell_modifier = 0.001;
    // The model's sum of 2 moves the take by 0.002 %, well inside half of a 0.01 step.
    d.sell_price = 101.0;
    d.tick = Some(0.01);
    d.fact_modifier = FactModifier::of(&d, &p, None);
    assert!((modifier_sum(&p, &d, d.buy_ms) - 2.0).abs() < 1e-9);
}

/// A trade whose take and stop both hold a reading keeps the sums both allow: a coarse step
/// leaves the take's band wide, and the stop's narrows it.
#[test]
fn the_take_and_the_stop_readings_overlap() {
    let (mut d, p) = hook_long();
    // A step of 1 on a take of ~101.9: the take's band spans sums of about 1.7 to 4.9.
    d.tick = Some(1.0);
    d.sell_price = 102.0;
    let take = take_reading(&d, &p, None).expect("a take band");
    assert!(take.low < 2.0 && take.high > 4.0);
    // A stop printed at −2.6 % on a fine grid: Σ = 3 give or take a hair.
    let fine = Deal {
        tick: Some(0.01),
        ..d.clone()
    };
    let stop = Reading::of(&fine, 97.4, 0.2, |level| {
        (-2.0 - (level / 100.0 - 1.0) * 100.0) / 0.2
    })
    .expect("a stop band");
    let both = take.overlap(stop).expect("the bands meet");
    assert!(both.low > 2.9 && both.high < 3.1);
    // Bands that do not meet give nothing, and the take's is kept.
    let apart = Reading {
        low: 10.0,
        high: 11.0,
    };
    assert_eq!(take.overlap(apart), None);
}

/// A sum read at `MaxModifier` says only that the core's reached the cap: the miss kept is what
/// lifts the model's sum to it, never the cap's cut.
#[test]
fn a_sum_read_at_the_cap_keeps_only_its_lower_bound() {
    let (mut d, mut p) = hook_long();
    p.max_modifier = 2.9;
    d.fact_modifier = FactModifier::of(&d, &p, None);
    assert!((modifier_sum(&p, &d, d.buy_ms) - 2.9).abs() < 1e-9);
    let uncapped = ExitParams {
        max_modifier: 0.0,
        ..p
    };
    assert!((modifier_sum(&uncapped, &d, d.buy_ms) - 2.9).abs() < 1e-9);
}

/// A hook's take is placed off the depth its stated take implies, not the comment's `Depth`,
/// which the core writes at the close.
#[test]
fn the_hook_depth_is_read_off_the_stated_take() {
    use crate::db::tuner::ticks::record::placed_hook_depth;
    let (mut d, p) = hook_long();
    d.hook_depth_pct = Some(2.0);
    d.hook_stated_take_pct = Some(1.4);
    assert!(placed_hook_depth(&d, &p).is_some_and(|depth| (depth - 2.8).abs() < 1e-9));
    let fixed = ExitParams {
        hook_sell_fixed: true,
        ..p.clone()
    };
    assert_eq!(placed_hook_depth(&d, &fixed), None);
    d.hook_stated_take_pct = None;
    assert_eq!(placed_hook_depth(&d, &p), None);
}
