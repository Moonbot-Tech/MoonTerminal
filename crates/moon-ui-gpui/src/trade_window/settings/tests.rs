// NOT `use super::*`: the parent imports `gpui::*`, whose `test` macro shadows `#[test]`.
use moon_core::market::candles::{
    CANDLE_MODE_FILLED, CANDLE_MODE_OFF, CANDLE_MODE_OUTLINE, CANDLE_MODE_OUTLINE_IN_ZONE,
    CandleViewCfg,
};

use super::{
    PINNED_TF_MIN, TRADE_WINDOW_MODES, candles_for_default, pinned_candle_view,
    trade_window_mode_segment,
};

fn user(mode: u8, tf_min: u32) -> CandleViewCfg {
    CandleViewCfg {
        tf_min,
        mode,
        outline_px: 2.0,
        trade_candles: 5,
        wicks_in_zone: false,
        ..CandleViewCfg::default()
    }
}

/// The pin is unconditional: whatever timeframe the user's chart is on, the window draws minutes.
///
/// Breakage: a coarser bucket resamples the minute rows under a caption that still names minutes —
/// the bug the pin was written for.
#[test]
fn the_timeframe_is_pinned_whatever_the_user_chose() {
    for tf in [1, 5, 30, 240] {
        for ticks in [false, true] {
            assert_eq!(
                pinned_candle_view(user(CANDLE_MODE_FILLED, tf), ticks).tf_min,
                PINNED_TF_MIN
            );
        }
    }
}

/// Mode Off stands only while ticks are on screen; before that the shipped mode draws instead,
/// and every other mode is left exactly as picked.
///
/// Breakage: honouring Off on a candle replay draws an empty pane under a caption naming
/// candles; forcing it once ticks are there takes the user's pure tick chart away.
#[test]
fn off_is_replaced_until_ticks_arrive_and_other_modes_pass_through() {
    let shipped = CandleViewCfg::default().mode;
    assert_ne!(
        shipped, CANDLE_MODE_OFF,
        "the shipped mode must draw candles"
    );
    assert_eq!(
        pinned_candle_view(user(CANDLE_MODE_OFF, 1), false).mode,
        shipped
    );
    assert_eq!(
        pinned_candle_view(user(CANDLE_MODE_OFF, 1), true).mode,
        CANDLE_MODE_OFF
    );
    for mode in [
        CANDLE_MODE_FILLED,
        CANDLE_MODE_OUTLINE,
        CANDLE_MODE_OUTLINE_IN_ZONE,
    ] {
        for ticks in [false, true] {
            assert_eq!(pinned_candle_view(user(mode, 1), ticks).mode, mode);
        }
    }
}

/// The pin touches nothing but the two fields it is about.
#[test]
fn the_pin_leaves_the_rest_of_the_choice_alone() {
    let pinned = pinned_candle_view(user(CANDLE_MODE_OUTLINE, 30), false);
    let expected = user(CANDLE_MODE_OUTLINE, PINNED_TF_MIN);
    assert_eq!(pinned, expected);
}

/// A candle row never writes the pinned minute into the stored set: it keeps the timeframe the
/// trade-window kind already carried.
///
/// Breakage: the stored set would silently become a one-minute one, and a main-chart ⧉ press
/// that later addressed the kind would overwrite it — the two paths disagreeing about what the
/// setting is.
#[test]
fn the_stored_default_keeps_the_kinds_own_timeframe() {
    let shown = user(CANDLE_MODE_OFF, PINNED_TF_MIN);
    let stored = candles_for_default(shown, 5);
    assert_eq!(stored.tf_min, 5);
    assert_eq!(
        stored.mode, CANDLE_MODE_OFF,
        "the user's choice, not the forced one"
    );
    assert_eq!(
        CandleViewCfg {
            tf_min: PINNED_TF_MIN,
            ..stored
        },
        shown,
        "nothing but the timeframe differs"
    );
}

/// The popup offers Off, Filled and Outline — and not "In zone", whose zone the frozen replay
/// never reaches; a stored "In zone" lights "Filled", which is what it draws as here.
///
/// Breakage: offering "In zone" gives a segment that changes nothing; lighting nothing for a
/// stored "In zone" tells the reader the window is in no mode at all.
#[test]
fn the_mode_row_offers_off_but_not_in_zone_and_maps_in_zone_to_filled() {
    assert_eq!(
        TRADE_WINDOW_MODES,
        [CANDLE_MODE_OFF, CANDLE_MODE_FILLED, CANDLE_MODE_OUTLINE]
    );
    assert!(!TRADE_WINDOW_MODES.contains(&CANDLE_MODE_OUTLINE_IN_ZONE));
    assert_eq!(
        trade_window_mode_segment(CANDLE_MODE_OUTLINE_IN_ZONE),
        CANDLE_MODE_FILLED
    );
    for mode in TRADE_WINDOW_MODES {
        assert_eq!(trade_window_mode_segment(mode), mode);
    }
    assert_eq!(
        trade_window_mode_segment(200),
        CANDLE_MODE_OFF,
        "unknown reads as Off"
    );
}
