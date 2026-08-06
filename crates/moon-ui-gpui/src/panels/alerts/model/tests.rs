//! Unit tests for the Alerts panel's row rules: what the filters accept, and what the sort orders.

use super::{ALL_COLUMNS_MASK, AlCol, AlertsViewState, ArmFilter, FigRow, KindFilter, sort_rows};
use moon_core::session::CoreId;

/// Builds a row without going through `Figure`, so a test may state exactly the combination it is
/// about — including ones no tool produces today, such as an armed figure that cannot be armed.
#[allow(clippy::too_many_arguments)]
fn row(
    core: CoreId,
    id: u64,
    coin: &str,
    figure: &str,
    price: f64,
    time_ms: i64,
    alertable: bool,
    armed: bool,
) -> FigRow {
    FigRow {
        core,
        core_name: format!("core{core}"),
        market: format!("{coin}USDT"),
        coin: coin.to_string(),
        figure: figure.to_string(),
        price,
        time_ms,
        alertable,
        // A figure only ever ARRIVES from a core for a tool that core knows, so the two flags in
        // these fixtures move together; the row keeps them apart because `can_arm` also refuses a
        // shared figure.
        from_server: false,
        armed,
        can_arm: alertable,
        shared: false,
        core_online: true,
        id,
        strategy_id: 0,
        strategy: "—".to_string(),
    }
}

/// A kind filter that also hid unarmed figures — or an arm filter that hid Terminal-only ones —
/// would make the panel unable to show the very rows it exists to arm. The two are independent, and
/// every combination must survive both.
#[test]
fn the_kind_and_arm_filters_are_independent() {
    // Two Moonbot types, one armed and one not, plus a Terminal-only type that can never be armed
    // — the three states the panel actually holds. A row that is armed but not alertable is not
    // among them: the store cannot produce one, and pretending otherwise would test a fiction.
    let rows = [
        row(1, 1, "BTC", "Line", 10.0, 100, true, true),
        row(1, 2, "ETH", "Line", 20.0, 200, true, false),
        row(1, 3, "SOL", "Rect", 30.0, 300, false, false),
    ];
    let cases: [(KindFilter, ArmFilter, &[u64]); 6] = [
        (KindFilter::All, ArmFilter::All, &[1, 2, 3]),
        (KindFilter::Moonbot, ArmFilter::All, &[1, 2]),
        (KindFilter::Terminal, ArmFilter::All, &[3]),
        (KindFilter::All, ArmFilter::Armed, &[1]),
        (KindFilter::All, ArmFilter::Unarmed, &[2, 3]),
        (KindFilter::Moonbot, ArmFilter::Unarmed, &[2]),
    ];
    for (kind, arm, expected) in cases {
        let view = AlertsViewState {
            kind,
            arm,
            ..AlertsViewState::default()
        };
        let got: Vec<u64> = rows
            .iter()
            .filter(|r| view.scope_accepts(r.alertable, r.armed))
            .map(|r| r.id)
            .collect();
        assert_eq!(got, expected, "kind={kind:?} arm={arm:?}");
    }
}

/// The query has to match the COIN, not only the raw market name: on an exchange whose market is an
/// index (`@156`) the raw name never contains the token the user typed. It also has to match in
/// either case — nobody types a ticker in capitals — and neither side is folded before the compare,
/// so the case-insensitivity has to be in the matcher itself.
#[test]
fn the_coin_query_matches_the_resolved_coin_in_either_case() {
    let mut r = row(1, 1, "SOL", "Line", 1.0, 1, false, false);
    r.market = "@156".to_string();
    assert!(r.matches("SOL"));
    assert!(r.matches("sol"));
    assert!(r.matches("So"));
    assert!(r.matches("@15"));
    assert!(r.matches(""));
    assert!(!r.matches("BTC"));
    // A query longer than the field it is compared against must not match, and must not panic.
    assert!(!r.matches("SOLANAUSDTPERPETUAL"));
}

