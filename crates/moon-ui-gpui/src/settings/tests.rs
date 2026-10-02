//! Regression coverage for global chart geometry settings in the save indicator.

use super::draft_dirty;
use moon_core::config::AppConfig;

/// Catches dropping width from the draft signature, which would disable Save for a width-only edit.
#[test]
fn global_book_width_edit_marks_settings_dirty_and_reverting_clears_it() {
    let saved = AppConfig::load(None, false).expect("test config");
    let mut draft = saved.clone();
    draft.order_book_width_px = if saved.order_book_width_px == 340.0 {
        360.0
    } else {
        340.0
    };
    assert!(draft_dirty(&saved, &draft));
    assert_eq!(saved.structural_sig(), draft.structural_sig());
    draft.order_book_width_px = saved.order_book_width_px;
    assert!(!draft_dirty(&saved, &draft));
}
