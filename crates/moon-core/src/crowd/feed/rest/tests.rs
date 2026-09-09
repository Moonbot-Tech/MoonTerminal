use super::*;

/// A real answer, trimmed to five rows. Kept verbatim rather than tidied: the point of a fixture
/// is that it is what the wire actually said.
const COINS: &str = r#"[{"i":1,"c":"SOPH","p":10116.56},{"i":2,"c":"PUMP","p":6165},
{"i":6,"c":"CP","p":-8887.88},{"i":7,"c":"XAN","p":-5234.14},{"i":8,"c":"BNC","p":-2069.74}]"#;

/// Likewise, with the hidden trader the service really sends.
const TRADERS: &str = r#"[
{"r":1,"i":404,"t":"@moon_bot_kurilka","p":12343.03,"o":369.04,"c":54052,"pb":45550.51,"f":"pro"},
{"r":4,"i":40729,"t":"@","p":26.83,"o":4547.19,"c":68,"pb":1220.03,"f":"pro"}]"#;

#[test]
fn the_coin_board_reads_both_ends_of_the_day() {
    let rows = coins(COINS);
    assert_eq!(rows.len(), 5);
    assert_eq!(rows[0].coin, "SOPH");
    assert!((rows[0].profit - 10_116.56).abs() < 1e-6);
    // The losing end keeps its sign: what the service pays and what it costs are one column, but
    // they are not one number.
    assert!(rows[4].profit < 0.0, "the worst coin came back positive");
    // And the column is ordered by MONEY, biggest gain first: the service counts its losing half
    // down from the smallest loss, so taken as it arrives this row would not be last.
    let ordered: Vec<f64> = rows.iter().map(|row| row.profit).collect();
    let mut sorted = ordered.clone();
    sorted.sort_by(|a, b| b.partial_cmp(a).expect("finite"));
    assert_eq!(ordered, sorted, "the day's coins came back out of order");
}

#[test]
fn the_trader_board_takes_the_money_from_pb_and_the_count_from_c() {
    // Not from `p`, which is on the same row and is NOT dollars: summed over fifty rows it comes
    // to 16.8k against a day total of 55.7k, while `pb` comes to 58.9k. That is what pins this.
    let rows = traders(TRADERS);
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].place, 1);
    assert_eq!(rows[0].name(), "moon_bot_kurilka");
    assert!((rows[0].profit - 45_550.51).abs() < 1e-6);
    assert_eq!(rows[0].trades, 54_052);
    assert!(rows[1].anonymous(), "a hidden trader came back named");
    // `i` here is the ACCOUNT, not the place — the same letter means the place on the coin board,
    // which is exactly why it is read off a real body and pinned by a test.
    assert_eq!(rows[0].id, 404);
    assert_eq!(rows[1].id, 40_729, "the hidden row lost its number");
    assert_ne!(
        u64::from(rows[1].place),
        rows[1].id,
        "the place was read as the id"
    );
}

#[test]
fn rubbish_is_dropped_a_row_at_a_time() {
    // One malformed row must not cost the whole board: the screen showing nine coins is better
    // than the screen showing none, and a defaulted row would be a number nobody sent.
    let mixed = r#"[{"i":1,"c":"SOPH","p":10116.56},{"i":2,"p":6165},{"i":3,"c":"","p":1},
    {"i":4,"c":"PONS","p":null},{"i":5,"c":"SKY","p":3761.66}]"#;
    let rows = coins(mixed);
    assert_eq!(rows.len(), 2, "kept {rows:?}");
    assert_eq!(rows[0].coin, "SOPH");
    assert_eq!(rows[1].coin, "SKY");
}

#[test]
fn a_body_that_is_not_a_board_is_an_empty_board() {
    // The service answering with a page of HTML, or with nothing at all, is a quiet screen — not
    // a panic on the thread that reads it.
    assert!(coins("<html>502 Bad Gateway</html>").is_empty());
    assert!(traders("").is_empty());
    assert!(coins("{}").is_empty());
    assert!(traders("[1, 2, 3]").is_empty());
}

#[test]
fn a_ticker_that_arrives_twice_is_taken_once() {
    // The coin board is keyed by its ticker the way the trader board is keyed by its account: two
    // rows with one name would share a place, a glow and a farewell.
    let doubled = r#"[{"i":1,"c":"SOPH","p":10116.56},{"i":2,"c":"SOPH","p":-4.0},
    {"i":3,"c":"PUMP","p":6165}]"#;
    let rows = coins(doubled);
    assert_eq!(rows.len(), 2, "kept {rows:?}");
    assert_eq!(rows[0].coin, "SOPH");
    assert!(
        (rows[0].profit - 10_116.56).abs() < 1e-6,
        "the second SOPH won"
    );
    assert_eq!(rows[1].coin, "PUMP");
}

#[test]
fn a_rank_that_does_not_fit_is_dropped_rather_than_clamped() {
    // Clamping would print 4294967295 in the first column of a board whose own type says "from
    // one" — a figure that cannot be true, standing where a rank belongs.
    let body = r#"[{"r":4294967296,"i":7,"t":"@x","c":3,"pb":10.0},
{"r":2,"i":8,"t":"@y","c":3,"pb":9.0}]"#;
    let rows = traders(body);
    assert_eq!(rows.len(), 1, "the impossible rank was kept");
    assert_eq!(rows[0].id, 8);
}

#[test]
fn a_trade_count_is_read_whichever_way_the_service_spells_the_number() {
    // It arrives as an integer today. A JSON float would otherwise silently become "0 trades"
    // beside a real profit — the defaulted lie this parser refuses everywhere else.
    let body = r#"[{"r":1,"i":7,"t":"@x","c":68.0,"pb":10.0}]"#;
    let rows = traders(body);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].trades, 68, "a float count was read as no trades");
}

#[test]
fn a_trader_row_without_an_account_is_dropped_rather_than_zeroed() {
    // The account is what the board is keyed by, so a defaulted one is not a missing figure — it
    // is two people wearing the same name. A repeat is dropped for the same reason, and the board
    // comes back in rank order whatever order it arrived in.
    let ragged = r#"[
    {"r":2,"i":40729,"t":"@","p":26.83,"o":4547.19,"c":68,"pb":1220.03,"f":"pro"},
    {"r":1,"i":404,"t":"@moon_bot_kurilka","p":12343.03,"o":369.04,"c":54052,"pb":45550.51},
    {"r":3,"t":"@nobody","pb":10.0,"c":1},
    {"r":4,"i":"777","t":"@string","pb":9.0,"c":1},
    {"r":5,"i":404,"t":"@twice","pb":8.0,"c":1}]"#;
    let rows = traders(ragged);
    assert_eq!(rows.len(), 2, "kept {rows:?}");
    assert_eq!(rows[0].place, 1, "the board came back out of rank order");
    assert_eq!(rows[0].id, 404);
    assert_eq!(rows[1].id, 40_729);
    assert!(
        rows.iter().all(|row| row.id != 0),
        "a row was given the account zero"
    );
}
