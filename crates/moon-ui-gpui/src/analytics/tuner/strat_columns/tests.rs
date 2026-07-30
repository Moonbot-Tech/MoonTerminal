use super::super::columns::{MetricCol, COL_PROFIT, COL_TRADES, COL_WINRATE};
use super::{
    metric_bit, COL_BIT_CORE, COL_BIT_KIND, COL_BIT_LASTEDIT, CORE_MIN_W, KIND_MIN_W,
    LASTEDIT_MIN_W, METRIC_COLS, STRAT_COLS_ALL, STRAT_COLS_DEFAULT, STRAT_COLS_DEFAULT_COINS,
    STRAT_MIN_PANEL_W, STRAT_NAME_MIN_W,
};

/// What the row spends outside the metric descriptors AT ITS FLOOR: the two identity
/// columns, the alive dot and its gap, the row padding, and the gap to the cluster.
///
/// Floors, not preferred widths: every column shrinks toward its own floor before the row
/// overflows, so "does this fit" is a question about the minimum layout.
fn fixed_extras(mask: u16) -> f32 {
    let mut w = 8.0 * 2.0 + (6.0 + 6.0) + 8.0;
    if mask & COL_BIT_KIND != 0 {
        w += KIND_MIN_W + 8.0;
    }
    if mask & COL_BIT_CORE != 0 {
        // The core column's PREFERRED width is content-measured, but "does this fit"
        // is a question about floors, and its floor is fixed.
        w += CORE_MIN_W + 8.0;
    }
    if mask & COL_BIT_LASTEDIT != 0 {
        w += LASTEDIT_MIN_W + 8.0;
    }
    w
}

/// What the whole column cluster costs at its floor under the given mask.
fn fixed_total(mask: u16) -> f32 {
    let metrics: f32 = METRIC_COLS
        .iter()
        .enumerate()
        .filter(|(i, _)| mask & metric_bit(*i) != 0)
        .map(|(_, c)| c.min_w + 8.0)
        .sum();
    fixed_extras(mask) + metrics
}

/// Every column shrinks toward its own floor, and the name has a floor of its own, so the
/// table fits exactly while the floors plus the name floor fit the panel. The default has
/// to satisfy that; the full mask is allowed not to (it overflows, visibly, on purpose).
#[test]
fn strat_columns_default_fits() {
    // Both defaults, or the axis that adds two columns is the one nobody checked.
    for (label, mask) in [
        ("filter/time", STRAT_COLS_DEFAULT),
        ("coins", STRAT_COLS_DEFAULT_COINS),
    ] {
        let left = STRAT_MIN_PANEL_W - fixed_total(mask);
        assert!(
            left >= STRAT_NAME_MIN_W,
            "{label} default leaves the name {left}px, below its {STRAT_NAME_MIN_W}px floor"
        );
    }
    // If even the full mask fits at its floors, the reduced default is pointless.
    assert!(
        STRAT_MIN_PANEL_W - fixed_total(STRAT_COLS_ALL) < STRAT_NAME_MIN_W,
        "the full mask now fits; make it the default instead of keeping a reduced one"
    );
}

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
