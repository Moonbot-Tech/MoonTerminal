use super::*;

/// A `storage.toml` written before `margin_min` existed — the shape every installed terminal
/// has on disk — loads with the default margin, and the other fields keep their values.
#[test]
fn old_storage_toml_without_margin_min_loads_with_the_default() {
    let text = "[strategies]
enabled = true
version_limit = 0

[trade_replay]
persist_trades = true
max_mb = 512
";
    let cfg: StorageCfg = toml::from_str(text).expect("old file parses");
    assert_eq!(cfg.trade_replay.margin_min, DEFAULT_TRADE_MARGIN_MIN);
    assert_eq!(cfg.trade_replay.max_mb, 512);
    assert!(cfg.trade_replay.persist_trades);
}

/// The margin round-trips through the file, and the ceiling is a property of the loader, not
/// of the parser: a hand-edited 600 parses as 600 and is clamped where `load` clamps it.
#[test]
fn margin_min_round_trips_and_the_ceiling_is_applied_on_load() {
    let mut cfg = StorageCfg::default();
    cfg.trade_replay.margin_min = 45;
    let text = toml::to_string(&cfg).expect("serialises");
    let back: StorageCfg = toml::from_str(&text).expect("parses");
    assert_eq!(back.trade_replay.margin_min, 45);
    let hand_edited: StorageCfg = toml::from_str(
        "[trade_replay]
margin_min = 600
",
    )
    .expect("parses");
    assert_eq!(hand_edited.trade_replay.margin_min, 600);
    assert_eq!(
        sanitize(hand_edited).trade_replay.margin_min,
        MAX_TRADE_MARGIN_MIN
    );
    assert_eq!(sanitize(back).trade_replay.margin_min, 45);
}
