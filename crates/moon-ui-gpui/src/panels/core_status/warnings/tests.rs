//! Duration text for an alarms row.
//!
//! Explicit imports (no `use super::*`): the parent glob-imports `gpui`.

use super::dur_text;

/// An open episode is the locale word alone. The closed arm is a different string.
#[test]
fn dur_text_ongoing_is_the_word_idjot() {
    let _locale = crate::test_locale::force("ru");
    let ongoing = dur_text(1_700_000_000_000, None, 1_700_000_240_000);
    assert_eq!(ongoing, "идёт");
    let closed = dur_text(
        1_700_000_000_000,
        Some(1_700_000_240_000),
        1_700_000_240_000,
    );
    assert_ne!(closed, ongoing);
}
