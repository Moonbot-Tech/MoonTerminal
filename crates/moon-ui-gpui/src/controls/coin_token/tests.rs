//! Token-split pins for the strategy blacklist field.

use super::{CoinTokenPiece, split_coin_token, window_coin_tokens};

/// `split_coin_token` marks a case-insensitive token and leaves a pair that merely starts with it.
#[test]
fn case_spaces_and_a_pair_substring_stay_distinct() {
    let marked = split_coin_token("  xalpha , XALPHAUSDT", "XALPHA");
    assert_eq!(marked.len(), 2);
    assert_eq!(&"  xalpha , XALPHAUSDT"[marked[0].0.clone()], "xalpha");
    assert!(marked[0].1);
    assert_eq!(&"  xalpha , XALPHAUSDT"[marked[1].0.clone()], "XALPHAUSDT");
    assert!(!marked[1].1);

    let pair = split_coin_token("XALPHAUSDT", "XALPHA");
    assert_eq!(pair.len(), 1);
    assert!(!pair[0].1);

    assert!(split_coin_token("", "XALPHA").is_empty());
    assert!(split_coin_token(" , , ", "XALPHA").is_empty());
}

/// A hit at the end of a list that does not fit keeps a leading ellipsis and the hit itself.
///
/// Dropping the window and painting from the first token would clip `WBTC` off the row.
#[test]
fn a_hit_at_the_end_of_a_long_list_keeps_a_leading_ellipsis() {
    let names = [
        "AAA", "BBB", "CCC", "DDD", "EEE", "FFF", "GGG", "HHH", "III", "JJJ", "KKK", "WBTC",
    ];
    let tokens: Vec<(&str, bool)> = names
        .iter()
        .copied()
        .map(|name| (name, name == "WBTC"))
        .collect();
    let full = names.join(",");
    assert!(
        full.chars().count() > 12,
        "the fixture has to be longer than the budget or the ellipsis is optional"
    );
    let pieces = window_coin_tokens(&tokens, Some(12));
    assert_eq!(
        pieces,
        vec![
            CoinTokenPiece::Ellipsis,
            CoinTokenPiece::Token {
                text: "KKK",
                hit: false,
            },
            CoinTokenPiece::Token {
                text: "WBTC",
                hit: true,
            },
        ]
    );
}

/// A list that already fits is drawn whole, with no ellipsis.
#[test]
fn a_short_list_keeps_every_token() {
    let tokens = [("XALPHA", true), ("WBTC", false)];
    let pieces = window_coin_tokens(&tokens, Some(40));
    assert!(
        pieces
            .iter()
            .all(|piece| !matches!(piece, CoinTokenPiece::Ellipsis))
    );
    assert_eq!(pieces.len(), 2);
}
