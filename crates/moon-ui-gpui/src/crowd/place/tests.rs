//! Frame regressions that need no window: which block an anchor claims, and how often.

use super::*;

/// A collapsed grid at the reported Main width must retain boards, while a narrow dock uses records.
#[test]
fn one_column_does_not_imply_compact_boards() {
    let thresholds = [px(410.0), px(410.0), px(490.0)];
    let needs = ColumnNeeds {
        start: px(454.0),
        center: px(376.0),
        end: px(454.0),
        fixed: px(60.0),
    };
    assert_eq!(needs.form(px(1150.0)), Form::OneColumn);
    assert_eq!(board_modes(px(1150.0), thresholds), 0);
    assert_eq!(board_modes(px(320.0), thresholds), 0b111);
    // The trader board alone needs records here; selecting one format for every board is wrong.
    assert_eq!(board_modes(px(450.0), thresholds), 0b100);
    assert_eq!(board_modes(px(490.0), thresholds), 0);
    assert_eq!(board_modes(px(0.0), thresholds), 0);
}

/// A top-left trader stack must never enter the center column's vertical sizing inputs.
#[test]
fn trader_and_brand_have_independent_vertical_flows() {
    let places = EmptyPlaces::default();
    let columns: Vec<Vec<Vec<EmptyBlock>>> = (0..3)
        .map(|column| {
            column_slots(column)
                .map(|slot| places.blocks_in(slot).collect())
                .collect()
        })
        .collect();
    assert_eq!(columns[0], vec![vec![EmptyBlock::Traders], vec![], vec![]]);
    assert_eq!(
        columns[1],
        vec![vec![], vec![EmptyBlock::Logo, EmptyBlock::Hint], vec![]]
    );
    assert_eq!(
        columns[2],
        vec![vec![EmptyBlock::Minute], vec![], vec![EmptyBlock::Coins]]
    );

    let mut column = column_flow(Vec::new(), None, px(12.0), px(40.0), px(18.0));
    let style = column.style();
    assert_eq!(style.flex_direction, Some(gpui::FlexDirection::Column));
    assert_eq!(style.min_size.height, Some(gpui::relative(1.0).into()));
    assert_eq!(
        style.size.height, None,
        "intrinsic content must be allowed to exceed the viewport"
    );
}

/// A side column is exactly as wide as the grid says, and the middle one takes what is left: equal
/// thirds would hand a 330-pixel board a third of the panel and make the collapse three boards wide.
#[test]
fn side_columns_take_their_width_and_the_middle_takes_the_rest() {
    let mut side = column_flow(Vec::new(), Some(px(330.0)), px(12.0), px(40.0), px(18.0));
    let style = side.style();
    assert_eq!(style.size.width, Some(px(330.0).into()));
    assert_eq!(style.flex_grow, Some(0.0));
    assert_eq!(style.flex_shrink, Some(0.0));

    let mut middle = column_flow(Vec::new(), None, px(12.0), px(40.0), px(18.0));
    let style = middle.style();
    assert_eq!(style.size.width, None);
    assert_eq!(style.flex_grow, Some(1.0));
}

/// Top and bottom retain their content height while only the middle consumes spare column space.
#[test]
fn column_anchors_do_not_share_or_shrink_their_heights() {
    for row in [0, 2] {
        let mut anchor = cell(
            row,
            0,
            vec![div().h(px(1200.0)).into_any_element()],
            px(12.0),
        );
        let style = anchor.style();
        assert_eq!(style.flex_shrink, Some(0.0));
        assert_eq!(style.flex_grow, None);
        assert_eq!(style.size.height, None);
    }
}

/// Three fitting columns still need two gaps and two insets; omitting them clips the last column
/// at the switch.
#[test]
fn grid_threshold_budgets_every_gap_and_inset() {
    let needs = ColumnNeeds {
        start: px(300.0),
        center: px(300.0),
        end: px(300.0),
        fixed: px(60.0),
    };
    assert_eq!(needs.narrow_below(), px(960.0));
    assert_eq!(needs.form(px(959.0)), Form::OneColumn);
    assert_eq!(needs.form(px(960.0)), Form::Grid { start: px(300.0), end: px(300.0) });
}

