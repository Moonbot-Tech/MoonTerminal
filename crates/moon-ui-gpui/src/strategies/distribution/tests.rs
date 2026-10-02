use super::*;
use moon_core::feed::types::ExchangeId;

fn venue(code: u8) -> ExchangeSection {
    ExchangeSection::Venue(ExchangeId { code, dex: 0 })
}

fn strat(name: &str, white: &str, black: &str) -> StrategyInput {
    StrategyInput {
        name: name.to_string(),
        white: white.to_string(),
        black: black.to_string(),
    }
}

fn slot(core: CoreId, strategies: Vec<StrategyInput>) -> SlotInput {
    SlotInput {
        core,
        core_name: format!("F{core}"),
        section: venue(1),
        strategies,
    }
}

fn catalog(coins: &[&str]) -> Option<HashSet<String>> {
    Some(coins.iter().map(|c| c.to_string()).collect())
}

fn coins(chips: &[Chip]) -> Vec<&str> {
    chips.iter().map(|c| c.coin.as_str()).collect()
}

fn state_of(chips: &[Chip], coin: &str) -> ChipState {
    chips.iter().find(|c| c.coin == coin).unwrap().state
}

#[test]
fn nothing_selected_draws_nothing() {
    let got = build(Vec::new(), &[], &HashMap::new());
    assert_eq!(got.unwrap_err(), Unavailable::NoSelection);
}

/// One market cannot be split between two exchanges, and a core that has not said which exchange
/// it is on cannot be checked against the others.
#[test]
fn rows_must_share_one_known_exchange() {
    let mut other = slot(2, vec![strat("B", "", "")]);
    other.section = venue(7);
    let got = build(
        vec![slot(1, vec![strat("A", "", "")]), other],
        &[],
        &HashMap::new(),
    );
    assert_eq!(got.unwrap_err(), Unavailable::MixedVenues);

    let mut unknown = slot(2, vec![strat("B", "", "")]);
    unknown.section = ExchangeSection::Unidentified;
    let got = build(
        vec![slot(1, vec![strat("A", "", "")]), unknown],
        &[],
        &HashMap::new(),
    );
    assert_eq!(got.unwrap_err(), Unavailable::UnknownVenue);
}

#[test]
fn saved_order_leads_and_the_rest_follow_in_tree_order() {
    assert_eq!(ordered(&[1, 2, 3, 4], &[3, 9, 1]), vec![3, 1, 2, 4]);
    assert_eq!(ordered(&[1, 2], &[]), vec![1, 2]);
}

/// Arranging one selection keeps what was saved for cores it does not show.
#[test]
fn moving_a_row_keeps_cores_it_does_not_show() {
    assert_eq!(moved(&[1, 2, 3], &[9, 1], 3, true), Some(vec![1, 3, 2, 9]));
    assert_eq!(moved(&[1, 2, 3], &[], 1, true), None);
    assert_eq!(moved(&[1, 2, 3], &[], 3, false), None);
    assert_eq!(moved(&[1, 2, 3], &[], 7, true), None);
}

/// A first core with no whitelist and everybody else's coins as its blacklist trades "the rest
/// of the market": every coin is traded once, none twice.
#[test]
fn blacklist_on_the_first_core_covers_the_rest_without_duplicates() {
    let inputs = vec![
        slot(1, vec![strat("A", "", "CCC, DDD")]),
        slot(2, vec![strat("A", "CCC", "")]),
        slot(3, vec![strat("A", "DDD", "")]),
    ];
    let cat = catalog(&["AAA", "BBB", "CCC", "DDD"]);
    let catalogs = HashMap::from([(1, cat.clone()), (2, cat.clone()), (3, cat)]);
    let board = build(inputs, &[], &catalogs).unwrap();

    assert_eq!(
        board.coverage,
        Some(Coverage {
            total: 4,
            traded: 4
        })
    );
    assert!(board.slots[0].white.is_empty());
    assert_eq!(coins(&board.slots[0].black), vec!["CCC", "DDD"]);
    assert!(
        board
            .slots
            .iter()
            .flat_map(|s| &s.white)
            .all(|c| c.state == ChipState::Normal)
    );
}

