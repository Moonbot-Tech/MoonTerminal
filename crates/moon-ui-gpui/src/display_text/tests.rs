use super::{flatten_lines, fmt_ban_left, fmt_duration_short};

/// Single-line input is returned byte-for-byte.
#[test]
fn single_line_passes_through() {
    assert_eq!(flatten_lines("Sell Price"), "Sell Price");
    assert_eq!(flatten_lines(""), "");
}

/// Whitespace other than line breaks is preserved.
#[test]
fn ordinary_whitespace_is_preserved() {
    assert_eq!(flatten_lines("  at foo"), "  at foo");
    assert_eq!(flatten_lines("trailing   "), "trailing   ");
    assert_eq!(flatten_lines("   "), "   ");
    assert_eq!(flatten_lines("  a\r\nb  "), "  a ¶ b  ");
    // Non-breaking spaces verify that only CR and LF are trimmed.
    assert_eq!(
        flatten_lines("\u{00A0}value\u{00A0}"),
        "\u{00A0}value\u{00A0}"
    );
}

/// A CRLF-separated report comment folds to one line.
#[test]
fn crlf_comment_folds_to_one_line() {
    let raw = "MoonShot: (strategy <MOONSHOT_01>)\r\n CPU: Bot 6 (Avg: 3) Sys: 8\r\nLatency: 138 / 138  Ping: 11 / 12";
    let out = flatten_lines(raw);
    assert_eq!(
        out,
        "MoonShot: (strategy <MOONSHOT_01>) ¶  CPU: Bot 6 (Avg: 3) Sys: 8 ¶ Latency: 138 / 138  Ping: 11 / 12"
    );
}

/// Continuation indentation survives folding.
#[test]
fn interior_indentation_is_preserved() {
    assert_eq!(flatten_lines("err:\n  at foo"), "err: ¶   at foo");
}

/// Mixed and lone separators each produce one marker.
#[test]
fn mixed_separators_each_yield_one_break() {
    assert_eq!(flatten_lines("a\r\nb\nc\rd"), "a ¶ b ¶ c ¶ d");
}

/// Edge breaks are removed without producing dangling markers.
#[test]
fn edge_breaks_do_not_leave_a_dangling_marker() {
    assert_eq!(flatten_lines("\r\nalone\r\n"), "alone");
    assert_eq!(flatten_lines("\r\n"), "");
    assert_eq!(flatten_lines("trailing\r\n"), "trailing");
}

/// The result contains no raw CR or LF characters.
#[test]
fn result_never_contains_a_raw_break() {
    for raw in [
        "a\r\nb\r\nc",
        "\r\r\n\n",
        "  x\ry  ",
        "no breaks here",
        "trailing\n",
    ] {
        let out = flatten_lines(raw);
        assert!(
            !out.contains('\n') && !out.contains('\r'),
            "raw break survived in {out:?}"
        );
    }
}

/// Flattening an already flattened value is a no-op.
#[test]
fn flattening_is_idempotent() {
    let once = flatten_lines("a\r\nb\r\nc");
    assert_eq!(flatten_lines(&once), once);
}

/// A caller has one line for a duration, so the formatter must fall back to coarser units instead
/// of growing. A third unit, or a zero second unit, overflows the cell it renders into.
///
/// Built from the same keys the formatter reads rather than from English literals: the locale is a
/// GLOBAL in `rust_i18n`, other tests in this binary switch it while these run, and a test that
/// hard-coded "45s" would pass or fail depending on which one got there first.
#[test]
fn duration_shows_at_most_two_units() {
    let (s, m, h, d) = (
        rust_i18n::t!("analytics.cal.dur_s"),
        rust_i18n::t!("analytics.cal.dur_m"),
        rust_i18n::t!("analytics.cal.dur_h"),
        rust_i18n::t!("analytics.cal.dur_d"),
    );
    assert_eq!(fmt_duration_short(45.0), format!("45{s}"));
    assert_eq!(fmt_duration_short(89.0), format!("1{m} 29{s}"));
    assert_eq!(fmt_duration_short(120.0), format!("2{m}"));
    assert_eq!(fmt_duration_short(8_100.0), format!("2{h} 15{m}"));
    assert_eq!(fmt_duration_short(7_200.0), format!("2{h}"));
    assert_eq!(fmt_duration_short(273_600.0), format!("3{d} 4{h}"));
    // Rounding happens before the split, so 59.6 s is a minute rather than "59s".
    assert_eq!(fmt_duration_short(59.6), format!("1{m}"));
    assert_eq!(fmt_duration_short(0.0), format!("0{s}"));
}

/// A duration that cannot exist must not render as a number.
#[test]
fn duration_rejects_impossible_input() {
    assert_eq!(fmt_duration_short(-1.0), "—");
    assert_eq!(fmt_duration_short(f64::NAN), "—");
    assert_eq!(fmt_duration_short(f64::INFINITY), "—");
}

/// The ban countdown is one rule for three surfaces — the chart's caption, the coin menu's row and
/// the coin dropdown's ban tab — read to the MINUTE, rounded up, and never below one.
///
/// Breakage this pins: printing the raw remainder. The core lets its own figure drift for up to a
/// minute before republishing, so seconds would be precision the value does not carry — and a ban
/// whose local countdown has run out would read "0s" beside a lift button that still works.
#[test]
fn the_ban_countdown_rounds_up_to_the_minute() {
    let _locale = crate::test_locale::force("en");
    assert_eq!(fmt_ban_left(1), "1m");
    assert_eq!(fmt_ban_left(59_000), "1m");
    assert_eq!(fmt_ban_left(61_000), "2m");
    assert_eq!(fmt_ban_left(3_600_000), "1h");
    assert_eq!(fmt_ban_left(3_600_000 + 12 * 60_000), "1h 12m");
    assert_eq!(fmt_ban_left(3 * 24 * 3_600_000), "3d");
    assert_eq!(
        fmt_ban_left(-5_000),
        "1m",
        "a row the core still lists is still a ban"
    );
}
