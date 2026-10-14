//! Filter-line parsing. A miss must stay `None`: the chart would otherwise open the wrong list.

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

/// Treating any line that mentions a blacklist as clickable would navigate from a caption that
/// does not name one of the two lists.
#[test]
fn a_line_without_the_suffix_stays_inert() {
    assert_eq!(
        parse_filter_line("UNIT_01 ( Kind ) : Market XALPHA is quiet"),
        None
    );
    assert_eq!(parse_filter_line("match global black"), None);
    assert_eq!(parse_filter_line("   "), None);
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