#[test]
fn a_coin_two_cores_trade_is_a_duplicate_and_an_untraded_one_is_counted() {
    let inputs = vec![
        slot(1, vec![strat("A", "AAA BBB", "")]),
        slot(2, vec![strat("A", "BBB", "")]),
    ];
    let cat = catalog(&["AAA", "BBB", "CCC"]);
    let catalogs = HashMap::from([(1, cat.clone()), (2, cat)]);
    let board = build(inputs, &[], &catalogs).unwrap();

    assert_eq!(state_of(&board.slots[0].white, "AAA"), ChipState::Normal);
    assert_eq!(state_of(&board.slots[0].white, "BBB"), ChipState::Duplicate);
    assert_eq!(state_of(&board.slots[1].white, "BBB"), ChipState::Duplicate);
    assert_eq!(
        board.coverage,
        Some(Coverage {
            total: 3,
            traded: 2
        })
    );
}

/// A listed coin its core's catalog no longer trades is grey, ahead of being a duplicate — and a
/// core whose catalog has not arrived marks nothing at all.
#[test]
fn a_coin_missing_from_the_catalog_is_gone_unless_the_catalog_is_unknown() {
    let inputs = vec![
        slot(1, vec![strat("A", "OLD, AAA", "OLD")]),
        slot(2, vec![strat("A", "OLD", "")]),
    ];
    let catalogs = HashMap::from([(1, catalog(&["AAA"])), (2, None)]);
    let board = build(inputs, &[], &catalogs).unwrap();

    assert_eq!(state_of(&board.slots[0].white, "OLD"), ChipState::Gone);
    assert_eq!(state_of(&board.slots[0].black, "OLD"), ChipState::Gone);
    assert_eq!(state_of(&board.slots[1].white, "OLD"), ChipState::Normal);
}

#[test]
fn no_catalog_means_unknown_coverage_not_zero() {
    let board = build(
        vec![slot(1, vec![strat("A", "AAA", "")])],
        &[],
        &HashMap::new(),
    )
    .unwrap();
    assert_eq!(board.coverage, None);
}

/// A long/short pair on one core is one row. When the pair disagrees, the row says so and draws
/// the union rather than one side's list.
#[test]
fn a_pair_that_disagrees_is_flagged_and_drawn_as_the_union() {
    let agree = slot(
        1,
        vec![strat("L", "AAA, BBB", "X"), strat("S", "bbb aaa", "x")],
    );
    let differ = slot(2, vec![strat("L", "CCC", ""), strat("S", "DDD", "")]);
    let board = build(vec![agree, differ], &[], &HashMap::new()).unwrap();

    assert!(!board.slots[0].lists_differ);
    assert_eq!(board.slots[0].strategies, vec!["L", "S"]);
    assert!(board.slots[1].lists_differ);
    assert_eq!(coins(&board.slots[1].white), vec!["CCC", "DDD"]);
}

/// Entries that fold to one coin — a contract suffix, another case — are one chip.
#[test]
fn spellings_of_one_coin_are_one_chip() {
    let board = build(
        vec![slot(1, vec![strat("A", "BTC_0626, btc, BTC", "")])],
        &[],
        &HashMap::new(),
    )
    .unwrap();
    assert_eq!(coins(&board.slots[0].white), vec!["BTC"]);
}

/// Two rows whitelisting one coin are a duplicate before any catalog arrives: the lists alone say
/// so.
#[test]
fn a_duplicate_shows_without_a_catalog() {
    let board = build(
        vec![
            slot(1, vec![strat("A", "AAA", "")]),
            slot(2, vec![strat("A", "AAA", "")]),
        ],
        &[],
        &HashMap::new(),
    )
    .unwrap();
    assert_eq!(state_of(&board.slots[0].white, "AAA"), ChipState::Duplicate);
}

/// A row cannot trade a coin its own core's exchange no longer lists, even when another core's
/// catalog still has it: it neither covers the coin nor duplicates the row that does.
#[test]
fn a_coin_missing_from_its_own_catalog_is_not_traded_by_that_row() {
    let inputs = vec![
        slot(1, vec![strat("A", "NEW", "")]),
        slot(2, vec![strat("A", "AAA, NEW", "")]),
    ];
    let catalogs = HashMap::from([(1, catalog(&["AAA"])), (2, catalog(&["AAA", "NEW"]))]);
    let board = build(inputs, &[], &catalogs).unwrap();

    assert_eq!(state_of(&board.slots[0].white, "NEW"), ChipState::Gone);
    assert_eq!(state_of(&board.slots[1].white, "NEW"), ChipState::Normal);
    assert_eq!(
        board.coverage,
        Some(Coverage {
            total: 2,
            traded: 2
        })
    );
}
