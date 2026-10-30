use super::CoinMarkets;

#[test]
fn a_coin_is_searched_once_while_the_inputs_hold() {
    let mut markets = CoinMarkets::default();
    for _ in 0..3 {
        let answer = markets.resolve(|| 7_u64, "ACE", || Some("ACEUSDT".to_string()));
        assert_eq!(answer.as_deref(), Some("ACEUSDT"));
    }
    assert_eq!(markets.searched, 1);
}

/// A refusal is kept under the version it was read at, and the market that appears with a newer
/// version is found: caching "not found" without the version in the inputs would hide it.
#[test]
fn a_not_found_holds_until_the_catalog_version_moves() {
    let mut markets = CoinMarkets::default();
    assert_eq!(markets.resolve(|| 1_u64, "NEW", || None), None);
    assert_eq!(markets.resolve(|| 1_u64, "NEW", || panic!("kept")), None);
    let answer = markets.resolve(|| 2_u64, "NEW", || Some("NEWUSDT".to_string()));
    assert_eq!(answer.as_deref(), Some("NEWUSDT"));
    assert_eq!(markets.searched, 2);
}

#[test]
fn a_market_found_under_older_inputs_is_searched_again() {
    let mut markets = CoinMarkets::default();
    markets.resolve(|| 1_u64, "ACE", || Some("ACEUSDT".to_string()));
    let answer = markets.resolve(|| 2_u64, "ACE", || Some("ACEUSDC".to_string()));
    assert_eq!(answer.as_deref(), Some("ACEUSDC"));
    assert_eq!(markets.searched, 2);
}

/// Compare uncached catalog searches with the same revision cache on 100000 synthetic deals.
/// This is a lookup model, not a MoonProto client: it cannot establish native SDK call latency.
#[test]
#[ignore = "synthetic station catalog lookup model measurement"]
fn station_market_cache_measurement() {
    let catalog: Vec<_> = (0..5_500).map(|i| format!("COIN_{i}USDT")).collect();
    for _ in 0..5 {
        for cached in [false, true] {
            let mut markets = CoinMarkets::default();
            let started = std::time::Instant::now();
            let mut checksum = 0;
            for i in 0..100_000 {
                let coin = format!("COIN_{}", i % 200);
                let search = || {
                    catalog
                        .iter()
                        .find(|name| name.as_str() == format!("{coin}USDT"))
                        .cloned()
                };
                let answer = if cached {
                    markets.resolve(|| 1_u64, &coin, search)
                } else {
                    search()
                }
                .unwrap();
                assert_eq!(answer, format!("{coin}USDT"));
                checksum += answer.len();
            }
            println!(
                "market 100000 cached={cached}: {:.3} ms, searches={}, checksum={checksum}",
                started.elapsed().as_secs_f64() * 1_000.0,
                markets.searched
            );
        }
    }
}
