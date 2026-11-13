use super::*;

// The registry is process-wide: every test uses core ids and exchange keys of its own.

#[test]
fn a_core_that_named_no_exchange_records_nothing() {
    assert_eq!(claim(9_100_001, 0), None);
}

#[test]
fn the_first_delivering_core_owns_until_it_falls_silent() {
    let (a, b) = (9_100_011, 9_100_012);
    set_exchange(a, "src-own:00000000", None);
    set_exchange(b, "src-own:00000000", None);
    assert_eq!(claim(a, 1_000).as_deref(), Some("src-own:00000000"));
    assert_eq!(claim(b, 1_500), None);
    assert!(claim(a, 3_000).is_some());
    assert_eq!(claim(b, 3_000 + TAKEOVER_MS), None);
    assert!(claim(b, 3_000 + TAKEOVER_MS + 1).is_some());
    assert_eq!(claim(a, 3_000 + TAKEOVER_MS + 2), None);
}

#[test]
fn a_core_that_goes_away_hands_its_exchange_on_at_once() {
    let (a, b) = (9_100_021, 9_100_022);
    set_exchange(a, "src-gone:00000000", None);
    set_exchange(b, "src-gone:00000000", None);
    assert!(claim(a, 1_000).is_some());
    forget(a);
    assert!(claim(b, 1_001).is_some());
    assert_eq!(claim(a, 1_002), None);
}

#[test]
fn a_core_that_moves_to_another_exchange_releases_the_old_one() {
    let (a, b) = (9_100_031, 9_100_032);
    set_exchange(a, "src-move-a:00000000", None);
    set_exchange(b, "src-move-a:00000000", None);
    assert!(claim(a, 1_000).is_some());
    set_exchange(a, "src-move-b:00000000", None);
    assert_eq!(claim(a, 1_001).as_deref(), Some("src-move-b:00000000"));
    assert_eq!(claim(b, 1_002).as_deref(), Some("src-move-a:00000000"));
}

/// Collapsing owners to the exchange alone would discard the USDC core's market lists.
#[test]
fn different_account_currencies_record_under_the_same_storage_exchange() {
    let (a, b) = (9_100_041, 9_100_042);
    let exchange = "src-currencies:00000000";
    set_exchange(a, exchange, Some("USDT".into()));
    set_exchange(b, exchange, Some("USDC".into()));
    assert_eq!(claim(a, 1_000).as_deref(), Some(exchange));
    assert_eq!(claim(b, 1_001).as_deref(), Some(exchange));
    assert_eq!(claim(a, 2_000).as_deref(), Some(exchange));
}

/// Currency grouping must still deduplicate USDT lists and preserve existing history keys.
#[test]
fn same_account_currency_keeps_one_owner_and_the_legacy_storage_key() {
    let (a, b) = (9_100_051, 9_100_052);
    set_exchange(a, "src-usdt:00000000", Some("USDT".into()));
    set_exchange(b, "src-usdt:00000000", Some("USDT".into()));
    assert_eq!(claim(a, 1_000).as_deref(), Some("src-usdt:00000000"));
    assert_eq!(claim(b, 1_001), None);
}

/// Unknown cores must follow election's fallback when currencies arrive after their identity.
#[test]
fn unknown_currency_joins_alphabetical_then_usdt_group() {
    let (unknown, usdc, btc, usdt) = (9_100_061, 9_100_062, 9_100_063, 9_100_064);
    let exchange = "src-fallback:00000000";
    set_exchange(unknown, exchange, None);
    assert_eq!(claim(unknown, 1_000).as_deref(), Some(exchange));
    set_exchange(usdc, exchange, Some("USDC".into()));
    assert_eq!(claim(unknown, 1_001).as_deref(), Some(exchange));
    assert_eq!(claim(usdc, 1_002), None);
    set_exchange(btc, exchange, Some("BTC".into()));
    assert_eq!(claim(unknown, 1_003).as_deref(), Some(exchange));
    assert_eq!(claim(btc, 1_004), None);
    assert_eq!(claim(usdc, 1_005).as_deref(), Some(exchange));
    set_exchange(usdt, exchange, Some("USDT".into()));
    assert_eq!(claim(unknown, 1_006).as_deref(), Some(exchange));
    assert_eq!(claim(usdt, 1_007), None);
    assert_eq!(claim(btc, 1_008).as_deref(), Some(exchange));
}

/// A late account currency must free the old owner slot instead of waiting for takeover.
#[test]
fn a_late_currency_releases_the_previous_group() {
    let (a, b) = (9_100_071, 9_100_072);
    let exchange = "src-late:00000000";
    set_exchange(a, exchange, None);
    set_exchange(b, exchange, Some("USDT".into()));
    assert!(claim(a, 1_000).is_some());
    set_exchange(a, exchange, Some("USDC".into()));
    assert_eq!(claim(a, 1_001).as_deref(), Some(exchange));
    assert_eq!(claim(b, 1_002).as_deref(), Some(exchange));
}
