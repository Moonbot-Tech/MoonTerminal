use super::*;
use moon_core::feed::types::ExchangeId;

fn venue(code: u8) -> ExchangeSection {
    ExchangeSection::Venue(ExchangeId { code, dex: 0 })
}

fn strat(name: &str, white: &str, black: &str) -> StrategyInput {
    StrategyInput {
        id: 1,
        name: name.to_string(),
        white: white.to_string(),
        black: black.to_string(),
        live_white: white.to_string(),
        live_black: black.to_string(),
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

/// A coin the exchange dropped stays grey and a duplicate stays yellow whatever it earned; only a
/// normal chip takes its colour from the result.
#[test]
fn a_chip_reads_its_state_before_its_result() {
    assert_eq!(look(ChipState::Gone, Some(5.0)), Look::Gone);
    assert_eq!(look(ChipState::Duplicate, Some(-5.0)), Look::Duplicate);
    assert_eq!(look(ChipState::Normal, Some(5.0)), Look::Profit);
    assert_eq!(look(ChipState::Normal, Some(-5.0)), Look::Loss);
    assert_eq!(look(ChipState::Normal, Some(0.0)), Look::Flat);
    assert_eq!(look(ChipState::Normal, None), Look::Flat);
}

#[test]
fn profit_order_puts_untraded_coins_between_winners_and_losers() {
    let chips: Vec<Chip> = ["AAA", "BBB", "CCC", "DDD"]
        .iter()
        .map(|c| Chip {
            coin: c.to_string(),
            state: ChipState::Normal,
            edit: Edit::Same,
        })
        .collect();
    let profit = |coin: &str| match coin {
        "AAA" => Some(-3.0),
        "CCC" => Some(7.0),
        "DDD" => Some(1.0),
        _ => None,
    };
    let order: Vec<&str> = by_profit(&chips, profit)
        .into_iter()
        .map(|c| c.coin.as_str())
        .collect();
    assert_eq!(order, vec!["CCC", "DDD", "BBB", "AAA"]);
}

/// Report rows name a coin as the core spelled it; the chips name it by match key. Spellings of
/// one coin add up, and a unit they disagree on is dropped rather than mislabelled.
#[test]
fn report_groups_fold_onto_the_chip_key() {
    let mut units = moon_core::db::QuoteCurrency::all();
    let (a, b) = (units.next(), units.next());
    let folded = stats::fold_coins([
        ("BTC".to_string(), 3, 2, 10.0, a),
        ("btc_0626".to_string(), 1, 0, -4.0, a),
        ("ETH".to_string(), 2, 1, 1.0, a),
        ("eth".to_string(), 1, 1, 1.0, b),
    ]);
    let btc = folded.stats["BTC"];
    assert_eq!(
        (btc.trades, btc.wins, btc.profit, btc.currency),
        (4, 2, 6.0, a)
    );
    assert_eq!(folded.spellings["BTC"], vec!["BTC", "btc_0626"]);
    assert_eq!(folded.stats["ETH"].currency, None);
    // Sums in two units compare with nothing: no colour, no place in the profit order.
    assert_eq!(folded.stats["ETH"].comparable_profit(), None);
}

/// A loss too small for the unit's precision prints as an unsigned zero, not "-0.00".
#[test]
fn profit_text_signs_what_it_prints() {
    let usdt = moon_core::db::QuoteCurrency::usdt();
    let text = |profit| {
        trades::profit_text(&stats::CoinStat {
            trades: 1,
            wins: 0,
            profit,
            currency: Some(usdt),
        })
    };
    assert_eq!(text(12.345), "+12.35 USDT");
    assert_eq!(text(-3.0), "-3.00 USDT");
    assert_eq!(text(-0.004), "0.00 USDT");
    assert_eq!(text(-0.0), "0.00 USDT");
    assert_eq!(
        trades::profit_text(&stats::CoinStat {
            trades: 1,
            ..Default::default()
        }),
        "—"
    );
}

/// The trades table sorts numbers by value, not as text ("10" after "9"), text caselessly, and an
/// empty cell first.
#[test]
fn trade_cells_compare_by_kind() {
    use rusqlite::types::Value;
    use std::cmp::Ordering;
    let cmp = trades::cmp_values;
    assert_eq!(cmp(&Value::Integer(9), &Value::Integer(10)), Ordering::Less);
    assert_eq!(cmp(&Value::Real(-2.5), &Value::Integer(1)), Ordering::Less);
    assert_eq!(
        cmp(&Value::Text("abc".into()), &Value::Text("ABD".into())),
        Ordering::Less
    );
    assert_eq!(cmp(&Value::Null, &Value::Integer(0)), Ordering::Less);
    // Mixed types order by type first, so the comparison stays transitive.
    assert_eq!(
        cmp(&Value::Integer(5), &Value::Text("a".into())),
        Ordering::Less
    );
    assert_eq!(
        cmp(&Value::Text("a".into()), &Value::Integer(1)),
        Ordering::Greater
    );
}

/// A coin a row both whitelists and blacklists is not traded by that row: its chip says so, and
/// it neither covers the coin nor makes another row's chip a duplicate.
#[test]
fn a_whitelisted_coin_the_row_blacklists_is_blocked() {
    let inputs = vec![
        slot(1, vec![strat("A", "SYN, PROM", "SYN")]),
        slot(2, vec![strat("A", "SYN", "")]),
    ];
    let cat = catalog(&["SYN", "PROM"]);
    let catalogs = HashMap::from([(1, cat.clone()), (2, cat)]);
    let board = build(inputs, &[], &catalogs).unwrap();

    assert_eq!(state_of(&board.slots[0].white, "SYN"), ChipState::Blocked);
    assert_eq!(state_of(&board.slots[0].white, "PROM"), ChipState::Normal);
    assert_eq!(state_of(&board.slots[1].white, "SYN"), ChipState::Normal);
    assert_eq!(look(ChipState::Blocked, Some(5.0)), Look::Blocked);
}

/// An empty whitelist still has a share: the coins its core's catalog lists less its blacklist.
/// A coin another row whitelists is a duplicate there too; a row that names its coins, or whose
/// catalog has not arrived, draws no such line.
#[test]
fn an_empty_whitelist_shows_what_it_trades() {
    let inputs = vec![
        slot(1, vec![strat("A", "", "CCC")]),
        slot(2, vec![strat("A", "BBB", "")]),
    ];
    let cat = catalog(&["AAA", "BBB", "CCC"]);
    let catalogs = HashMap::from([(1, cat.clone()), (2, cat)]);
    let board = build(inputs, &[], &catalogs).unwrap();

    let traded = board.slots[0].traded.as_ref().unwrap();
    assert_eq!(coins(traded), vec!["AAA", "BBB"]);
    assert_eq!(state_of(traded, "AAA"), ChipState::Normal);
    assert_eq!(state_of(traded, "BBB"), ChipState::Duplicate);
    assert!(board.slots[1].traded.is_none());

    let unknown = build(
        vec![slot(3, vec![strat("A", "", "")])],
        &[],
        &HashMap::new(),
    )
    .unwrap();
    assert!(unknown.slots[0].traded.is_none());
}

/// A draft is drawn against what the core stores: an entry it adds is marked, one it drops stays
/// in place as removed — and only the draft decides what the row trades.
#[test]
fn drafts_mark_added_and_removed_chips() {
    let mut s = strat("A", "AAA, CCC", "");
    s.live_white = "AAA, BBB".to_string();
    let board = build(vec![slot(1, vec![s])], &[], &HashMap::new()).unwrap();
    let white = &board.slots[0].white;
    let edit_of = |coin: &str| white.iter().find(|c| c.coin == coin).unwrap().edit;
    assert_eq!(edit_of("AAA"), Edit::Same);
    assert_eq!(edit_of("BBB"), Edit::Removed);
    assert_eq!(edit_of("CCC"), Edit::Added);
    assert_eq!(board.slots[0].lists.white, vec!["AAA", "CCC"]);
}

/// The "Trades" line is marked against what the core trades now: emptying a whitelist in the
/// draft shows the coins it starts trading as added, the one it kept as unchanged.
#[test]
fn the_trades_line_marks_its_change_against_the_core() {
    let mut s = strat("A", "", "");
    s.live_white = "AAA".to_string();
    let catalogs = HashMap::from([(1, catalog(&["AAA", "BBB"]))]);
    let board = build(vec![slot(1, vec![s])], &[], &catalogs).unwrap();
    let traded = board.slots[0].traded.as_ref().unwrap();
    let edit_of = |coin: &str| traded.iter().find(|c| c.coin == coin).unwrap().edit;
    assert_eq!(edit_of("AAA"), Edit::Same);
    assert_eq!(edit_of("BBB"), Edit::Added);
}
