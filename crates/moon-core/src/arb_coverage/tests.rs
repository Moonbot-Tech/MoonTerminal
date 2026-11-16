use super::*;

/// One market's facts with the fields a test does not care about filled in.
fn market<'a>(name: &'a str, trading: bool, quotes: &'a [(u8, i64)]) -> MarketFacts<'a> {
    MarketFacts {
        name,
        trading,
        futures_type: "USDT",
        futures_flag: true,
        quotes,
    }
}

/// The gate is tested on a value, never through the process-wide one: every other test in the
/// binary shares that, and admitting a sweep here would make them race.
///
/// Breakage: without the `running` half a sweep slower than the period overlaps the next one and
/// two threads interleave their lines; without the period half the reconciliation tick starts a
/// full scan of every core once a second.
#[test]
fn a_sweep_is_admitted_once_per_period_and_never_while_one_runs() {
    let mut gate = Gate::default();
    assert!(gate.admit(1_000_000));
    assert!(
        !gate.admit(1_000_000 + SWEEP_PERIOD_MS * 3),
        "one is running"
    );
    gate.running = false;
    assert!(
        !gate.admit(1_000_000 + SWEEP_PERIOD_MS - 1),
        "period not over"
    );
    assert!(gate.admit(1_000_000 + SWEEP_PERIOD_MS));
}

/// A clock set back must not silence the channel until it catches up.
#[test]
fn a_clock_step_backwards_admits_at_once() {
    let mut gate = Gate::default();
    assert!(gate.admit(5_000_000));
    gate.running = false;
    assert!(gate.admit(1_000_000));
}

/// A REFUSED admission must return, and must leave the running sweep's gate closed.
///
/// The whole path, lock included, on a gate of this test's own — which is what the two tests above
/// do not reach. Shipped once without it: the guard was built before the answer was known, so a
/// refusal dropped it on the spot, and its `Drop` took the gate's mutex while the admission still
/// held it. The first tick was admitted and the second one, a second later, hung the UI thread
/// for good (2026-11-16, two starts of a measurement build, both frozen ~7 s in).
///
/// Run on a thread with a timeout so the regression FAILS here instead of hanging the suite.
#[test]
fn a_refused_sweep_returns_and_does_not_release_the_running_one() {
    let gate: &'static Mutex<Gate> = Box::leak(Box::new(Mutex::default()));
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let first = try_begin(gate, 1_000_000);
        let refused_in_period = try_begin(gate, 1_000_001).is_none();
        // A refusal that reset `running` would admit this one over the sweep still in flight.
        let refused_while_running = try_begin(gate, 1_000_000 + SWEEP_PERIOD_MS).is_none();
        let admitted_first = first.is_some();
        drop(first);
        let admitted_after_release = try_begin(gate, 1_000_000 + SWEEP_PERIOD_MS * 2).is_some();
        let _ = tx.send((
            admitted_first,
            refused_in_period,
            refused_while_running,
            admitted_after_release,
        ));
    });
    assert_eq!(
        rx.recv_timeout(std::time::Duration::from_secs(10)),
        Ok((true, true, true, true)),
        "a refused admission deadlocked on the gate, or released a sweep it did not own"
    );
}

/// A market is filed by its NEWEST quote, a venue by its own.
///
/// Breakage: bucketing a market by its oldest quote would call a market with one dead venue stale
/// while four live ones quote it; counting a venue fresh off the market's newest quote would hide
/// exactly the dead venue the per-venue column is there to show.
#[test]
fn a_market_is_as_fresh_as_its_newest_quote_and_a_venue_as_its_own() {
    let mut coverage = CoreCoverage::default();
    coverage.add_market(&market("AUSDT", true, &[(3, 10_000), (4, RECENT_MS + 1)]));
    assert_eq!(
        (
            coverage.quoted,
            coverage.fresh,
            coverage.recent,
            coverage.older
        ),
        (1, 1, 0, 0)
    );
    assert_eq!(
        coverage.venues[&3],
        VenueCount {
            markets: 1,
            fresh: 1
        }
    );
    assert_eq!(
        coverage.venues[&4],
        VenueCount {
            markets: 1,
            fresh: 0
        }
    );
}

/// The three ages are three buckets, each bound belonging to the younger one, and together they
/// are every quoted market.
#[test]
fn quote_age_decides_the_bucket() {
    let mut coverage = CoreCoverage::default();
    coverage.add_market(&market("FRESH", true, &[(3, FRESH_MS)]));
    coverage.add_market(&market("RECENT", true, &[(3, FRESH_MS + 1)]));
    coverage.add_market(&market("EDGE", true, &[(3, RECENT_MS)]));
    coverage.add_market(&market("OLD", true, &[(3, RECENT_MS + 1)]));
    assert_eq!((coverage.fresh, coverage.recent, coverage.older), (1, 2, 1));
    assert_eq!(
        coverage.fresh + coverage.recent + coverage.older,
        coverage.quoted
    );
}

