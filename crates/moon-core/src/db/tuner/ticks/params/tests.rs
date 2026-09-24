//! The knobs' grids.

use super::*;

/// The corridor's grids step by 0.05 to 8, and hold a strategy's own spellings exactly: 1.7 is a
/// step, spelled back as `1.7`, not snapped to 1.75.
#[test]
fn the_corridor_grids_step_by_a_twentieth_to_eight() {
    for key in ["MShotPrice", "MShotPriceMin"] {
        let field = TICK_PARAMS.iter().find(|f| f.key == key).expect("a knob");
        let ParamKind::Num { grid } = &field.kind else {
            panic!("{key} is a number");
        };
        assert_eq!(grid.len(), 160, "{key}");
        assert_eq!(grid.first().copied(), Some(0.05));
        assert_eq!(grid.last().copied(), Some(8.0));
        assert!(grid.contains(&1.7) && grid.contains(&1.2), "{key}");
        assert_eq!(format!("{}", grid[33]), "1.7");
    }
}
