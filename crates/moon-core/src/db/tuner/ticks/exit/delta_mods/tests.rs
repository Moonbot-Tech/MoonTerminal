//! The Delta Modifiers tab's sum as the core forms it (the core developer via LinKvo, 2026-09-24).

use super::*;
use crate::db::tuner::ticks::exit::tests::deal;
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
