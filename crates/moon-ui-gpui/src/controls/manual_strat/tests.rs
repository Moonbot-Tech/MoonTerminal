//! Unit tests for manual-strategy picker visibility and ordering.

use moon_core::feed::StrategyRow;

use super::manual_strategy_options;
use crate::backend::MANUAL_STRATEGY_KIND;

/// Build a strategy row carrying only the fields used by the picker-option filter.
fn strategy(id: u64, name: &str, kind_ordinal: u8) -> StrategyRow {
    StrategyRow {
        id,
        name: name.to_string(),
        kind: "Test".to_string(),
        kind_ordinal,
        folder_path: String::new(),
        checked: false,
        is_short: false,
        fields: Vec::new(),
    }
}

/// Returning `Some(Vec::new())` would render the header control and an empty popover.
#[test]
fn an_empty_snapshot_hides_the_manual_strategy_control() {
    assert!(manual_strategy_options(&[]).is_none());
}

/// Removing the Manual-kind filter would show the control for a core with only other strategies.
#[test]
fn non_manual_strategies_do_not_make_the_control_visible() {
    let strategies = [strategy(7, "Auto", MANUAL_STRATEGY_KIND - 1)];

    assert!(manual_strategy_options(&strategies).is_none());
}

/// Sorting or retaining other kinds would change the existing picker order and contents.
#[test]
fn manual_strategy_options_preserve_snapshot_order() {
    let strategies = [
        strategy(7, "Manual B", MANUAL_STRATEGY_KIND),
        strategy(8, "Auto", MANUAL_STRATEGY_KIND - 1),
        strategy(9, "Manual A", MANUAL_STRATEGY_KIND),
    ];

    assert_eq!(
        manual_strategy_options(&strategies),
        Some(vec![
            (7, "Manual B".to_string()),
            (9, "Manual A".to_string()),
        ])
    );
}

/// #1191: a full row of names that fits the window by itself must yield once the update cluster's
/// width joins the base, and stop short of it — fewer or number-only buttons, never the same row.
/// Without an offered update (reservation 0) the ladder answers exactly as before.
#[test]
fn update_reservation_stops_a_full_row_short_of_the_button() {
    use super::fit::{LabelMode, SlotWidths, resolve_strat_fit};

    let slots = [SlotWidths {
        name_only: 70.0,
        number_only: 26.0,
    }; 10];
    let (gap, base, width) = (4.0, 400.0, 1150.0);
    let row = |fit: super::fit::StratFit| {
        let each = |s: &SlotWidths| match fit.label_mode {
            LabelMode::NameOnly => s.name_only,
            LabelMode::NumberOnly => s.number_only,
        };
        let n = fit.visible_count;
        slots[..n].iter().map(each).sum::<f32>() + gap * n.saturating_sub(1) as f32
    };

    let before = resolve_strat_fit(width, gap, &slots, None, base);
    assert_eq!(before.label_mode, LabelMode::NameOnly);
    assert_eq!(before.visible_count, 10);
    assert_eq!(
        resolve_strat_fit(width, gap, &slots, None, base + 0.0),
        before
    );

    for reserve in [185.0, 240.0] {
        let after = resolve_strat_fit(width, gap, &slots, None, base + reserve);
        assert_ne!(after, before, "reserve {reserve} must shrink the row");
        assert!(
            base + row(after) + reserve <= width,
            "reserve {reserve}: names run under the update button"
        );
    }
}
