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
    // The autoload switch came after the margin and reads as OFF from a file without it: it
    // spends the venues' budget, and may not switch itself on.
    assert!(!cfg.trade_replay.autoload_missing);
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
autoload_missing = true
";
    let cfg: StorageCfg = toml::from_str(text).expect("old file parses");
    assert_eq!(cfg.trade_replay.margin_s, 900);
    assert!(!cfg.trade_replay.persist_trades);
    assert_eq!(cfg.trade_replay.max_mb, 64);
    assert!(cfg.trade_replay.autoload_missing);
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
    assert_eq!(snap_trade_margin_s(0), 5);
    assert_eq!(snap_trade_margin_s(7), 5, "nearer to 5 than to 10");
    assert_eq!(snap_trade_margin_s(8), 10);
    assert_eq!(snap_trade_margin_s(19), 10);
    assert_eq!(snap_trade_margin_s(20), 10, "tie goes to the lower step");
    assert_eq!(snap_trade_margin_s(21), 30);
    assert_eq!(snap_trade_margin_s(u32::MAX), MAX_TRADE_MARGIN_S);

    assert_eq!(step_trade_margin_s(900, 1), 1800);
    assert_eq!(step_trade_margin_s(900, -1), 600);
    assert_eq!(step_trade_margin_s(900, 3), 7200);
    assert_eq!(
        step_trade_margin_s(900, 4),
        7200,
        "the top absorbs the rest"
    );
    assert_eq!(step_trade_margin_s(5, -1), 5, "so does the bottom");
    assert_eq!(
        step_trade_margin_s(2700, 1),
        3600,
        "from the snapped step, 1800"
    );
    assert_eq!(step_trade_margin_s(2700, 0), 1800);
}