/// Sorting by a COARSE key — Kind, Alert — leaves most rows equal to each other. Without the
/// `(core, id)` tiebreak those rows would be free to swap places on every rebuild, and the table
/// would reshuffle under the cursor while nothing changed.
#[test]
fn equal_sort_keys_keep_one_stable_order() {
    let make = || {
        vec![
            row(2, 7, "BTC", "Line", 1.0, 10, false, false),
            row(1, 9, "ETH", "Line", 2.0, 20, false, false),
            row(1, 3, "SOL", "Line", 3.0, 30, false, false),
        ]
    };
    let mut a = make();
    let mut b = make();
    b.reverse();
    sort_rows(&mut a, AlCol::Kind, true);
    sort_rows(&mut b, AlCol::Kind, true);
    let key = |rows: &[FigRow]| rows.iter().map(|r| (r.core, r.id)).collect::<Vec<_>>();
    assert_eq!(key(&a), key(&b));
    assert_eq!(key(&a), vec![(1, 3), (1, 9), (2, 7)]);
}

/// The panel has always opened newest-first, and the default view state is what keeps that true.
#[test]
fn the_default_sort_is_newest_first() {
    let view = AlertsViewState::default();
    assert_eq!(view.sort, AlCol::Time);
    assert!(!view.sort_asc);
    let mut rows = vec![
        row(1, 1, "BTC", "Line", 1.0, 100, false, false),
        row(1, 2, "ETH", "Line", 2.0, 300, false, false),
        row(1, 3, "SOL", "Line", 3.0, 200, false, false),
    ];
    sort_rows(&mut rows, view.sort, view.sort_asc);
    assert_eq!(
        rows.iter().map(|r| r.time_ms).collect::<Vec<_>>(),
        vec![300, 200, 100]
    );
}

/// Descending must be the exact reverse of ascending for a key with no ties, or one of the two
/// directions is quietly doing something else.
#[test]
fn descending_reverses_ascending() {
    let make = || {
        vec![
            row(1, 1, "BTC", "Line", 3.5, 1, false, false),
            row(1, 2, "ETH", "Line", 1.25, 2, false, false),
            row(1, 3, "SOL", "Line", 2.0, 3, false, false),
        ]
    };
    let mut up = make();
    let mut down = make();
    sort_rows(&mut up, AlCol::Price, true);
    sort_rows(&mut down, AlCol::Price, false);
    down.reverse();
    assert_eq!(
        up.iter().map(|r| r.id).collect::<Vec<_>>(),
        down.iter().map(|r| r.id).collect::<Vec<_>>()
    );
    assert_eq!(up.iter().map(|r| r.id).collect::<Vec<_>>(), vec![2, 3, 1]);
}

/// Every column must round-trip through the key that is persisted, and the mask must cover them
/// all. A column added without a `key()` arm, or one whose key collides with another's, would
/// silently drop out of a saved layout.
#[test]
fn every_column_round_trips_through_its_persisted_key() {
    for col in AlCol::ALL {
        assert_eq!(AlCol::from_key(col.key()), Some(col), "{}", col.key());
        assert_ne!(col.bit(), 0);
        assert_ne!(ALL_COLUMNS_MASK & col.bit(), 0, "{}", col.key());
    }
    let keys: std::collections::HashSet<&str> = AlCol::ALL.iter().map(|c| c.key()).collect();
    assert_eq!(keys.len(), AlCol::ALL.len(), "two columns share one key");
}

/// Both filters are persisted to `docks.json` as a small integer, and both are read back on the
/// next launch. A variant added without an arm in one direction would silently reopen the panel on
/// a different filter than the one it was closed with.
#[test]
fn both_filters_round_trip_through_their_persisted_ordinal() {
    for f in [KindFilter::All, KindFilter::Moonbot, KindFilter::Terminal] {
        assert_eq!(KindFilter::from_u8(f.to_u8()), f, "{f:?}");
    }
    for f in [ArmFilter::All, ArmFilter::Armed, ArmFilter::Unarmed] {
        assert_eq!(ArmFilter::from_u8(f.to_u8()), f, "{f:?}");
    }
    // An ordinal from a newer build falls back to the unfiltered value rather than to a filter the
    // user never chose.
    assert_eq!(KindFilter::from_u8(200), KindFilter::All);
    assert_eq!(ArmFilter::from_u8(200), ArmFilter::All);
}

/// The action buttons are the panel's only way to delete a figure or open its settings, so the
/// column menu must never be able to hide them.
#[test]
fn the_action_column_can_be_neither_hidden_nor_sorted() {
    assert!(!AlCol::Actions.hideable());
    assert!(!AlCol::Actions.sortable());
    assert!(AlCol::ALL.iter().filter(|c| c.hideable()).count() == AlCol::ALL.len() - 1);
}
