use std::time::Duration;

use super::{count_text, duration_text};

#[test]
fn a_count_keeps_its_thousands_apart() {
    assert_eq!(count_text(0.0), "0");
    assert_eq!(count_text(999.4), "999");
    assert_eq!(count_text(1000.0), "1\u{202f}000");
    assert_eq!(count_text(4_456_380.0), "4\u{202f}456\u{202f}380");
}

#[test]
fn a_duration_reads_in_the_largest_unit_it_needs() {
    let _locale = crate::test_locale::force("en");
    assert_eq!(duration_text(Duration::from_millis(200)), "1 s");
    assert_eq!(duration_text(Duration::from_secs(59)), "59 s");
    assert_eq!(duration_text(Duration::from_secs(61)), "2 min");
    assert_eq!(
        duration_text(Duration::from_secs(3 * 3600 + 5 * 60)),
        "3 h 5 min"
    );
}