/// A short viewport must scroll a tall middle stack instead of shrinking that band to zero.
#[test]
fn middle_band_keeps_its_intrinsic_height() {
    let mut middle = cell(1, 1, vec![div().h(px(600.0)).into_any_element()], px(12.0));
    let style = middle.style();
    assert_eq!(style.flex_shrink, Some(0.0));
    assert_eq!(style.flex_grow, Some(1.0));
    assert_eq!(style.flex_basis, None);
    assert_eq!(style.min_size.height, None);
}

/// Build the list [`frame`] works from, one plain element per block.
fn pending(blocks: &[EmptyBlock]) -> Vec<(EmptyBlock, Option<AnyElement>)> {
    blocks
        .iter()
        .map(|block| (*block, Some(div().into_any_element())))
        .collect()
}

/// Every cell asks for the blocks anchored in it, and a block anchored in one cell must not also be
/// drawn in another. Handing the same element out twice would put one live table on the screen
/// twice, reading one board through two sets of rows.
#[test]
fn a_block_is_claimed_once_and_then_gone() {
    let mut list = pending(&[EmptyBlock::Logo, EmptyBlock::Minute]);

    assert!(
        take(&mut list, EmptyBlock::Logo).is_some(),
        "the first cell to ask for a block must get it"
    );
    assert!(
        take(&mut list, EmptyBlock::Logo).is_none(),
        "a second cell must not be handed the same element again"
    );
    assert!(take(&mut list, EmptyBlock::Minute).is_some());
}

/// A block that is switched OFF is simply absent from the list, and asking for it must be an
/// ordinary empty answer rather than a panic or an empty element taking up a cell.
#[test]
fn a_block_that_is_switched_off_is_never_placed() {
    let mut list = pending(&[EmptyBlock::Hint]);

    for block in EmptyBlock::ALL {
        let claimed = take(&mut list, block).is_some();
        assert_eq!(
            claimed,
            block == EmptyBlock::Hint,
            "{block:?} was placed although it was not handed in"
        );
    }
}

/// The user's report: the minute board top-left and the coin board bottom-right, the trader board
/// switched OFF, on a panel of 1100 logical pixels — and everything drawn down the middle. The
/// threshold was three times the widest board there is, not the width of what is on the screen.
#[test]
fn two_boards_on_a_laptop_panel_are_a_grid_not_a_column() {
    let needs = ColumnNeeds {
        start: px(330.0),
        center: px(0.0),
        end: px(330.0),
        fixed: px(60.0),
    };
    assert_eq!(needs.narrow_below(), px(720.0));
    assert!(px(1100.0) >= needs.narrow_below());
    assert_eq!(needs.form(px(1100.0)), Form::Grid { start: px(330.0), end: px(330.0) });
}

/// The shipped arrangement keeps the brand in the true middle while the panel affords it: both side
/// columns take the wider side's width, so the center column is centered on the screen.
#[test]
fn the_grid_is_symmetric_while_it_can_be_and_asymmetric_before_it_collapses() {
    let needs = ColumnNeeds {
        start: px(454.0),
        center: px(376.0),
        end: px(330.0),
        fixed: px(60.0),
    };
    // 454 + 376 + 330 + 60
    assert_eq!(needs.narrow_below(), px(1220.0));
    // 2 * 454 + 376 + 60 = 1344 fits: symmetric.
    assert_eq!(needs.form(px(1400.0)), Form::Grid { start: px(454.0), end: px(454.0) });
    // Between 1220 and 1344 the brand gives up its exact center rather than the whole grid.
    assert_eq!(needs.form(px(1300.0)), Form::Grid { start: px(454.0), end: px(330.0) });
    assert_eq!(needs.form(px(1219.0)), Form::OneColumn);
    // The unmeasured first frame is the grid, exactly as before.
    assert_eq!(needs.form(px(0.0)), Form::Grid { start: px(454.0), end: px(454.0) });
}

/// A column nobody put anything in costs nothing but its gap.
#[test]
fn an_empty_column_needs_no_width() {
    let needs = ColumnNeeds {
        start: px(0.0),
        center: px(376.0),
        end: px(0.0),
        fixed: px(60.0),
    };
    assert_eq!(needs.narrow_below(), px(436.0));
    assert_eq!(needs.form(px(500.0)), Form::Grid { start: px(0.0), end: px(0.0) });
}
