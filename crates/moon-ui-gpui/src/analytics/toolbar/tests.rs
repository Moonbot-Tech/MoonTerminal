//! Decision table for the "closed trades the core never dated" notice.
//!
//! The notice is the only thing that says money is missing from every figure on the window,
//! so the two ways it can go wrong — starting open, or being collapsible when the read
//! FAILED — are both worth pinning.

use std::collections::HashSet;

use moon_core::db::ReadFail;
use moon_core::db::analytics::UndatedCloses;

use super::super::AnalyticsSessionState;
use super::{UndatedBanner, sole_core_name, undated_banner_state};

/// Some undated trades, with money attached.
fn found(n: i64) -> Option<UndatedCloses> {
    Some(UndatedCloses { n, profit: -12.5 })
}

/// Nothing known yet, and a clean zero, both mean silence — not an empty band.
#[test]
fn nothing_to_say_renders_no_strip() {
    assert_eq!(
        undated_banner_state(None, None, false),
        UndatedBanner::None,
        "an unknown count says nothing"
    );
    assert_eq!(
        undated_banner_state(None, Some(UndatedCloses::default()), true),
        UndatedBanner::None,
        "zero undated trades says nothing even when expanded"
    );
}

/// The notice starts COLLAPSED and opens only when asked.
///
/// Plausible edit this catches: `AnalyticsSessionState::undated_expanded` is defaulted to
/// `true` (or the `!expanded` test is inverted while "simplifying" the branch), and every
/// user gets the full warning band on the tab they use most.
#[test]
fn the_notice_starts_collapsed_and_opens_only_on_request() {
    // The default is half the claim, so assert it here rather than leaving it to a source
    // grep: `undated_banner_state` only ever sees the value it is handed.
    assert!(
        !AnalyticsSessionState::default().undated_expanded,
        "a fresh process must start with the notice collapsed"
    );
    assert!(
        matches!(
            undated_banner_state(None, found(4), false),
            UndatedBanner::Collapsed(_)
        ),
        "collapsed unless the user opened it"
    );
    assert!(
        matches!(
            undated_banner_state(None, found(4), true),
            UndatedBanner::Full(..)
        ),
        "opened on request"
    );
}

/// A failed read outranks collapsing, in BOTH states.
///
/// Plausible edit this catches: the collapse check is moved above the failure check (it reads
/// like the cheaper guard), and a replica that could not be queried renders as a tidy
/// one-line count — claiming a number for rows nobody managed to read.
#[test]
fn a_read_failure_is_never_collapsed() {
    for expanded in [false, true] {
        assert!(
            matches!(
                undated_banner_state(Some(&ReadFail::NotReady), found(4), expanded),
                UndatedBanner::Failed(..)
            ),
            "a read failure must survive expanded={expanded}"
        );
    }
}

/// The tab bar names a core in exactly one case: a genuine one-of-many selection.
///
/// The rule is "name it exactly when the core trigger shows the count 1", i.e. exactly when the
/// name is otherwise unreachable without opening the dropdown.
///
/// Breakage this pins: testing only `selected.len() == 1` and dropping the all-cores guard. A
/// single-core install would then paint that core's name beside a trigger reading "All cores",
/// asserting a filter the query does not apply — and it would keep asserting it until a second
/// core connected.
#[test]
fn the_sole_core_name_shows_only_for_a_genuine_one_of_many_selection() {
    let two = vec![(1u64, "alpha".to_string()), (2u64, "beta".to_string())];
    let one = vec![(1u64, "alpha".to_string())];

    for (cores, selected, expected, why) in [
        (
            &two,
            vec![2u64],
            Some("beta"),
            "one of two is a real filter",
        ),
        (&two, vec![], None, "the implicit All names nothing"),
        (&two, vec![1, 2], None, "an explicit All names nothing"),
        (
            &one,
            vec![1],
            None,
            "the only core ticked still reads as All",
        ),
        (&one, vec![], None, "one core, implicit All"),
        // A deleted core keeps its id in the selection so the query cannot silently broaden. The
        // trigger counts only the ids that still resolve, so this reads as "1" there and must
        // name the one live core here.
        (
            &two,
            vec![1, 99],
            Some("alpha"),
            "one live core plus a stale id is still a sole selection",
        ),
    ] {
        let set: HashSet<u64> = selected.into_iter().collect();
        assert_eq!(sole_core_name(cores, &set), expected, "{why}");
    }
}

/// A selected id with no core behind it names nothing at all.
///
/// A core deleted from config keeps its id in the saved selection (deliberately — a stale id must
/// not silently broaden the query), so this is reachable in normal use.
///
/// Breakage this pins: resolving with a positional fallback such as `cores.first()`. The bar would
/// then name an unrelated core, and the number beside it would belong to a third one.
#[test]
fn a_stale_selected_id_names_no_core() {
    let cores = vec![(1u64, "alpha".to_string()), (2u64, "beta".to_string())];
    let set: HashSet<u64> = [99u64].into_iter().collect();

    assert_eq!(
        sole_core_name(&cores, &set),
        None,
        "an id no core answers to must not borrow another core's name"
    );
}
