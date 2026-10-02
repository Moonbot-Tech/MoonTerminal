use super::*;

fn keys(list: &[&str]) -> Vec<String> {
    list.iter().map(|c| c.to_string()).collect()
}

fn row(white: &[&str], black: &[&str]) -> RowLists {
    RowLists {
        white: keys(white),
        black: keys(black),
    }
}

/// A copy keeps the coin where it is and appends it to the other list; a removal drops it.
#[test]
fn chip_actions_copy_or_remove_one_coin() {
    let lists = row(&["AAA", "BBB"], &["ZZZ"]);
    assert_eq!(
        apply_action(&lists, "BBB", ChipAction::RemoveWhite),
        row(&["AAA"], &["ZZZ"])
    );
    assert_eq!(
        apply_action(&lists, "BBB", ChipAction::AddBlack),
        row(&["AAA", "BBB"], &["ZZZ", "BBB"])
    );
    assert_eq!(
        apply_action(&lists, "ZZZ", ChipAction::AddWhite),
        row(&["AAA", "BBB", "ZZZ"], &["ZZZ"])
    );
    // Adding what is already there changes nothing.
    assert_eq!(apply_action(&lists, "ZZZ", ChipAction::AddBlack), lists);
}

/// Six rows, thirteen coins: the remainder (1) goes to the top part (3), the rest take 2 each. With
/// the first row blacklisting, it keeps no whitelist and its blacklist is the common one followed
/// by the other parts; every other row gets its part and the common blacklist.
#[test]
fn distribute_with_the_first_row_blacklisting() {
    let universe = keys(&[
        "A", "B", "C", "D", "E", "F", "G", "H", "I", "J", "K", "L", "M",
    ]);
    let rows = vec![
        row(&[], &["X", "D"]),
        row(&["D"], &["Y"]),
        row(&[], &[]),
        row(&[], &[]),
        row(&[], &[]),
        row(&[], &[]),
    ];
    let out = distribute(
        &rows,
        &universe,
        DistributeOptions {
            first_blacklists: true,
            skip_common_black: false,
        },
    );
    // "D" sat in row 1's blacklist only because row 2 whitelisted it — the previous distribution,
    // not a choice — so it is not pooled into the common blacklist.
    let common = keys(&["X", "Y"]);
    assert_eq!(out[0].white, Vec::<String>::new());
    assert_eq!(
        out[0].black,
        keys(&["X", "Y", "D", "E", "F", "G", "H", "I", "J", "K", "L", "M"])
    );
    assert_eq!(
        out[1],
        RowLists {
            white: keys(&["D", "E"]),
            black: common.clone()
        }
    );
    assert_eq!(
        out[5],
        RowLists {
            white: keys(&["L", "M"]),
            black: common
        }
    );
}

#[test]
fn distribute_to_every_row_and_skip_the_common_blacklist() {
    let universe = keys(&["A", "B", "C", "D", "E"]);
    let rows = vec![row(&[], &["B"]), row(&[], &[])];
    let out = distribute(
        &rows,
        &universe,
        DistributeOptions {
            first_blacklists: false,
            skip_common_black: true,
        },
    );
    // "B" stays out of the parts: A C D E split 2 / 2.
    assert_eq!(
        out[0],
        RowLists {
            white: keys(&["A", "C"]),
            black: keys(&["B"])
        }
    );
    assert_eq!(
        out[1],
        RowLists {
            white: keys(&["D", "E"]),
            black: keys(&["B"])
        }
    );

    // Without the skip the blacklisted coin is dealt like any other.
    let dealt = distribute(
        &rows,
        &universe,
        DistributeOptions {
            first_blacklists: false,
            skip_common_black: false,
        },
    );
    assert_eq!(dealt[0].white, keys(&["A", "B", "C"]));
}

#[test]
fn nothing_to_distribute_between_no_rows() {
    let opts = DistributeOptions {
        first_blacklists: true,
        skip_common_black: false,
    };
    assert!(distribute(&[], &keys(&["A"]), opts).is_empty());
}

/// Entries the target keeps stay as written (both spellings of BTC, in their case), new ones are
/// added as the key, the field's own separator is kept, and the order follows the target.
#[test]
fn a_list_is_rewritten_without_disturbing_what_it_keeps() {
    assert_eq!(
        rewrite_list("btc, BTC_0626, eth", &keys(&["ETH", "BTC", "SOL"]), &[]),
        "eth, btc, BTC_0626, SOL"
    );
    assert_eq!(rewrite_list("AAA BBB", &keys(&["BBB"]), &[]), "BBB");
    assert_eq!(rewrite_list("", &keys(&["AAA", "BBB"]), &[]), "AAA, BBB");
    // A coin copied in from the other list keeps that list's spelling: a contract stays one.
    assert_eq!(
        rewrite_list("ETH", &keys(&["ETH", "BTC"]), &["SOL, BTC_0626"]),
        "ETH, BTC_0626"
    );
    assert_eq!(list_keys("sol, SOL_RP, eth"), keys(&["SOL", "ETH"]));
}

