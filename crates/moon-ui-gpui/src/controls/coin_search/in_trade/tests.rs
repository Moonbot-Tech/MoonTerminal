use super::*;
use moon_core::market::MarketLabel;

fn hit(core: CoreId, market: &str) -> CoinHit {
    CoinHit {
        core,
        market: market.to_string(),
        server: format!("Core {core}"),
        label: MarketLabel {
            coin: "BTC".to_string(),
            canonic: String::new(),
            quote: "USDT".to_string(),
            contract: None,
        },
        venue: None,
        in_trade: false,
    }
}

#[test]
fn an_open_order_or_position_counts_and_a_done_order_does_not() {
    let set = trading_markets(
        [("BTCUSDT", false), ("ETHUSDT", true)],
        [
            ("SOLUSDT", 1.5),
            ("ADAUSDT", 0.0),
            ("XRPUSDT", -2.0),
            ("DOGEUSDT", f64::NAN),
        ],
    );
    let mut got: Vec<&str> = set.iter().map(String::as_str).collect();
    got.sort_unstable();
    assert_eq!(got, ["BTCUSDT", "SOLUSDT", "XRPUSDT"]);
}

#[test]
fn marking_matches_the_exact_market_key_per_core() {
    // Core 1 trades the perpetual, core 2 a dated contract of the same coin, core 3 nothing.
    let trading: HashMap<CoreId, HashSet<String>> = HashMap::from([
        (1, HashSet::from(["BTCUSDT".to_string()])),
        (2, HashSet::from(["BTCUSDT_0925".to_string()])),
    ]);
    let mut hits = vec![hit(1, "BTCUSDT"), hit(2, "BTCUSDT"), hit(3, "BTCUSDT")];
    mark_hits(&mut hits, |core, market| {
        trading.get(&core).is_some_and(|set| set.contains(market))
    });
    let flags: Vec<bool> = hits.iter().map(|h| h.in_trade).collect();
    assert_eq!(flags, [true, false, false]);
}

#[test]
fn trading_cores_sort_first_and_both_sides_keep_their_order() {
    let mut members = vec![hit(1, "B"), hit(2, "B"), hit(3, "B"), hit(4, "B")];
    members[1].in_trade = true;
    members[3].in_trade = true;
    let order: Vec<CoreId> = in_trade_first(&members).iter().map(|h| h.core).collect();
    assert_eq!(order, [2, 4, 1, 3]);
    // The members themselves are untouched, so the row's pick stays the canonical first.
    assert_eq!(members[0].core, 1);
}

/// A removed core must not leave its whole traded-market set in the thread-local cache.
#[test]
fn removed_cores_leave_the_traded_market_cache() {
    CACHE.with(|cache| {
        cache.borrow_mut().insert(
            77,
            (
                (1, 1, 1),
                Rc::new(HashSet::from(["XALPHAUSDT".to_string()])),
            ),
        );
    });
    retain_live_cores(|core| core == 1);
    assert!(CACHE.with(|cache| cache.borrow().is_empty()));
}

/// Measure a synthetic day of 100,000 cached cores shrinking to the ten still configured.
#[test]
#[ignore = "manual traded-market cache retention scale measurement"]
fn bench_traded_market_cache_day() {
    let mut samples = Vec::new();
    for _ in 0..5 {
        CACHE.with(|cache| {
            let mut cache = cache.borrow_mut();
            for core in 0..100_000 {
                cache.insert(
                    core,
                    (
                        (1, 1, 1),
                        Rc::new(HashSet::from([format!("XALPHA{core}USDT")])),
                    ),
                );
            }
        });
        let before = CACHE.with(|cache| cache.borrow().len());
        let start = std::time::Instant::now();
        retain_live_cores(|core| core < 10);
        samples.push(start.elapsed().as_secs_f64() * 1_000.0);
        assert_eq!(
            (before, CACHE.with(|cache| cache.borrow().len())),
            (100_000, 10)
        );
        CACHE.with(|cache| cache.borrow_mut().clear());
    }
    samples.sort_by(f64::total_cmp);
    println!(
        "[bench] traded-market core sets retained=100000 -> 10 prune median={:.6} ms",
        samples[2]
    );
}