/// A stamp in the future is a price that arrived, not an old one.
///
/// The stamp is the receiving client's own clock, so a step forward and back puts it ahead of
/// "now". Read as a huge age it would report a live stream as dead.
#[test]
fn a_negative_age_counts_as_fresh() {
    let mut coverage = CoreCoverage::default();
    coverage.add_market(&market("AUSDT", true, &[(3, -5_000)]));
    assert_eq!((coverage.quoted, coverage.fresh), (1, 1));
    assert_eq!(coverage.venues[&3].fresh, 1);
}

/// Every coverage figure is over TRADING markets, so the line has one denominator.
///
/// Breakage: counting a halted market's leftover slot as quoted lets `quoted` exceed `trading`
/// and fills `older` with every delisting the exchange keeps in its list — the coverage question
/// answered by dead markets. Counting a halted market with no quote as a gap does the same to
/// `unquoted_trading`.
#[test]
fn halted_markets_stay_out_of_the_coverage_figures() {
    let mut coverage = CoreCoverage::default();
    coverage.add_market(&market("LIVE", true, &[]));
    coverage.add_market(&market("QUOTED", true, &[(3, 1_000)]));
    coverage.add_market(&market("HALTED", false, &[]));
    coverage.add_market(&market("HALTED_OLD_SLOT", false, &[(3, RECENT_MS * 10)]));
    assert_eq!(coverage.markets, 4);
    assert_eq!(coverage.trading, 2);
    assert_eq!(
        coverage.quoted + coverage.unquoted_trading,
        coverage.trading
    );
    assert_eq!(coverage.older, 0);
    assert_eq!(coverage.halted_quoted, 1);
    assert_eq!(coverage.venues[&3].markets, 1);
    assert_eq!(coverage.unquoted_sample, vec!["LIVE".to_string()]);
}

/// The listing flag splits the samples, and each sample stops at its cap.
#[test]
fn samples_follow_the_listing_flag_and_stop_at_the_cap() {
    let mut coverage = CoreCoverage::default();
    let names: Vec<String> = (0..SAMPLE_LEN + 5).map(|i| format!("M{i}")).collect();
    for name in &names {
        coverage.add_market(&MarketFacts {
            name,
            trading: true,
            futures_type: "EMPTY",
            futures_flag: false,
            quotes: &[],
        });
    }
    coverage.add_market(&market("WITHFUT", true, &[]));
    assert_eq!(coverage.no_futures_sample.len(), SAMPLE_LEN);
    assert_eq!(coverage.unquoted_sample.len(), SAMPLE_LEN);
    assert_eq!(coverage.with_futures_sample, vec!["WITHFUT".to_string()]);
    assert_eq!(coverage.futures_types["EMPTY"], SAMPLE_LEN + 5);
    assert_eq!(coverage.futures_types["USDT"], 1);
    // The counter is not capped with the sample: the summary needs the real number.
    assert_eq!(coverage.unquoted_trading, SAMPLE_LEN + 6);
}

/// A venue the mask asks for is listed even when it never quoted, and pending settings are not
/// printed as an empty mask.
///
/// Breakage: listing only venues that reported makes a watched-and-silent venue look unwatched,
/// and printing `mask=0` before the settings arrive makes the first lines of every fill curve read
/// "this core stores nothing".
#[test]
fn the_summary_names_silent_venues_and_pending_settings() {
    let mut watched = CoreCoverage::new(Some(2), &[3, 4], true);
    watched.add_market(&market("AUSDT", true, &[(3, 1_000)]));
    let line = watched.summary();
    assert!(line.contains("mask=2 "), "{line}");
    assert!(line.contains("BinanceS:1/1"), "{line}");
    assert!(line.contains("BinanceF:0/0"), "{line}");

    let pending = CoreCoverage::new(None, &[], false);
    assert!(pending.summary().contains("mask=pending "));
    let unticked = CoreCoverage::new(Some(0), &[], false);
    assert!(unticked.summary().contains("mask=0 "));
}

/// An empty catalog is "not arrived", which the writer must be able to tell from a reading.
#[test]
fn a_core_with_no_markets_is_empty() {
    let mut coverage = CoreCoverage::default();
    assert!(coverage.is_empty());
    coverage.add_market(&market("AUSDT", false, &[]));
    assert!(!coverage.is_empty());
}
