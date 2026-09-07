use super::*;

/// One row, spelled and timed as given.
fn row(symbol: &str, secs: u64) -> TempBlacklistRow {
    TempBlacklistRow {
        symbol: symbol.to_string(),
        remaining: Duration::from_secs(secs),
    }
}

/// A remainder counting down is what every row does between two snapshots; publishing it would
/// wake every reader once per snapshot to say nothing new.
#[test]
fn a_countdown_is_not_news() {
    let published = vec![row("ADAUSDT", 7200)];
    assert!(!worth_publishing(
        Some(&published),
        Duration::from_secs(100),
        &[row("ADAUSDT", 7100)]
    ));
    assert!(!worth_publishing(
        Some(&published),
        Duration::ZERO,
        &[row("ADAUSDT", 7200)]
    ));
    assert!(
        !worth_publishing(Some(&published), Duration::ZERO, &[row("adausdt", 7200)]),
        "a different case is the same row"
    );
}

/// Everything else is: the set changing, a row being respelled, extended, or cut short.
#[test]
fn an_edit_is_news() {
    let published = vec![row("ADAUSDT", 7200)];
    let cases: [(&str, Vec<TempBlacklistRow>); 5] = [
        ("the first state always publishes", Vec::new()),
        ("a row went away", Vec::new()),
        (
            "a row appeared",
            vec![row("ADAUSDT", 7200), row("PEPEUSDT", 600)],
        ),
        ("a row was respelled", vec![row("ADA", 7200)]),
        ("a ban was extended", vec![row("ADAUSDT", 10_800)]),
    ];
    assert!(
        worth_publishing(None, Duration::ZERO, &cases[0].1),
        "{}",
        cases[0].0
    );
    for (why, held) in &cases[1..] {
        assert!(
            worth_publishing(Some(&published), Duration::ZERO, held),
            "{why}"
        );
    }
    assert!(
        worth_publishing(Some(&published), Duration::ZERO, &[row("ADAUSDT", 600)]),
        "a ban cut short is news too — the regression that left a 20h countdown over a 2h ban"
    );
}
