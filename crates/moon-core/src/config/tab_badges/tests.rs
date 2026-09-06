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

/// Seen kinds are recorded per core, and marking REPLACES rather than unions.
///
/// Replace is what keeps the map both correct and bounded: a kind the core stopped reporting drops
/// out, so a finding that comes back lights the badge again — it is news. A union would remember
/// every kind a core ever had and silence a returning fault forever.
#[test]
fn seen_kinds_are_per_core_and_marking_replaces_the_set() {
    let mut s = TabBadgeSettings::default();
    assert!(!s.core_kind_seen("CoreStatus", "g", 1, 10));

    assert!(s.mark_core_kinds_seen("CoreStatus", "g", 1, &[10, 11, 10]));
    assert!(s.core_kind_seen("CoreStatus", "g", 1, 10));
    assert!(s.core_kind_seen("CoreStatus", "g", 1, 11));
    // Keyed per core and per group: neither shares the other's set.
    assert!(!s.core_kind_seen("CoreStatus", "g", 2, 10));
    assert!(!s.core_kind_seen("CoreStatus", "other", 1, 10));

    // The same set again changes nothing, so an idle panel cannot dirty the file every frame.
    assert!(!s.mark_core_kinds_seen("CoreStatus", "g", 1, &[11, 10]));

    // A kind that left the core leaves the set, and would light the badge if it returned.
    assert!(s.mark_core_kinds_seen("CoreStatus", "g", 1, &[11]));
    assert!(!s.core_kind_seen("CoreStatus", "g", 1, 10));

    // An empty set FORGETS the core rather than being a no-op.
    assert!(s.mark_core_kinds_seen("CoreStatus", "g", 1, &[]));
    assert!(!s.core_kind_seen("CoreStatus", "g", 1, 11));
    assert!(!s.mark_core_kinds_seen("CoreStatus", "g", 1, &[]));
}

/// A `tab_badges.json` written before this field existed still loads, keeping its watermarks.
///
/// The gate for any change to a persisted struct: an existing file on a user's machine must not be
/// reset by a new build. Tested against the OLD shape rather than a freshly serialized one, which
/// would only prove the new build agrees with itself.
#[test]
fn a_file_written_before_seen_kinds_existed_still_loads() {
    let old = r#"{"hidden":["News"],"merged":[],"seen":{"News/main":1717000000000}}"#;
    let s: TabBadgeSettings = serde_json::from_str(old).expect("an older file must still load");
    assert!(!s.counters_visible("News"), "the old switch survives");
    assert_eq!(s.watermark("News", "main"), 1_717_000_000_000);
    assert!(
        !s.core_kind_seen("CoreStatus", "main", 1, 10),
        "the absent map defaults to nothing seen, not to everything seen"
    );
}
