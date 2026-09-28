//! Unit regressions for Mini App mass-action targeting and per-core list order.

use std::cmp::Ordering;
use std::collections::HashMap;

use moon_core::venue::CoreVenue;

use std::time::{Duration, Instant};

use moon_core::telegram::web::dto::StrategyPendingDto;

use super::{by_section, natural_cmp, scope_targets, strategy_pending};

/// `mini_app.rs:scope_targets` keeps only visible cores, in visible order, once each.
///
/// Mutation: return the requested ids unfiltered or skip the dedupe. A mass
/// trading switch then commands a core the owner does not see, or the same core
/// twice. Oracle: requested [3, 99, 1, 3] against visible [1, 2, 3] is [1, 3].
#[test]
fn scope_targets_drops_unknown_and_duplicate_cores() {
    assert_eq!(scope_targets(&[3, 99, 1, 3], &[1, 2, 3]), vec![1, 3]);
    assert_eq!(scope_targets(&[99], &[1, 2, 3]), Vec::<u64>::new());
}

/// `mini_app.rs:natural_cmp` reads digit runs as numbers and ignores letter case.
///
/// Mutation: compare the raw strings. "SUB ACC № 10" then sorts before "№ 9".
#[test]
fn natural_cmp_orders_numbers_by_value() {
    assert_eq!(natural_cmp("SUB ACC № 9", "SUB ACC № 10"), Ordering::Less);
    assert_eq!(natural_cmp("core 21", "core 3"), Ordering::Greater);
    assert_eq!(natural_cmp("alpha", "Beta"), Ordering::Less);
    assert_eq!(natural_cmp("core 007", "core 7"), Ordering::Less);
    assert_eq!(natural_cmp("core", "core 1"), Ordering::Less);
    assert_eq!(natural_cmp("x", "x"), Ordering::Equal);
}

/// `mini_app.rs:by_section` groups by the terminal's exchange sections and natural-sorts names.
///
/// Mutation: skip the in-section sort, or keep the input order across sections. Oracle: the
/// unidentified core leads (the terminal's unknown-first rule), each venue's cores stay together,
/// and "№ 9" precedes "№ 10" inside its section.
#[test]
fn by_section_groups_by_exchange_then_natural_name() {
    let venues = HashMap::from([
        (1, CoreVenue::identify(6, "", None)),
        (2, CoreVenue::identify(2, "", None)),
        (3, CoreVenue::identify(6, "", None)),
        (4, CoreVenue::identify(2, "", None)),
    ]);
    let rows = vec![
        (1u64, "№ 10".to_string()),
        (2, "b".to_string()),
        (3, "№ 9".to_string()),
        (5, "lost".to_string()),
        (4, "A".to_string()),
    ];
    let ordered = by_section(rows, &venues, |(id, _)| *id, |(_, name)| name);
    let ids: Vec<u64> = ordered.iter().map(|(_, (id, _))| *id).collect();
    assert_eq!(ids[0], 5, "the unidentified core leads, as in the terminal");
    // Section order oracle: the terminal's own partition of the two venues.
    let terminal = crate::core_order::exchange_sections([(2, venues.get(&2)), (6, venues.get(&1))]);
    let first_is_code_2 = terminal[0].1 == [2];
    let expected: [u64; 4] = if first_is_code_2 {
        [4, 2, 3, 1]
    } else {
        [3, 1, 4, 2]
    };
    assert_eq!(
        &ids[1..],
        &expected,
        "terminal section order, natural names inside"
    );
    assert_eq!(ordered[1].0, ordered[2].0);
    assert_ne!(ordered[2].0, ordered[3].0);
}

/// `mini_app.rs:strategy_pending` keeps a toggle Pending inside the 45 s window, then TimedOut
/// only while the row disagrees AND no fresh strategy list arrived.
///
/// Mutation: `if checked == wanted || rev_now != rev_before {` -> `if checked == wanted {`.
/// A core that rebuilt its strategy list without flipping the row then keeps the timed-out chip
/// forever. Oracle: the documented window (45 s) and the settle rule, not the function's output.
#[test]
fn strategy_pending_times_out_until_row_agrees_or_list_moves() {
    let sent = Instant::now();
    let entry = (true, sent, 5, 7);
    let at = |secs| sent + Duration::from_secs(secs);
    assert_eq!(
        strategy_pending(entry, 5, 7, false, at(44)),
        Some(StrategyPendingDto::Pending)
    );
    assert_eq!(
        strategy_pending(entry, 5, 7, false, at(46)),
        Some(StrategyPendingDto::TimedOut)
    );
    assert_eq!(strategy_pending(entry, 5, 7, true, at(46)), None);
    assert_eq!(
        strategy_pending(entry, 5, 8, false, at(46)),
        None,
        "a fresh strategy list after the window is the truth"
    );
}
