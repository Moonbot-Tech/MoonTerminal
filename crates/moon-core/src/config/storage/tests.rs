use super::*;

/// A `storage.toml` written before the margin existed at all — the shape the oldest installed
/// terminals have on disk — loads with the default margin, and the other fields keep their values.
#[test]
fn old_storage_toml_without_any_margin_loads_with_the_default() {
    let text = "[strategies]
enabled = true
version_limit = 0

[trade_replay]
persist_trades = true
max_mb = 512
";
    let cfg: StorageCfg = toml::from_str(text).expect("old file parses");
    assert_eq!(cfg.trade_replay.margin_s, DEFAULT_TRADE_MARGIN_S);
    assert_eq!(cfg.trade_replay.max_mb, 512);
    assert!(cfg.trade_replay.persist_trades);
    // The long-position threshold came after the margin and reads as the five minutes it was as
    // a constant; the startup cleanup may not switch itself on.
    assert_eq!(
        cfg.trade_replay.long_position_min,
        DEFAULT_LONG_POSITION_MIN
    );
    assert!(!cfg.trade_replay.cleanup_at_startup);
}

/// The long-position threshold round-trips through the file and is bounded where `load`
/// bounds it: a hand-edited zero — every trade "long" — becomes the floor, an hour past the
/// ceiling becomes the ceiling.
#[test]
fn long_position_min_round_trips_and_is_clamped_on_load() {
    let mut cfg = StorageCfg::default();
    cfg.trade_replay.long_position_min = 15;
    cfg.trade_replay.cleanup_at_startup = true;
    let text = toml::to_string(&cfg).expect("serialises");
    assert!(text.contains("long_position_min = 15"), "{text}");
    assert!(text.contains("cleanup_at_startup = true"), "{text}");
    let back: StorageCfg = toml::from_str(&text).expect("parses");
    assert_eq!(back.trade_replay.long_position_min, 15);
    assert!(back.trade_replay.cleanup_at_startup);
    let zero: StorageCfg = toml::from_str(
        "[trade_replay]
long_position_min = 0
",
    )
    .expect("parses");
    assert_eq!(sanitize(zero).trade_replay.long_position_min, 1);
    let huge: StorageCfg = toml::from_str(
        "[trade_replay]
long_position_min = 180
",
    )
    .expect("parses");
    assert_eq!(sanitize(huge).trade_replay.long_position_min, 120);
}

/// A file written while the margin was `margin_min` (minutes) — what every terminal installed
/// before 2026-09-20 has — reads as the same stretch in seconds, and the other fields survive
/// the detour through the raw shape.
#[test]
fn old_storage_toml_with_margin_min_reads_as_seconds() {
    let text = "[trade_replay]
persist_trades = false
max_mb = 64
margin_min = 15
";
    let cfg: StorageCfg = toml::from_str(text).expect("old file parses");
    assert_eq!(cfg.trade_replay.margin_s, 900);
    assert!(!cfg.trade_replay.persist_trades);
    assert_eq!(cfg.trade_replay.max_mb, 64);
    // A migrated value off the list lands on the nearest step where `load` snaps it — the
    // lower one on a tie, so 45 minutes becomes 30, not 60.
    let odd: StorageCfg = toml::from_str("[trade_replay]\nmargin_min = 45\n").expect("parses");
    assert_eq!(odd.trade_replay.margin_s, 2700);
    assert_eq!(sanitize(odd).trade_replay.margin_s, 1800);
    // The new key wins over the old one when a hand-edited file carries both.
    let both: StorageCfg =
        toml::from_str("[trade_replay]\nmargin_min = 15\nmargin_s = 30\n").expect("parses");
    assert_eq!(both.trade_replay.margin_s, 30);
}

/// The margin round-trips through the file under the new key only, and the snap is a property
/// of the loader, not of the parser: a hand-edited 600 minutes parses as is and is snapped
/// where `load` snaps it.
#[test]
fn margin_s_round_trips_and_the_snap_is_applied_on_load() {
    let mut cfg = StorageCfg::default();
    cfg.trade_replay.margin_s = 30;
    let text = toml::to_string(&cfg).expect("serialises");
    assert!(text.contains("margin_s = 30"), "{text}");
    assert!(!text.contains("margin_min"), "{text}");
    let back: StorageCfg = toml::from_str(&text).expect("parses");
    assert_eq!(back.trade_replay.margin_s, 30);
    let hand_edited: StorageCfg = toml::from_str(
        "[trade_replay]
margin_s = 36000
",
    )
    .expect("parses");
    assert_eq!(hand_edited.trade_replay.margin_s, 36000);
    assert_eq!(
        sanitize(hand_edited).trade_replay.margin_s,
        MAX_TRADE_MARGIN_S
    );
    assert_eq!(sanitize(back).trade_replay.margin_s, 30);
}

/// A file written while the steps started at 5 s — the default of 2026-09-21 is on disk in
/// every terminal that never touched the setting — loads at the new floor, 30 s: the tuner's
/// run-up and tail, which the setting is no longer padded to behind the tab's back.
#[test]
fn a_margin_under_the_floor_loads_at_the_floor() {
    for old in [5, 10] {
        let cfg: StorageCfg = toml::from_str(&format!("[trade_replay]\nmargin_s = {old}\n"))
            .expect("old file parses");
        assert_eq!(sanitize(cfg).trade_replay.margin_s, 30, "margin_s = {old}");
    }
    assert_eq!(
        i64::from(TRADE_MARGIN_STEPS_S[0]) * 1_000,
        crate::market::trade_replay::MODEL_PAD_MS,
        "the floor is the tuner's pad"
    );
}

/// The step list is what the snap and the stepper agree on: every step snaps to itself, the
/// ends absorb what lies beyond them, and the default and the ceiling are both members.
#[test]
fn snap_and_step_walk_the_step_list() {
    for &step in TRADE_MARGIN_STEPS_S {
        assert_eq!(snap_trade_margin_s(step), step);
    }
    assert!(TRADE_MARGIN_STEPS_S.contains(&DEFAULT_TRADE_MARGIN_S));
    assert_eq!(
        TRADE_MARGIN_STEPS_S.last().copied(),
        Some(MAX_TRADE_MARGIN_S)
    );
    assert_eq!(snap_trade_margin_s(0), 30);
    assert_eq!(snap_trade_margin_s(44), 30);
    assert_eq!(snap_trade_margin_s(45), 30, "tie goes to the lower step");
    assert_eq!(snap_trade_margin_s(46), 60);
    assert_eq!(snap_trade_margin_s(65), 65);
    assert_eq!(snap_trade_margin_s(62), 60);
    assert_eq!(snap_trade_margin_s(63), 65);
    assert_eq!(snap_trade_margin_s(u32::MAX), MAX_TRADE_MARGIN_S);

    assert_eq!(step_trade_margin_s(900, 1), 1800);
    assert_eq!(step_trade_margin_s(900, -1), 600);
    assert_eq!(step_trade_margin_s(900, 3), 7200);
    assert_eq!(
        step_trade_margin_s(900, 4),
        7200,
        "the top absorbs the rest"
    );
    assert_eq!(step_trade_margin_s(30, -1), 30, "so does the bottom");
    assert_eq!(step_trade_margin_s(60, 1), 65);
    assert_eq!(step_trade_margin_s(65, 1), 180);
    assert_eq!(step_trade_margin_s(65, -1), 60);
    assert_eq!(
        step_trade_margin_s(2700, 1),
        3600,
        "from the snapped step, 1800"
    );
    assert_eq!(step_trade_margin_s(2700, 0), 1800);
}
