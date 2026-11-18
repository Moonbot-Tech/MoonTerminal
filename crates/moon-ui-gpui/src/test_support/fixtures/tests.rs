use super::*;

/// A roster that omits a configured core silently shrinks session-first benchmark work.
#[test]
fn panel_fixture_aligns_session_config_and_store_ids() {
    let input = PanelFixture::new(20, 100, 5000, 3000, 300);
    let session_ids: Vec<_> = input.sessions.iter().map(|session| session.id).collect();
    let config_ids: Vec<_> = input
        .config
        .servers
        .iter()
        .map(|server| server.id)
        .collect();
    let mut store_ids: Vec<_> = input.store.cores().map(|(id, _)| id).collect();
    store_ids.sort_unstable();
    assert_eq!(session_ids, (1..=20).collect::<Vec<_>>());
    assert_eq!(session_ids, config_ids);
    assert_eq!(session_ids, store_ids);
    for (session, config) in input.sessions.iter().zip(&input.config.servers) {
        assert_eq!(session.name, config.name);
        assert_eq!(session.group, config.group);
        assert_eq!(session.group, "UNIT_01");
    }
}

/// An account or strategy name must be a numbered synthetic unit.
fn is_unit(name: &str) -> bool {
    name.strip_prefix("UNIT_")
        .is_some_and(|suffix| suffix.len() >= 2 && suffix.bytes().all(|b| b.is_ascii_digit()))
}

/// Accept only the synthetic coin vocabulary, with the quote suffix added for markets.
fn is_coin(name: &str) -> bool {
    let coin = name.strip_suffix("USDT").unwrap_or(name);
    matches!(coin, "XALPHA" | "XBETA")
        || coin
            .strip_prefix("XA")
            .is_some_and(|suffix| !suffix.is_empty() && suffix.bytes().all(|b| b.is_ascii_digit()))
}

/// Dropped feed rows or reused ids would shrink the benchmark workload at the specified scale.
#[test]
fn store_retains_the_requested_fleet_and_neutral_rows() {
    let store = core_store(20, 100, 5000, 3000, 300);
    let cfg = app_config(20);
    assert_eq!(store.cores().count(), 20);
    assert_eq!(cfg.servers.len(), 20);

    for (index, server) in cfg.servers.iter().enumerate() {
        assert_eq!(
            server.name,
            format!("CORE_{}", char::from(b'A' + index as u8))
        );
        assert!(is_unit(&server.group));
        assert!(is_coin(&server.market));
        let core = store
            .core(server.id)
            .expect("config id must resolve in the store");
        assert_eq!(core.strategies.len(), 100);
        assert_eq!(core.orders.len(), 250);
        assert_eq!(core.assets.rows.len(), 150);
        assert_eq!(core.detects.len(), 15);
        assert_eq!(core.latest_detect.len(), 15);

        for row in &core.strategies {
            assert!(is_unit(&row.name) && is_unit(&row.folder_path));
        }
        for row in &core.orders {
            assert!(is_coin(&row.market) && is_coin(&row.market_display) && is_coin(&row.coin));
            assert!(is_unit(&row.strat_name));
        }
        for row in &core.assets.rows {
            assert!(is_coin(&row.market) && is_coin(&row.coin));
        }
        for row in &core.detects {
            assert!(is_coin(&row.market) && is_unit(&row.strat_name));
        }
    }
}

/// Fleet totals keep remainder rows, and the 200-core case uses numeric names after CORE_Z.
#[test]
fn uneven_totals_and_large_core_names_are_preserved() {
    let store = core_store(3, 0, 7, 5, 2);
    assert_eq!(
        store.cores().map(|(_, c)| c.orders.len()).sum::<usize>(),
        7
    );
    assert_eq!(
        store.cores().map(|(_, c)| c.assets.rows.len()).sum::<usize>(),
        5
    );
    assert_eq!(
        store.cores().map(|(_, c)| c.detects.len()).sum::<usize>(),
        2
    );

    let cfg = app_config(200);
    assert_eq!(cfg.servers.len(), 200);
    for (index, server) in cfg.servers.iter().enumerate().skip(26) {
        assert_eq!(server.name, format!("CORE_{}", index + 1));
    }
}

/// Reversing source order or introducing invalid bars would benchmark a different chart path.
#[test]
fn replay_inputs_keep_their_counts_ordering_and_neutral_names() {
    let deals = deals(5000);
    assert_eq!(deals.len(), 5000);
    assert!(
        deals
            .iter()
            .all(|row| row.core_name == "CORE_A" && is_coin(&row.coin))
    );
    assert!(deals.windows(2).all(|w| w[0].close_ms < w[1].close_ms));

    let history = history(50_000);
    assert_eq!(history.len(), 50_000);
    assert!(history.iter().all(|row| is_coin(&row.coin)));
    assert!(history.windows(2).all(|w| w[0].close_date > w[1].close_date));

    let candles = candles(100_000);
    assert_eq!(candles.len(), 100_000);
    assert!(
        candles
            .windows(2)
            .all(|w| w[1].t_open_ms - w[0].t_open_ms == 60_000.0)
    );
    assert!(
        candles
            .iter()
            .all(|bar| bar.low <= bar.open && bar.close <= bar.high)
    );
}
