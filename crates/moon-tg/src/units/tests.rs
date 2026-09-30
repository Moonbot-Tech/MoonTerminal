use super::*;

/// Kilobytes, megabytes, gigabytes, each at its own precision and rounded to the nearest.
#[test]
fn sizes_read_in_their_unit() {
    {
        let _locale = crate::test_locale::force("en");
        assert_eq!(size_text(10), "0 KB");
        assert_eq!(size_text(1536), "2 KB");
        assert_eq!(size_text(609 * 1024 * 1024 + 300 * 1024), "609.3 MB");
        assert_eq!(size_text(24 * 1024 * 1024 * 1024 - 1), "24.00 GB");
    }
    // The locale lock is held per guard: the next one only after the first is gone.
    let _locale = crate::test_locale::force("ru");
    assert_eq!(size_text(1024 * 1024 * 1024), "1.00 ГБ");
}
