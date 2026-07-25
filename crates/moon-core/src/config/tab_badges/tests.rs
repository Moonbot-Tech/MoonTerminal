use super::TabBadgeSettings;

/// Catches making `mark_read` a plain assignment: a reconnect replays the core's news ring, so an
/// older item arriving after a newer one would rewind the watermark and resurrect news the user
/// already read as unread.
#[test]
fn a_lower_watermark_never_rewinds_the_read_mark() {
    let mut s = TabBadgeSettings::default();
    assert!(s.mark_read("News", "main", 1_000));
    assert!(!s.mark_read("News", "main", 900));
    assert_eq!(s.watermark("News", "main"), 1_000);
}

/// Catches keying the watermark by panel alone: two window groups watch different cores, so one
/// group's News being read would silently clear the other group's unread count.
#[test]
fn groups_keep_separate_watermarks() {
    let mut s = TabBadgeSettings::default();
    s.mark_read("News", "main", 5_000);
    assert_eq!(s.watermark("News", "second"), 0);
}

/// Catches inverting the `hidden` set: an absent entry must mean "counters shown", so a fresh
/// install shows badges without anyone opting in.
#[test]
fn counters_are_visible_and_split_until_switched_off() {
    let mut s = TabBadgeSettings::default();
    assert!(s.counters_visible("Orders"));
    assert!(!s.counters_merged("News"));
    assert!(s.set_counters_visible("Orders", false));
    assert!(!s.counters_visible("Orders"));
}

/// Catches bumping `rev` unconditionally: panels fold it into their repaint signature, so a
/// no-op write would repaint every open view on each render pass that touches the settings.
#[test]
fn rev_advances_only_on_a_real_change() {
    let mut s = TabBadgeSettings::default();
    s.set_counters_merged("News", true);
    let after_first = s.rev();
    assert!(!s.set_counters_merged("News", true));
    assert_eq!(s.rev(), after_first);
    assert!(s.set_counters_merged("News", false));
    assert_eq!(s.rev(), after_first + 1);
}
