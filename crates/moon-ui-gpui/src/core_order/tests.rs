//! Canonical-order edge-case tests.

use super::*;
use moon_core::config::Secret;
use moon_core::config::{FeedFlags, ServerConfig};

/// Build a server fixture with explicit runtime and durable ids.
fn server(id: u64, uid: u64, name: &str) -> ServerConfig {
    ServerConfig {
        id,
        uid,
        name: name.to_string(),
        active: true,
        show_window: true,
        feed: FeedFlags::default(),
        key: Secret::default(),
        group: "default".to_string(),
        market: "Binance".to_string(),
        color: [0, 0, 0],
        synthetic: false,
        chart_bundle: String::new(),
        default_alert_strategy: 0,
        own_trade_config: false,
        strat_slots: None,
        manual_strategy: None,
        trade: None,
        transport: None,
    }
}

/// Cores that exist only in `reports.sqlite`, not in the current config, must land AFTER every
/// configured core and keep the order the query returned them in.
///
/// Protects `CoreOrder::from_db` on two counts: unknown cores must rank LAST (giving them
/// rank 0 instead of `u32::MAX` floats deleted cores above every configured one in the
/// Report and Analytics filters), and the tail must keep the query's own order, which
/// relies on `sort_by_key` being a stable sort — swapping it for `sort_unstable_by_key`
/// is the plausible "faster sort" edit that breaks it.
#[test]
fn historical_cores_form_a_stable_tail_after_the_configured_ones() {
    let servers = vec![server(1, 1, "Alpha"), server(2, 2, "Bravo")];
    let order = CoreOrder::from_parts(&servers, CoreSortMode::AddedOldest);
    let ordered = order.from_db(vec![
        (77, "Deleted-B".to_string()),
        (2, "Bravo".to_string()),
        (66, "Deleted-A".to_string()),
        (1, "Alpha".to_string()),
    ]);
    let names: Vec<&str> = ordered.iter().map(|(_, n)| n.as_str()).collect();
    assert_eq!(names, ["Alpha", "Bravo", "Deleted-B", "Deleted-A"]);
}

/// A server added in the Settings draft has no uid yet. It must rank newest, not oldest —
/// ranking `0` as a real uid would jump an unsaved row to the top of every list.
#[test]
fn an_unsaved_server_sorts_last_in_oldest_first_mode() {
    let servers = vec![
        server(9, 0, "Unsaved"),
        server(2, 2, "Second"),
        server(1, 1, "First"),
    ];
    let order = CoreOrder::from_parts(&servers, CoreSortMode::AddedOldest);
    assert!(order.rank(1) < order.rank(2), "uid 1 predates uid 2");
    assert!(order.rank(2) < order.rank(9), "an unsaved server is newest");
}

/// The same unsaved row must lead the list in newest-first mode.
///
/// Protects the `uid == 0 -> u64::MAX` mapping inside `insertion_key`. The plausible edit:
/// writing `AddedNewest` directly as `sort_by_key(|s| Reverse(s.uid))`, which reads as an
/// obvious mirror and drops the mapping. A row being edited in Settings would then sink to
/// the BOTTOM of every list in the mode whose whole promise is "newest first".
#[test]
fn an_unsaved_server_sorts_first_in_newest_first_mode() {
    let servers = vec![
        server(9, 0, "Unsaved"),
        server(2, 2, "Second"),
        server(1, 1, "First"),
    ];
    let order = CoreOrder::from_parts(&servers, CoreSortMode::AddedNewest);
    assert!(order.rank(9) < order.rank(2), "an unsaved server leads");
    assert!(order.rank(2) < order.rank(1), "uid 2 postdates uid 1");
}

/// The two insertion modes must be exact mirrors of each other.
///
/// Protects the shared `insertion_key`. The plausible edit: inlining one arm's key while
/// leaving the other alone, after which the two drift on the tie-break or the unsaved rule
/// and "newest first" stops being the reverse of "oldest first" for some inputs.
#[test]
fn the_two_insertion_modes_are_exact_mirrors() {
    // Includes an UNSAVED row (uid 0): without it both arms agree even when one drops the
    // `uid == 0 -> u64::MAX` mapping, and the mirror property would pass a broken build.
    let servers = vec![
        server(4, 40, "D"),
        server(1, 10, "A"),
        server(5, 0, "Unsaved"),
        server(3, 30, "C"),
        server(2, 20, "B"),
    ];
    let ids = [1u64, 2, 3, 4, 5];

    let oldest = CoreOrder::from_parts(&servers, CoreSortMode::AddedOldest);
    let newest = CoreOrder::from_parts(&servers, CoreSortMode::AddedNewest);

    let mut by_oldest: Vec<u64> = ids.to_vec();
    by_oldest.sort_by_key(|id| oldest.rank(*id));
    let mut by_newest: Vec<u64> = ids.to_vec();
    by_newest.sort_by_key(|id| newest.rank(*id));

    by_newest.reverse();
    assert_eq!(
        by_oldest, by_newest,
        "newest-first must be the exact reverse of oldest-first"
    );
}

/// Equal names use uid order so Name mode stays independent of the servers Vec order.
#[test]
fn duplicate_names_rank_deterministically_by_uid() {
    let servers = vec![server(5, 5, "Same"), server(3, 3, "Same")];
    let order = CoreOrder::from_parts(&servers, CoreSortMode::Name);
    assert!(order.rank(3) < order.rank(5));
}
