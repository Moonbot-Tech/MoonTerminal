use super::*;

/// Splitting on punctuation or dropping digits would give cards and outages different searches.
/// Names without a letter must remain escaped text rather than an unclickable numeric tag.
#[test]
fn coin_tags_keep_one_searchable_token_or_the_escaped_fallback() {
    for (raw, expected) in [
        ("SAMPLE", "#SAMPLE"),
        ("SAMPLE714", "#SAMPLE714"),
        ("1000SAMPLE", "#1000SAMPLE"),
        ("Desk F-2.v1/test", "#Desk_F_2_v1_test"),
        ("Desk_F2", "#Desk_F2"),
        (
            "\u{042f}\u{0434}\u{0440}\u{043e} 2",
            "#\u{042f}\u{0434}\u{0440}\u{043e}_2",
        ),
        ("A<&>\"B", "#A____B"),
        ("A\u{00b2}B", "#A_B"),
        ("A\u{0345}B", "#A_B"),
        ("\u{00b2}\u{0345}", "\u{00b2}\u{0345}"),
        ("A\u{0662}", "#A\u{0662}"),
        ("#123", "#123"),
        ("12345", "12345"),
        ("___", "___"),
        ("<&>\"", "&lt;&amp;&gt;&quot;"),
        ("", ""),
    ] {
        assert_eq!(
            name_tag(raw, TAG_NAME_CHARS, false),
            expected,
            "input: {raw:?}"
        );
    }
}

/// Cutting after escaping or after mapping would change the tag between push surfaces.
#[test]
fn coin_tags_cap_unicode_scalars_before_mapping() {
    let prefix = "A".repeat(TAG_NAME_CHARS - 1);
    assert_eq!(
        name_tag(&format!("{prefix}\u{044f}-tail"), TAG_NAME_CHARS, false),
        format!("#{prefix}\u{044f}")
    );
    assert_eq!(name_tag("&&A", 2, false), "&amp;&amp;");
}

/// A coin is a tag; a character a tag cannot hold becomes `_`; a coin without a letter is bold.
#[test]
fn coin_tags() {
    assert_eq!(name_tag("MARSCOIN", 64, true), "#MARSCOIN");
    assert_eq!(name_tag("1000PEPE", 64, true), "#1000PEPE");
    assert_eq!(name_tag("BTC-PERP", 64, true), "#BTC_PERP");
    assert_eq!(name_tag("<b>&", 64, true), "#_b__");
    assert_eq!(name_tag("1000", 64, true), "<b>1000</b>");
    assert_eq!(name_tag("#123", 64, true), "<b>#123</b>");
    assert_eq!(name_tag("<>", 64, true), "<b>&lt;&gt;</b>");
    assert_eq!(name_tag("ABCDEF", 3, true), "#ABC");
    assert_eq!(name_tag("", 64, true), "");
}
