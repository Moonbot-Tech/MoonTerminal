//! Frame regressions that need no window: which block an anchor claims, and how often.

use super::*;

/// A collapsed grid at the reported Main width must retain boards, while a narrow dock uses records.
#[test]
fn one_column_does_not_imply_compact_boards() {
    let thresholds = [px(410.0), px(410.0), px(490.0)];
    let grid = grid_width(px(454.0), px(18.0), px(12.0));
    assert!(px(1150.0) < grid);
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

    let mut column = column_flow(Vec::new(), px(12.0), px(40.0), px(18.0));
    let style = column.style();
    assert_eq!(style.flex_direction, Some(gpui::FlexDirection::Column));
    assert_eq!(style.min_size.height, Some(gpui::relative(1.0).into()));
    assert_eq!(
        style.size.height, None,
        "intrinsic content must be allowed to exceed the viewport"
    );
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

/// Three fitting boards still need two gaps; omitting them clips the last columns at the switch.
#[test]
fn grid_threshold_budgets_every_gap_and_inset() {
    assert_eq!(grid_width(px(300.0), px(18.0), px(12.0)), px(960.0));
    assert_eq!(grid_width(px(450.0), px(27.0), px(18.0)), px(1440.0));
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
