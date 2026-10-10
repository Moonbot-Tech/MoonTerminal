use super::*;

/// Removing the restart gate would restore the exited letter; collapsing empty suffixes would
/// lose the distinction between a reported release and an unknown build.
#[test]
fn restart_letter_is_unknown_but_fresh_letters_are_preserved() {
    assert_eq!(letter_to_publish(Some("R3".into()), true), None);
    assert_eq!(letter_to_publish(Some(String::new()), true), None);
    assert_eq!(
        letter_to_publish(Some("R3".into()), false),
        Some("R3".into())
    );
    assert_eq!(
        letter_to_publish(Some(String::new()), false),
        Some(String::new())
    );
    assert_eq!(letter_to_publish(None, false), None);
}

/// `publish_gate.rs:server_info_predates_restart` must withhold only the allocation captured at
/// `ServerRestart`. Treating every snapshot as stale would blank the build after an ordinary
/// reconnect; treating none as stale would republish the exited process's letter.
///
/// Pinning that allocation is the caller's held `Arc<MoonStateSnapshot>` (`PinnedRestartInfo` in
/// `feed/live/mod.rs`). A unit test cannot force the allocator to reuse a freed address, so the
/// pin itself is not exercised here.
#[test]
fn a_server_restart_withholds_only_the_server_info_it_observed() {
    assert!(server_info_predates_restart(0x1000, Some(0x1000)));
    assert!(!server_info_predates_restart(0x2000, Some(0x1000)));
    assert!(
        !server_info_predates_restart(0x1000, None),
        "a same-process reconnect has no restart mark and must republish"
    );
}
