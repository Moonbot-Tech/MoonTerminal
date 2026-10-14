//! Filter-line parsing: complete strategy prefixes navigate, while malformed text stays inert.

use super::{FilterLineTarget, parse_filter_line};

fn global(coin: &str) -> FilterLineTarget {
    FilterLineTarget::GlobalBlacklist {
        coin: coin.to_string(),
    }
}

fn strategy(name: &str, coin: &str) -> FilterLineTarget {
    FilterLineTarget::StrategyBlacklist {
        strategy: name.to_string(),
        coin: coin.to_string(),
    }
}

/// Dropping the optional strategy-shaped prefix on a global line would leave the real caption inert.
#[test]
fn a_global_line_parses_with_or_without_the_strategy_prefix() {
    assert_eq!(
        parse_filter_line("UNIT_01 ( Kind ) : Market XALPHA match global black list"),
        Some(global("XALPHA"))
    );
    assert_eq!(
        parse_filter_line("Market XALPHA match global black list"),
        Some(global("XALPHA"))
    );
}

/// Taking the strategy name as the coin, or stripping a quote off the market token, would open
/// the wrong list. The coin is only the token after ` ) : `, in the core's own spelling.
#[test]
fn a_strategy_line_keeps_the_name_and_the_raw_coin() {
    assert_eq!(
        parse_filter_line("UNIT_01 ( Kind ) : Market XALPHAUSDT match strategy black list"),
        Some(strategy("UNIT_01", "XALPHAUSDT"))
    );
    assert_eq!(
        parse_filter_line("XALPHA ( Kind ) : Market 2Z match strategy black list"),
        Some(strategy("XALPHA", "2Z"))
    );
}

/// Collapsing the parser onto a single-space scan would miss a caption the core padded.
/// Uppercasing the coin would miss a list the core stored in the spelling it printed.
#[test]
fn odd_spacing_around_the_tokens_still_parses() {
    assert_eq!(
        parse_filter_line(
            "  UNIT_01   ( Kind ) :   Market    xalphausdt   match strategy black list  "
        ),
        Some(strategy("UNIT_01", "xalphausdt"))
    );
}

/// Matching the suffix in one case only would ignore the same caption after a core rebuilds it
/// in another case. The coin stays as written; the strategy name does too.
#[test]
fn the_suffix_match_is_ascii_case_insensitive() {
    assert_eq!(
        parse_filter_line("UNIT_01 ( Kind ) : Market xalpha MATCH Global Black List"),
        Some(global("xalpha"))
    );
    assert_eq!(
        parse_filter_line("Unit_01 ( Kind ) : Market xalpha MATCH Strategy Black List"),
        Some(strategy("Unit_01", "xalpha"))
    );
}

/// Removing the complete strategy prefix requirement would navigate from unrelated captions.
#[test]
fn a_line_without_a_strategy_prefix_or_blacklist_suffix_stays_inert() {
    assert_eq!(parse_filter_line("Market XALPHA is quiet"), None);
    assert_eq!(parse_filter_line("match global black"), None);
    assert_eq!(parse_filter_line("   "), None);
}

/// Leaving non-blacklist refusals inert or mapping them to the wrong parameter loses navigation.
#[test]
fn strategy_refusals_reveal_only_the_fields_the_reason_identifies() {
    for (reason, field) in [
        (
            "3h delta (27.3%) doesnt match strategy range (6.9% .. 25.8%)",
            Some("Delta_3h_Max"),
        ),
        (
            "3h delta (1.2%) doesnt match strategy range (6.9% .. 25.8%)",
            Some("Delta_3h_Min"),
        ),
        (
            "Vol Minute delta (1.12) doesnt match strategy range (3.00 .. 0.00)",
            Some("MinuteVolDeltaMin"),
        ),
        (
            "EMA filter not passed. Open full screen chart to see more info",
            Some("CustomEMA"),
        ),
        (
            "Market WAL must have \"TradFi\" tag",
            Some("BinanceTokenTags"),
        ),
        (
            "Market WAL doesnt match strategy white list",
            Some("CoinsWhiteList"),
        ),
        ("Market XALPHA is quiet", None),
        (
            "Other delta (27.3%) doesnt match strategy range (6.9% .. 25.8%)",
            None,
        ),
        (
            "3h delta (NaN) doesnt match strategy range (6.9% .. 25.8%)",
            None,
        ),
        (
            "3h delta (27.3%) doesnt match strategy range (30% .. 25.8%)",
            None,
        ),
        (
            "3h delta (27.3%) doesnt match strategy range (6.9% .. 0%)",
            None,
        ),
    ] {
        assert_eq!(
            parse_filter_line(&format!("UNIT_01 ( Kind ) : {reason}")),
            Some(FilterLineTarget::Strategy {
                strategy: "UNIT_01".into(),
                field
            }),
            "{reason}"
        );
    }
}

/// Dropping a display code at parsing time loses exact-name matches in the strategy store.
#[test]
fn display_codes_are_preserved_and_summary_lines_stay_inert() {
    assert_eq!(
        parse_filter_line("UNIT_01 - SAMPLE [7QK2M4XZ] ( Kind ) : unknown reason"),
        Some(FilterLineTarget::Strategy {
            strategy: "UNIT_01 - SAMPLE [7QK2M4XZ]".into(),
            field: None
        })
    );
    assert_eq!(parse_filter_line("... ещё 6"), None);
    assert_eq!(parse_filter_line("... 6 more"), None);
    assert_eq!(parse_filter_line("UNIT_01 ( ) : unknown reason"), None);
    assert_eq!(parse_filter_line("UNIT_01 ( Kind : unknown reason"), None);
}

/// Guessing a strategy name from the last word, or from a half-written prefix, would open a
/// strategy the line never named. The strategy kind needs the whole `<name> ( <kind> ) : ` shape.
#[test]
fn a_strategy_suffix_with_a_malformed_prefix_stays_inert() {
    assert_eq!(
        parse_filter_line("Market XALPHAUSDT match strategy black list"),
        None
    );
    assert_eq!(
        parse_filter_line("UNIT_01 ( Kind ) Market XALPHA match strategy black list"),
        None
    );
    assert_eq!(
        parse_filter_line(" ( Kind ) : Market XALPHA match strategy black list"),
        None
    );
}

/// Reading `Market` itself as the coin would navigate from a caption that names no coin.
/// A pair token stays whole: the core's blacklist identity is the token it printed.
#[test]
fn the_coin_is_the_raw_token_after_one_leading_market() {
    assert_eq!(
        parse_filter_line("Market 2Z match global black list"),
        Some(global("2Z"))
    );
    assert_eq!(
        parse_filter_line("Market WBTC match global black list"),
        Some(global("WBTC"))
    );
    assert_eq!(
        parse_filter_line("Market XALPHAUSDT match global black list"),
        Some(global("XALPHAUSDT"))
    );
    assert_eq!(parse_filter_line("Market match global black list"), None);
    assert_eq!(
        parse_filter_line("UNIT_01 ( Kind ) : Market match strategy black list"),
        None
    );
    assert_eq!(
        parse_filter_line("Market --- match global black list"),
        None
    );
}
