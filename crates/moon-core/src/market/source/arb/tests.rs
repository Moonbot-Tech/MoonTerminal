use super::*;

/// A quote with only the fields deduplication reads.
fn raw(code: u8, dex: &str, price: f64, at_ms: i64, donor: CoreId) -> ArbRaw {
    ArbRaw {
        venue: ArbVenue::from_code(code),
        dex_name: dex.to_string(),
        price,
        my_price: 0.0,
        at_ms,
        donor,
        market: "BTCUSDT".to_string(),
        deposit_blocked: false,
        withdraw_blocked: false,
    }
}

/// A donor's market may only price the coin the way the chart does.
///
/// Breakage: accepting `ENABTC` for a USDT chart divides a BTC-denominated price by a dollar one
/// and prints −99.99 % on every venue — which reads as a crash, not as a mismatch.
#[test]
fn only_a_comparable_quote_currency_may_answer() {
    assert!(quotes_comparable("USDT", "USDT"));
    assert!(quotes_comparable("USDT", "usdt"));
    // A COIN-M chart is quoted in USD and has to be able to borrow from a USDT donor.
    assert!(quotes_comparable("USD", "USDT"));
    assert!(quotes_comparable("USDC", "USDT"));
    assert!(!quotes_comparable("USDT", "BTC"));
    assert!(!quotes_comparable("BTC", "USDT"));
    assert!(!quotes_comparable("BTC", "ETH"));
}

/// An unlabelled market must not empty the column.
///
/// The catalog leaves the quote empty for a market it does not hold, and the chart still shows a
/// price for it.
#[test]
fn an_unknown_quote_currency_is_accepted() {
    assert!(quotes_comparable("", "USDT"));
    assert!(quotes_comparable("USDT", ""));
}

#[test]
fn one_row_per_venue_keeps_the_freshest() {
    let rows = vec![
        raw(9, "", 1.0, 1_000, 7),
        raw(9, "", 2.0, 5_000, 8),
        raw(9, "", 3.0, 2_000, 9),
    ];
    let out = MarketDataSource::arb_dedupe(rows, 1);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].price, 2.0);
}

#[test]
fn the_readers_own_core_wins_over_a_fresher_donor() {
    // Only the reader's own core carries a `my_price` stamped with the quote, so its row states the
    // better spread even when another core filed the same number a moment later.
    let rows = vec![raw(9, "", 2.0, 9_000, 8), raw(9, "", 1.0, 1_000, 3)];
    let out = MarketDataSource::arb_dedupe(rows, 3);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].donor, 3);
}

#[test]
fn a_tie_goes_to_the_lowest_core_id() {
    // Cores live in a `HashMap`; without this two panes on one coin could print two prices.
    let a = MarketDataSource::arb_dedupe(vec![raw(9, "", 1.0, 1_000, 5), raw(9, "", 2.0, 1_000, 4)], 1);
    let b = MarketDataSource::arb_dedupe(vec![raw(9, "", 2.0, 1_000, 4), raw(9, "", 1.0, 1_000, 5)], 1);
    assert_eq!(a[0].donor, 4);
    assert_eq!(b[0].donor, 4);
}

#[test]
fn deployers_are_separate_rows_and_an_exchange_is_not_a_deployer() {
    // Every Hyperliquid deployer shares one code, so the DEX name is the whole identity; a bare
    // code must not collapse into one of them.
    let rows = vec![
        raw(ArbVenue::DEPLOYER_BASE + 1, "hyna", 1.0, 1_000, 4),
        raw(ArbVenue::DEPLOYER_BASE + 2, "para", 2.0, 1_000, 4),
        raw(13, "", 3.0, 1_000, 4),
    ];
    let out = MarketDataSource::arb_dedupe(rows, 1);
    assert_eq!(out.len(), 3);
}

#[test]
fn print_order_does_not_depend_on_the_order_cores_were_read_in() {
    let one = MarketDataSource::arb_dedupe(
        vec![raw(9, "", 1.0, 1_000, 4), raw(2, "", 2.0, 1_000, 5)],
        1,
    );
    let other = MarketDataSource::arb_dedupe(
        vec![raw(2, "", 2.0, 1_000, 5), raw(9, "", 1.0, 1_000, 4)],
        1,
    );
    let codes = |rows: &[ArbRaw]| rows.iter().map(|r| r.venue.code()).collect::<Vec<_>>();
    assert_eq!(codes(&one), vec![2, 9]);
    assert_eq!(codes(&other), codes(&one));
}

/// Reading the column while cores come and go must not deadlock.
///
/// The book is a `Mutex` inside the source's `RwLock`, so a read that held the book across a call
/// into the source would take the two in the opposite order from `remove_client` — the one bug in
/// this design that no single-threaded test can see and that hangs the whole application, frame
/// loop included, when it happens.
#[test]
fn reads_and_client_churn_do_not_deadlock() {
    use std::sync::mpsc;
    use std::sync::Arc;

    let source = Arc::new(MarketDataSource::new(crate::market::MarketStore::shared(
        0.0,
    )));
    let mut providers = std::collections::HashMap::new();
    providers.insert(7u64, 7u64);
    source.set_provider_map(&providers);

    let (tx, rx) = mpsc::channel();
    let readers: Vec<_> = (0..4)
        .map(|_| {
            let source = Arc::clone(&source);
            let tx = tx.clone();
            std::thread::spawn(move || {
                for _ in 0..200 {
                    let _ = source.market_arb(7, "BTCUSDT");
                }
                let _ = tx.send(());
            })
        })
        .collect();
    let churn = {
        let source = Arc::clone(&source);
        let tx = tx.clone();
        std::thread::spawn(move || {
            for _ in 0..200 {
                source.remove_client(7);
                source.set_core_venues(&std::collections::HashMap::new());
            }
            let _ = tx.send(());
        })
    };
    drop(tx);

    for _ in 0..5 {
        rx.recv_timeout(std::time::Duration::from_secs(30))
            .expect("arbitrage read deadlocked against client churn");
    }
    for handle in readers {
        handle.join().expect("reader panicked");
    }
    churn.join().expect("churn panicked");
}

#[test]
fn forgetting_a_core_drops_its_picks_and_the_donor_roster() {
    let mut book = ArbBook::default();
    book.coins.insert(
        "ENA".to_string(),
        CoinEntry {
            built_ms: 1,
            rows: vec![raw(9, "", 1.0, 1, 4)],
        },
    );
    book.markets.insert(
        ("ENA".to_string(), "USDT".to_string(), 4),
        MarketPick {
            at_ms: 1,
            market: Some("ENAUSDT".to_string()),
        },
    );
    book.markets.insert(
        ("ENA".to_string(), "USDT".to_string(), 5),
        MarketPick {
            at_ms: 1,
            market: None,
        },
    );
    book.donors = Some((1, vec![4, 5]));

    book.forget_core(4);

    // The quotes name their donor, so every coin's entry is rebuilt rather than filtered.
    assert!(book.coins.is_empty());
    assert!(!book.markets.contains_key(&("ENA".to_string(), "USDT".to_string(), 4)));
    assert!(book.markets.contains_key(&("ENA".to_string(), "USDT".to_string(), 5)));
    assert!(book.donors.is_none());
}