/// Switching "BL on the first" off after a distribution made with it on must not blacklist every
/// row's own part: the first row's blacklist WAS the other parts.
#[test]
fn a_previous_split_is_not_pooled_into_the_common_blacklist() {
    let universe = keys(&["A", "B", "C", "D"]);
    let rows = vec![
        row(&[], &["C", "D", "X"]),
        row(&["C"], &[]),
        row(&["D"], &[]),
    ];
    let out = distribute(
        &rows,
        &universe,
        DistributeOptions {
            first_blacklists: false,
            skip_common_black: false,
        },
    );
    assert!(out.iter().all(|r| r.black == keys(&["X"])));
    // A coin a row blacklists while whitelisting it itself stays blacklisted.
    let own = distribute(
        &[row(&["S"], &["S"]), row(&[], &[])],
        &universe,
        DistributeOptions {
            first_blacklists: false,
            skip_common_black: false,
        },
    );
    assert_eq!(own[1].black, keys(&["S"]));
}

/// Putting a coin back restores ONE list as the core has it; a copy in the other list stays.
#[test]
fn restore_puts_the_coin_back_on_its_own_list_only() {
    let live = row(&["AAA", "BBB"], &[]);
    let copied_then_removed = apply_action(
        &apply_action(&live, "BBB", ChipAction::AddBlack),
        "BBB",
        ChipAction::RemoveWhite,
    );
    assert_eq!(
        restore(&copied_then_removed, &live, "BBB", true),
        row(&["AAA", "BBB"], &["BBB"])
    );
    let added = apply_action(&live, "NEW", ChipAction::AddBlack);
    assert_eq!(restore(&added, &live, "NEW", false), live);
}

/// Removing what a copy added takes the lists back to where they were — so the draft drops.
#[test]
fn removing_a_copy_undoes_it() {
    let live = row(&["AAA"], &["ZZZ"]);
    let copied = apply_action(&live, "AAA", ChipAction::AddBlack);
    assert_eq!(apply_action(&copied, "AAA", ChipAction::RemoveBlack), live);
}

/// The free coins' remainder is spread one coin each over the top rows, not heaped on the first.
#[test]
fn the_remainder_goes_one_each_to_the_top_rows() {
    let universe: Vec<String> = (0..740).map(|i| format!("C{i:03}")).collect();
    let rows = vec![RowLists::default(); 6];
    let out = distribute(
        &rows,
        &universe,
        DistributeOptions {
            first_blacklists: false,
            skip_common_black: false,
        },
    );
    let sizes: Vec<usize> = out.iter().map(|r| r.white.len()).collect();
    assert_eq!(sizes, vec![124, 124, 123, 123, 123, 123]);
    // Consecutive runs that cover the market exactly once.
    let dealt: Vec<&String> = out.iter().flat_map(|r| &r.white).collect();
    assert_eq!(dealt, universe.iter().collect::<Vec<_>>());
}

/// With the common blacklist dealt too, every row still TRADES the same number of coins (±1):
/// the blacklisted coins are spread apart from the rest instead of landing where the alphabet
/// puts them.
#[test]
fn every_row_trades_the_same_number_when_the_blacklist_is_dealt() {
    let mut universe: Vec<String> = (0..718).map(|i| format!("C{i:03}")).collect();
    // 22 blacklisted coins, all at the start of the alphabet.
    let black: Vec<String> = (0..22).map(|i| format!("A{i:02}")).collect();
    universe.extend(black.iter().cloned());
    universe.sort();
    let mut rows = vec![RowLists::default(); 6];
    rows[1].black = black.clone();
    let out = distribute(
        &rows,
        &universe,
        DistributeOptions {
            first_blacklists: false,
            skip_common_black: false,
        },
    );
    let trading: Vec<usize> = out
        .iter()
        .map(|r| r.white.iter().filter(|c| !black.contains(c)).count())
        .collect();
    assert_eq!(trading, vec![120, 120, 120, 120, 119, 119]);
    let sizes: Vec<usize> = out.iter().map(|r| r.white.len()).collect();
    assert!(sizes.iter().all(|s| (123..=124).contains(s)), "{sizes:?}");
    assert_eq!(sizes.iter().sum::<usize>(), 740);
}
