use super::schema_default;

/// A resolved unlisted market must keep the core address and unknown valuation in its job.
#[test]
fn unlisted_resolved_row_can_be_queued_without_a_directory_venue() {
    let source = moon_core::fixture::unlisted::source(1);
    let resolver = super::FetchResolver {
        source,
        quotes: Default::default(),
        addresses: Default::default(),
        btc_markets: Default::default(),
    };
    let address = std::sync::Arc::new(super::RowAddress {
        core_uid: 1,
        venue: None,
        exchange_key: moon_core::fixture::unlisted::EXCHANGE_KEY.into(),
        market: "XALPHA".into(),
        btc_market: None,
    });
    let deal = moon_core::db::tuner::ticks::Deal {
        report_uid: 200,
        core_uid: 1,
        core_name: "CORE_X".into(),
        strategy_id: 1,
        kind: "Spread".into(),
        coin: "XALPHA".into(),
        buy_ms: 100_000,
        close_ms: 110_000,
        buy_price: 1.0,
        sell_price: 1.01,
        spent: 100.0,
        is_short: false,
        sell_reason: String::new(),
        fact_pnl: 0.0,
        sizing: None,
        pnl_pct: false,
        profit: None,
        deltas: Default::default(),
        tick: None,
        pre_spike_ask: None,
        archived_take: None,
        fact_modifier: None,
        hook_depth_pct: None,
        hook_stated_take_pct: None,
        strike_detect: None,
        step_lag_ms: 0.0,
        round_trip_ms: None,
        stop_anchor: None,
        delta_track: None,
        bars: None,
        own_entry: None,
        buy_set_ms: None,
        corridor: None,
        entry_placed: None,
        gap: None,
    };
    let row = resolver
        .queued_row(deal, address, super::job::RowOrigin::User)
        .unwrap();
    assert_eq!(row.replay_address.exchange_key, "200:00000000");
    assert_eq!(row.replay_address.venue, None);
    assert_eq!(
        row.tick_value,
        moon_core::market::trade_replay::venue_caps::TickValue::Unknown
    );
}

/// moonproto sends a default only when it is not zero (`FLAG_DEFAULT_NZ`): a number or switch
/// field without one defaults to zero, which the table must hold — `PriceToSwitch2Stop`, left out
/// of every strategy, read as "no default" and the search turned `UseSecondStop` on without it.
#[test]
fn a_field_without_a_default_defaults_to_zero() {
    for type_name in [
        "Bool", "Int32", "Int64", "Double", "Single", "Byte", "Word", "UInt32",
    ] {
        assert_eq!(schema_default(type_name, None), Some(0.0), "{type_name}");
    }
    assert_eq!(schema_default("String", None), None);
    assert_eq!(schema_default("Unknown", None), None);
}

/// A switch's default arrives spelled `Yes`/`No` (`feed::strategies::fmt_field`) and is kept as
/// 1/0, the way `StrategyValues::bool` reads the table; a number keeps its value.
#[test]
fn a_switch_default_is_one_or_zero_and_a_number_keeps_its_value() {
    assert_eq!(schema_default("Bool", Some("Yes")), Some(1.0));
    assert_eq!(schema_default("Bool", Some("No")), Some(0.0));
    assert_eq!(schema_default("Double", Some("-2,5")), Some(-2.5));
    assert_eq!(schema_default("Double", Some("1.5%")), Some(1.5));
    assert_eq!(schema_default("String", Some("abc")), None);
}

/// Resolving off the UI thread gives the map the per-deal loop gave, one entry per distinct
/// `(core, coin)`.
#[test]
fn resolve_in_matches_the_per_deal_loop() {
    let resolver = || super::FetchResolver {
        source: moon_core::fixture::unlisted::source(1),
        quotes: Default::default(),
        addresses: Default::default(),
        btc_markets: Default::default(),
    };
    let deal = |report_uid: i64, coin: &str| moon_core::db::tuner::ticks::Deal {
        report_uid,
        coin: coin.into(),
        ..synthetic_deal()
    };
    let deals = [deal(1, "XALPHA"), deal(2, "XALPHA"), deal(3, "XNOPE")];
    let mut old = resolver();
    let expected: std::collections::HashMap<_, _> = deals
        .iter()
        .map(|d| ((d.core_uid, d.coin.clone()), old.address(d)))
        .collect();
    let resolved = super::resolve_in(&mut resolver(), &deals);
    let print = |map: &std::collections::HashMap<(u64, String), Option<_>>| {
        let mut entries: Vec<_> = map.iter().collect();
        entries.sort_by(|a, b| a.0.cmp(b.0));
        format!("{entries:?}")
    };
    assert_eq!(print(&resolved), print(&expected));
    assert_eq!(resolved.len(), 2);
}

fn synthetic_deal() -> moon_core::db::tuner::ticks::Deal {
    moon_core::db::tuner::ticks::Deal {
        report_uid: 1,
        core_uid: 1,
        core_name: "CORE_X".into(),
        strategy_id: 1,
        kind: "Spread".into(),
        coin: "XALPHA".into(),
        buy_ms: 100_000,
        close_ms: 110_000,
        buy_price: 1.0,
        sell_price: 1.01,
        spent: 100.0,
        is_short: false,
        sell_reason: String::new(),
        fact_pnl: 0.0,
        sizing: None,
        pnl_pct: false,
        profit: None,
        deltas: Default::default(),
        tick: None,
        pre_spike_ask: None,
        archived_take: None,
        fact_modifier: None,
        hook_depth_pct: None,
        hook_stated_take_pct: None,
        strike_detect: None,
        step_lag_ms: 0.0,
        round_trip_ms: None,
        stop_anchor: None,
        delta_track: None,
        bars: None,
        own_entry: None,
        buy_set_ms: None,
        corridor: None,
        entry_placed: None,
        gap: None,
    }
}
