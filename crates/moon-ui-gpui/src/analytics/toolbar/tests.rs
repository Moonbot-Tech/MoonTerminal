//! Decision table for the "closed trades the core never dated" notice.
//!
//! The notice is the only thing that says money is missing from every figure on the window,
//! so the two ways it can go wrong — starting open, or being collapsible when the read
//! FAILED — are both worth pinning.

use moon_core::db::ReadFail;
use moon_core::db::analytics::UndatedCloses;

use super::super::AnalyticsSessionState;
use super::{UndatedBanner, undated_banner_state};

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
