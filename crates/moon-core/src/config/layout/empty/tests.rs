//! Placement regressions for the empty Main screen: the shipped arrangement, what a saved choice
//! does to it, what an unreadable file falls back to, and what a reset leaves behind.

use super::*;
use crate::config::layout::WindowLayout;

/// The blocks must open exactly where the terminal has always drawn them. A default that moved one
/// of them would rearrange every existing profile on its next launch without anybody asking.
#[test]
fn a_profile_that_has_never_chosen_gets_the_screen_the_terminal_always_had() {
    let places = EmptyPlaces::restore(&WindowLayout::default());

    assert_eq!(places.slot(EmptyBlock::Logo), EmptySlot::MiddleCenter);
    assert_eq!(places.slot(EmptyBlock::Hint), EmptySlot::MiddleCenter);
    assert_eq!(places.slot(EmptyBlock::Minute), EmptySlot::TopEnd);
    assert_eq!(places.slot(EmptyBlock::Traders), EmptySlot::TopStart);
    assert_eq!(places.slot(EmptyBlock::Coins), EmptySlot::BottomEnd);
    assert_eq!(
        places,
        EmptyPlaces::default(),
        "an empty layout and the shipped arrangement must be the same screen"
    );
}

/// The brand and its hint share the middle cell on the shipped arrangement, and the mark must stack
/// ABOVE the line. Reversing [`EmptyBlock::ALL`] would draw a hint with a logo under it.
#[test]
fn the_brand_stacks_above_the_line_it_shares_a_cell_with() {
    let places = EmptyPlaces::default();
    let stacked: Vec<_> = places.blocks_in(EmptySlot::MiddleCenter).collect();

    assert_eq!(
        stacked,
        vec![EmptyBlock::Logo, EmptyBlock::Hint],
        "the middle cell must read as a mark with a line under it"
    );
}

/// One saved anchor moves ONE block. A restore that recomputed the others from it would move a
/// block nobody touched.
#[test]
fn a_saved_anchor_moves_only_its_own_block() {
    let mut layout = WindowLayout::default();
    EmptyBlock::Traders.store(&mut layout, Some(EmptySlot::BottomStart));

    let places = EmptyPlaces::restore(&layout);
    assert_eq!(places.slot(EmptyBlock::Traders), EmptySlot::BottomStart);
    assert_eq!(places.slot(EmptyBlock::Minute), EmptySlot::TopEnd);
    assert_eq!(places.slot(EmptyBlock::Coins), EmptySlot::BottomEnd);
    assert_eq!(places.slot(EmptyBlock::Logo), EmptySlot::MiddleCenter);
}

/// Two blocks in one anchor is a legal arrangement, not a clash: the drawing stacks them. Anything
/// that "repaired" it by moving one of them away would contradict the shipped middle cell, where
/// two blocks share an anchor by design.
#[test]
fn two_blocks_may_share_one_anchor_and_stack_in_block_order() {
    let mut layout = WindowLayout::default();
    EmptyBlock::Minute.store(&mut layout, Some(EmptySlot::BottomEnd));

    let places = EmptyPlaces::restore(&layout);
    assert_eq!(places.slot(EmptyBlock::Minute), EmptySlot::BottomEnd);
    assert_eq!(
        places.slot(EmptyBlock::Coins),
        EmptySlot::BottomEnd,
        "the block that was already there must not be pushed anywhere"
    );
    assert_eq!(
        places.blocks_in(EmptySlot::BottomEnd).collect::<Vec<_>>(),
        vec![EmptyBlock::Minute, EmptyBlock::Coins],
        "the stack must follow the fixed block order, not the order they were chosen in"
    );
}

/// A value this build does not recognise must cost the block its choice and NOTHING else — not the
/// other four blocks, and not the window layout the key is stored beside.
#[test]
fn an_unreadable_anchor_falls_back_to_the_blocks_own_default() {
    let file = "\
main_empty_place_minute = \"somewhere-else\"
main_empty_place_coins = 7
main_empty_place_traders = \"bottom-start\"
";
    let layout: WindowLayout = toml::from_str(file).expect("a bad anchor must not fail the layout");

    let places = EmptyPlaces::restore(&layout);
    assert_eq!(
        places.slot(EmptyBlock::Minute),
        EmptySlot::TopEnd,
        "an unknown anchor name must fall back rather than persist"
    );
    assert_eq!(
        places.slot(EmptyBlock::Coins),
        EmptySlot::BottomEnd,
        "an anchor of the wrong TYPE must fall back too"
    );
    assert_eq!(
        places.slot(EmptyBlock::Traders),
        EmptySlot::BottomStart,
        "a readable anchor beside an unreadable one must survive"
    );
}

/// A reset must CLEAR the keys rather than write today's defaults into them: a profile that has
/// been reset is one that has never chosen, which is what lets a later change of default reach it.
#[test]
fn a_reset_forgets_the_choice_rather_than_writing_the_default() {
    let mut layout = WindowLayout::default();
    for block in EmptyBlock::ALL {
        block.store(&mut layout, Some(EmptySlot::BottomCenter));
    }

    EmptyPlaces::reset(&mut layout);

    for block in EmptyBlock::ALL {
        assert_eq!(
            block.saved(&layout),
            None,
            "{block:?} kept an explicit anchor after a reset"
        );
    }
    assert_eq!(EmptyPlaces::restore(&layout), EmptyPlaces::default());
}

/// The nine anchors must be a complete three-by-three grid, read row by row. The narrow single
/// column is drawn in exactly this order, so a hole or a repeat here silently reorders that column.
#[test]
fn the_anchors_are_a_complete_grid_in_reading_order() {
    for (index, slot) in EmptySlot::ALL.into_iter().enumerate() {
        let row = (index / 3) as u8;
        let column = (index % 3) as u8;
        assert_eq!(slot.row(), row, "{slot:?} is not in row {row}");
        assert_eq!(slot.column(), column, "{slot:?} is not in column {column}");
        assert_eq!(EmptySlot::at(row, column), Some(slot));
    }
    assert_eq!(EmptySlot::at(3, 0), None);
    assert_eq!(EmptySlot::at(0, 3), None);
}

/// Every block must be reachable through the same table the popup builds its rows from. A block
/// missing from [`EmptyBlock::ALL`] would be drawn but unmovable, with no control to place it.
#[test]
fn every_block_is_listed_once_and_carries_its_own_key() {
    assert_eq!(EmptyBlock::ALL.len(), EmptyBlock::COUNT);

    for block in EmptyBlock::ALL {
        let mut layout = WindowLayout::default();
        block.store(&mut layout, Some(EmptySlot::MiddleStart));

        for other in EmptyBlock::ALL {
            let expected = (other == block).then_some(EmptySlot::MiddleStart);
            assert_eq!(
                other.saved(&layout),
                expected,
                "{block:?} and {other:?} share one persisted key"
            );
        }
    }
}
