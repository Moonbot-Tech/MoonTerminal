use super::super::super::strat_columns::METRIC_COLS;
use super::{
    COIN_COLS, COIN_MIN_PANEL_W, COIN_NAME_MIN_W, COIN_ROW_GAP, COIN_ROW_PAD_X, COIN_TICKS_W,
};

/// Both tables sit on the same screen in "Coins" mode, so a metric present in both has to
/// keep the same relative position — otherwise the position cue learned on one misreads the
/// other.
#[test]
fn coin_columns_keep_the_strategy_order() {
    let shared: Vec<&str> = METRIC_COLS
        .iter()
        .map(|c| c.key)
        .filter(|k| COIN_COLS.iter().any(|c| c.key == *k))
        .collect();
    let coin_order: Vec<&str> = COIN_COLS.iter().map(|c| c.key).collect();
    assert_eq!(shared, coin_order);
}

/// The coin table's name column is the residual of its width, so widening a shared
/// descriptor for the roomier strategy table silently eats the coin name here. Measured at
/// the column FLOORS — every column shrinks toward its own before the row overflows — at
/// the narrowest layout the panel is expected to work at, counting the two trailing tick
/// columns the descriptors know nothing about.
#[test]
fn coin_columns_leave_room_for_the_name() {
    let numbers: f32 = COIN_COLS.iter().map(|c| c.min_w).sum();
    // Two gaps beyond the metric columns: one before each trailing tick column.
    let gaps = COIN_ROW_GAP * (COIN_COLS.len() + 2) as f32;
    let name_w = COIN_MIN_PANEL_W - COIN_ROW_PAD_X * 2.0 - gaps - COIN_TICKS_W - numbers;
    assert!(
        name_w >= COIN_NAME_MIN_W,
        "coin name column squeezed to {name_w}px by the shared descriptors"
    );
}
