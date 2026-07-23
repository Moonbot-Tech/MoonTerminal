use super::build;
use moon_core::db::coin_lists::{CoinListRow, CoinListRows};
use std::collections::HashSet;

/// A fixed reference point, so the fixtures read as dates rather than magic numbers.
const NOW: i64 = 1_784_000_000_000;
const DAY: i64 = 86_400_000;

fn row(coin: &str, entries: &[&str], core: &str, since: Option<i64>) -> CoinListRow {
    CoinListRow {
        coin: coin.to_string(),
        entries: entries.iter().map(|e| e.to_string()).collect(),
        core_uid: 1,
        core_name: core.to_string(),
        since_ms: since,
        before_ms: None,
    }
}

fn set(items: &[&str]) -> HashSet<String> {
    items.iter().map(|s| s.to_string()).collect()
}

fn texts(coins: &[super::PickCoin]) -> Vec<String> {
    coins
        .iter()
        .map(|c| c.text.trim_end_matches(", ").to_string())
        .collect()
}

/// The field is the WORKING list, not what the database holds: a coin ticked in the table
/// must appear here, and one unticked must leave. Rendering the saved value instead made
/// the field sit still while the badge beside it counted the edits.
#[test]
fn the_field_follows_the_edit_not_the_saved_list() {
    let data = CoinListRows {
        white: Vec::new(),
        black: vec![row("BTC", &["BTC"], "BB1", Some(NOW - DAY))],
    };
    // Ticked but not saved.
    let f = build(&data, &set(&["BTC", "ETH"]), &set(&["BTC"]));
    assert_eq!(texts(&f), vec!["ETH", "BTC"], "the new tick leads");
    assert!(f[0].pending && !f[1].pending);

    // Unticked: gone from the field even though the database still lists it.
    let f = build(&data, &set(&[]), &set(&["BTC"]));
    assert!(f.is_empty());
}

/// A pending tick has no date and no core — claiming either would say the strategy
/// already holds it.
#[test]
fn a_pending_tick_claims_no_date_and_no_core() {
    let f = build(&CoinListRows::default(), &set(&["ETH"]), &set(&[]));
    assert_eq!(f.len(), 1);
    assert_eq!(f[0].at, None);
    assert_eq!(f[0].fresh, 1.0, "just added — the brightest there is");
    assert!(!f[0].tip.contains('·') || f[0].tip.matches('·').count() == 1);
}

/// The value is the entries AS WRITTEN. A list holding `BTC, BTC_0626, BTC_0925` matches
/// one coin but holds three entries, and writing back only `BTC` would delete two.
#[test]
fn every_written_entry_survives_the_fold() {
    let data = CoinListRows {
        white: Vec::new(),
        black: vec![row(
            "BTC",
            &["BTC", "BTC_0626", "BTC_0925"],
            "BB1",
            Some(NOW - DAY),
        )],
    };
    let f = build(&data, &set(&["BTC"]), &set(&["BTC"]));
    assert_eq!(texts(&f), vec!["BTC", "BTC_0626", "BTC_0925"]);
}

/// Across cores an EXACT date outranks a bound: the read already suppresses the bound
/// wherever it knows the real thing, and merging on "whichever is later" put one core's
/// "no later than" over another core's actual date.
#[test]
fn an_exact_date_outranks_another_cores_bound() {
    let mut bound = row("MAGMA", &["MAGMA"], "GateF", None);
    bound.before_ms = Some(NOW - DAY);
    bound.core_uid = 2;
    let exact = row("MAGMA", &["MAGMA"], "BB1", Some(NOW - 40 * DAY));
    let f = build(
        &CoinListRows {
            white: Vec::new(),
            black: vec![bound, exact],
        },
        &set(&["MAGMA"]),
        &set(&["MAGMA"]),
    );
    assert_eq!(f.len(), 1);
    assert_eq!(
        f[0].at,
        Some(NOW - 40 * DAY),
        "the real date, not the bound"
    );
}

/// Brightness is an ABSOLUTE age, so two coins added minutes apart look the same however
/// the rest of the list is spread. Normalising over the list's own span rendered a
/// One date for the whole list means every coin in it is equally the latest, so nothing
/// is dimmed - whatever day that was. The ramp answers "which of these came last", not
/// "how old is this", and most real lists carry exactly one date.
#[test]
fn a_single_date_list_is_uniformly_bright() {
    for age in [0, 400 * DAY] {
        let data = CoinListRows {
            white: Vec::new(),
            black: vec![
                row("A", &["A"], "BB1", Some(NOW - age)),
                row("B", &["B"], "BB1", Some(NOW - age)),
            ],
        };
        let f = build(&data, &set(&["A", "B"]), &set(&["A", "B"]));
        assert!(
            f.iter().all(|c| c.fresh == 1.0),
            "a list saved in one go is all bright, age={age}"
        );
    }
}

/// With a spread the newest sits at the bright end and the oldest at the dim one. The
/// ramp always spends its whole range on whatever this list holds, so it is a ranking
/// inside the list and never a statement about calendar age.
#[test]
fn a_spread_runs_newest_bright_to_oldest_dim() {
    let data = CoinListRows {
        white: Vec::new(),
        black: vec![
            row("NEW", &["NEW"], "BB1", Some(NOW)),
            row("MID", &["MID"], "BB1", Some(NOW - 10 * DAY)),
            row("OLD", &["OLD"], "BB1", Some(NOW - 20 * DAY)),
        ],
    };
    let picked = set(&["NEW", "MID", "OLD"]);
    let f = build(&data, &picked, &picked);
    assert_eq!(texts(&f), vec!["NEW", "MID", "OLD"]);
    assert_eq!(f[0].fresh, 1.0);
    assert_eq!(f[2].fresh, 0.0);
    assert!(
        f[1].fresh > 0.0 && f[1].fresh < 1.0,
        "the middle is between"
    );
}

/// A coin dated in the future (a core clock ahead of ours) must stay inside the ramp and
/// must not push the others out of it.
#[test]
fn a_future_date_stays_inside_the_ramp() {
    let data = CoinListRows {
        white: Vec::new(),
        black: vec![
            row("FUTURE", &["FUTURE"], "BB1", Some(NOW + 30 * DAY)),
            row("PAST", &["PAST"], "BB1", Some(NOW - 30 * DAY)),
        ],
    };
    let picked = set(&["FUTURE", "PAST"]);
    let f = build(&data, &picked, &picked);
    assert!(f.iter().all(|c| (0.0..=1.0).contains(&c.fresh)));
    assert_eq!(f[0].fresh, 1.0);
    assert_eq!(f[1].fresh, 0.0);
}

/// The separator is a DRAWING detail: it belongs to `display`, never to `text`. `text`
/// is what Save writes into the strategy, and a ", " baked into it would be written
/// there too.
#[test]
fn the_separator_never_reaches_the_saved_text() {
    let data = CoinListRows {
        white: Vec::new(),
        black: vec![
            row("A", &["A"], "BB1", Some(NOW)),
            row("B", &["B"], "BB1", Some(NOW - DAY)),
        ],
    };
    let f = build(&data, &set(&["A", "B"]), &set(&["A", "B"]));
    assert!(
        f[0].display.ends_with(", "),
        "all but the last carry a separator"
    );
    assert!(!f[1].display.ends_with(", "), "and the last one does not");
    assert!(
        f.iter().all(|c| !c.text.contains(',')),
        "the written value holds entries only, never separators"
    );
}
