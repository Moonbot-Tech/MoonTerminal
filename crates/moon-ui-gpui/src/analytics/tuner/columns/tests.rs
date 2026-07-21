use super::{
    COIN_COLS, COIN_PANEL_W, COIN_ROW_GAP, COIN_ROW_PAD_X, COL_PROFIT, COL_TRADES, COL_WINRATE,
    METRIC_COLS, MetricCol,
};

fn position(cols: &[MetricCol], key: &str) -> usize {
    cols.iter()
        .position(|c| c.key == key)
        .unwrap_or_else(|| panic!("column {key} missing"))
}

/// The tables are sorted by profit descending (SQL-side, `db::analytics`). The sort key has
/// to lead the numbers: buried mid-row it leaves the ranking without an anchor, and winrate
/// ahead of it presents a 100%-on-two-trades row as the top performer.
#[test]
fn profit_anchors_the_numeric_columns() {
    assert_eq!(METRIC_COLS[0].key, COL_TRADES.key, "trade count leads");
    assert_eq!(METRIC_COLS[1].key, COL_PROFIT.key, "profit follows it");
    assert!(
        position(METRIC_COLS, COL_PROFIT.key) < position(METRIC_COLS, COL_WINRATE.key),
        "profit must precede winrate"
    );
    // COIN_COLS inherits the ordering via `coin_columns_keep_the_strategy_order`.
}

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

/// The coin panel is a fixed-width box and its name column is the residual, so widening a
/// shared descriptor for the roomier strategy table silently eats the coin name here.
#[test]
fn coin_columns_leave_room_for_the_name() {
    let numbers: f32 = COIN_COLS.iter().map(|c| c.w).sum();
    let name_w =
        COIN_PANEL_W - COIN_ROW_PAD_X * 2.0 - COIN_ROW_GAP * COIN_COLS.len() as f32 - numbers;
    assert!(
        name_w >= 70.0,
        "coin name column squeezed to {name_w}px by the shared descriptors"
    );
}
