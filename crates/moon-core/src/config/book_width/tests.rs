use super::*;

/// Catches the carry-over reading the wrong key or dropping a hand-typed integer: a user who set
/// 340 in either spelling must keep 340 after the setting moved per tab.
#[test]
fn legacy_width_reads_the_retired_key_in_both_spellings() {
    assert_eq!(
        legacy_global_width_in("order_book_width_px = 340.0"),
        LegacyWidth::Stored(340.0)
    );
    assert_eq!(
        legacy_global_width_in("order_book_width_px = 340"),
        LegacyWidth::Stored(340.0)
    );
}

/// Catches a stray value reaching every tab unbounded: the carry-over writes what the old setting
/// DREW, and the old setting drew it clamped.
#[test]
fn legacy_width_is_normalized_like_the_old_setting() {
    assert_eq!(
        legacy_global_width_in("order_book_width_px = 9000.0"),
        LegacyWidth::Stored(MAX)
    );
    assert_eq!(
        legacy_global_width_in("order_book_width_px = nan"),
        LegacyWidth::Stored(DEFAULT)
    );
}

/// Catches a profile without the key, or a non-numeric value, being read as a stored width.
#[test]
fn legacy_width_is_absent_without_a_stored_number() {
    assert_eq!(
        legacy_global_width_in("ui_scale = 1.25"),
        LegacyWidth::Absent
    );
    assert_eq!(
        legacy_global_width_in("order_book_width_px = \"wide\""),
        LegacyWidth::Absent
    );
}

/// Catches a broken file being read as "no width": the key may still be in it, and the carry-over
/// must not commit its marker on that answer.
#[test]
fn a_file_that_is_not_toml_is_unreadable_not_absent() {
    assert_eq!(
        legacy_global_width_in("not toml ["),
        LegacyWidth::Unreadable
    );
}
