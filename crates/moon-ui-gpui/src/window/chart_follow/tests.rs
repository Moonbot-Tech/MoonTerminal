use super::{ChartFollow, charts_to_raise};

/// One core with the checkbox on is enough to tie the whole group's charts to the main window.
#[test]
fn any_core_on_makes_the_group_follow() {
    let mode = ChartFollow::for_group([Some(false), Some(true), Some(false)]);
    assert_eq!(mode, ChartFollow::FollowMain);
}

/// Only a group whose every core turned the checkbox off lets its charts go independent.
#[test]
fn every_core_off_makes_the_group_independent() {
    let mode = ChartFollow::for_group([Some(false), Some(false)]);
    assert_eq!(mode, ChartFollow::Independent);
}

/// A core whose settings have not arrived counts as MoonBot's default, on.
#[test]
fn unknown_core_counts_as_on() {
    assert_eq!(
        ChartFollow::for_group([Some(false), None]),
        ChartFollow::FollowMain
    );
}

/// A group with no core yet keeps the default rather than going independent.
#[test]
fn empty_group_follows_by_default() {
    assert_eq!(
        ChartFollow::for_group(std::iter::empty()),
        ChartFollow::FollowMain
    );
}

/// Following hides the charts' own taskbar buttons; independent charts keep theirs.
#[test]
fn taskbar_buttons_hidden_only_while_following() {
    assert!(ChartFollow::FollowMain.hides_chart_taskbar_buttons());
    assert!(!ChartFollow::Independent.hides_chart_taskbar_buttons());
}

/// An activation raises its own group's charts, in registry order, and no other group's.
#[test]
fn activation_raises_only_its_own_group() {
    let registry = vec![
        ("alpha".to_string(), 1u32),
        ("beta".to_string(), 2),
        ("alpha".to_string(), 3),
    ];
    assert_eq!(
        charts_to_raise("alpha", ChartFollow::FollowMain, &registry),
        vec![1, 3]
    );
    assert_eq!(
        charts_to_raise("beta", ChartFollow::FollowMain, &registry),
        vec![2]
    );
    assert!(charts_to_raise("gamma", ChartFollow::FollowMain, &registry).is_empty());
}

/// An independent group's activation raises nothing.
#[test]
fn independent_activation_raises_nothing() {
    let registry = vec![("alpha".to_string(), 1u32)];
    assert!(charts_to_raise("alpha", ChartFollow::Independent, &registry).is_empty());
}
