use super::super::columns::{
    COL_AVG_ORDER, COL_PROFIT, COL_PROFIT_PCT, COL_TRADES, COL_WINRATE, MetricCol,
};
use super::{
    COL_BIT_CORE, COL_BIT_KIND, COL_BIT_LASTEDIT, CORE_MIN_W, KIND_MIN_W, LASTEDIT_MIN_W,
    METRIC_COLS, STRAT_COLS_ALL, STRAT_COLS_DEFAULT, STRAT_COLS_DEFAULT_COINS,
    STRAT_COLS_V3_ADDITIONS, STRAT_MIN_PANEL_W, STRAT_NAME_MIN_W, metric_bit,
    restore_strat_columns,
};
use moon_core::config::layout::StratColsByMode;

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

/// Moving `strat_columns.rs:METRIC_COLS` without moving its semantic bit map must fail here;
/// otherwise Profit % can appear to the right of Avg order or restore the user's Avg-order
/// visibility preference onto the wrong column.
#[test]
fn profit_percent_precedes_average_order_without_swapping_saved_bits() {
    let profit_pct = position(METRIC_COLS, COL_PROFIT_PCT.key);
    let avg_order = position(METRIC_COLS, COL_AVG_ORDER.key);

    assert_eq!(profit_pct + 1, avg_order, "Profit % sits left of Avg order");
    assert_eq!(metric_bit(profit_pct), 1 << 13, "Profit % keeps its bit");
    assert_eq!(metric_bit(avg_order), 1 << 12, "Avg order keeps its bit");
}

/// Reusing a historical mask without adding the two new semantic bits must fail this assertion;
/// moving the historical last-edit bit must fail it as well and would restore the wrong columns.
#[test]
fn legacy_masks_gain_new_metrics_without_reinterpreting_old_bits() {
    let previous = StratColsByMode {
        filter: COL_BIT_KIND | COL_BIT_LASTEDIT,
        coins: 0,
        time: COL_BIT_CORE,
    };
    let restored = restore_strat_columns(None, Some(previous), None);

    assert_eq!(
        restored.filter,
        COL_BIT_KIND | COL_BIT_LASTEDIT | STRAT_COLS_V3_ADDITIONS
    );
    assert_eq!(restored.coins, STRAT_COLS_V3_ADDITIONS);
    assert_eq!(restored.time, COL_BIT_CORE | STRAT_COLS_V3_ADDITIONS);
    assert_eq!(COL_BIT_LASTEDIT, 1 << 11, "historical bit remains stable");
    let hidden = restore_strat_columns(Some(StratColsByMode::default()), Some(previous), None);
    assert_eq!(hidden.filter, 0, "current masks preserve a deliberate hide");
}
