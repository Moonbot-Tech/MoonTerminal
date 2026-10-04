use super::*;

/// A coin is a tag; a character a tag cannot hold becomes `_`; a coin without a letter is bold.
#[test]
fn coin_tags() {
    assert_eq!(coin_tag("MARSCOIN", 64), "#MARSCOIN");
    assert_eq!(coin_tag("1000PEPE", 64), "#1000PEPE");
    assert_eq!(coin_tag("BTC-PERP", 64), "#BTC_PERP");
    assert_eq!(coin_tag("<b>&", 64), "#_b__");
    assert_eq!(coin_tag("1000", 64), "<b>1000</b>");
    assert_eq!(coin_tag("<>", 64), "<b>&lt;&gt;</b>");
    assert_eq!(coin_tag("ABCDEF", 3), "#ABC");
    assert_eq!(coin_tag("", 64), "");
}
