//! Regressions for the popup's placement rows: every block and every anchor must be NAMED, and
//! named differently.

use std::collections::HashSet;

use rust_i18n::t;

use super::*;

/// A dropdown selects by index into [`EmptySlot::ALL`], so the index and the anchor must agree in
/// both directions. A mismatch shows a control naming one place while the block is drawn in
/// another, with nothing in the code that looks wrong.
#[test]
fn a_dropdowns_index_and_its_anchor_are_the_same_thing() {
    for (index, slot) in EmptySlot::ALL.into_iter().enumerate() {
        assert_eq!(slot_index(slot), index, "{slot:?} selects the wrong row");
    }
}

/// Nine anchors, nine different names. A copy-paste in the match would print two anchors
/// identically, and a list with the same words twice cannot be chosen from.
#[test]
fn every_anchor_reads_differently_in_every_language() {
    for locale in ["ru", "en", "es"] {
        let _locale = crate::test_locale::force(locale);
        let mut seen = HashSet::new();
        for slot in EmptySlot::ALL {
            let label = t!(slot_label(slot)).to_string();
            assert!(
                !label.is_empty() && !label.starts_with("crowd.slot."),
                "{slot:?} has no {locale} name"
            );
            assert!(
                seen.insert(label.clone()),
                "{locale} calls two anchors {label:?}"
            );
        }
    }
}

/// The same for the five blocks: the rows are read as "this block goes there", so two rows with
/// one caption is a control nobody can aim.
#[test]
fn every_block_reads_differently_in_every_language() {
    for locale in ["ru", "en", "es"] {
        let _locale = crate::test_locale::force(locale);
        let mut seen = HashSet::new();
        for block in EmptyBlock::ALL {
            let label = t!(block_label(block)).to_string();
            assert!(
                !label.is_empty() && !label.starts_with("crowd.block."),
                "{block:?} has no {locale} name"
            );
            assert!(
                seen.insert(label.clone()),
                "{locale} calls two blocks {label:?}"
            );
        }
    }
}
