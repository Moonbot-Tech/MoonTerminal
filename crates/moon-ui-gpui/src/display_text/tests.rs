use super::flatten_lines;

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
