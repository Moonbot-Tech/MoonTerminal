use super::*;

/// Protects `layout.rs:WindowLayout::max_core_uid` from dropping active Main-core references.
///
/// The plausible edit is folding only `header_ticker` into the high-water mark. A deleted core
/// selected in a Main header could then have its UID reissued, rebinding the saved group selection
/// to an unrelated new core after restart.
#[test]
fn active_trade_core_selection_raises_uid_floor() {
    let mut layout = WindowLayout {
        header_ticker: Some(HeaderTicker {
            core_uid: 7,
            market: "BTCUSDT".to_string(),
        }),
        ..WindowLayout::default()
    };
    layout
        .active_trade_core_by_group
        .insert("default".to_string(), 42);

    assert_eq!(layout.max_core_uid(), Some(42));
}

/// Protects `layout.rs:WindowLayout::active_trade_core_by_group` across application restarts.
///
/// The plausible edit is marking the field `#[serde(skip)]`. The selector would appear sticky
/// during one process but silently return to the first core after saving and reloading layout.toml.
#[test]
fn active_trade_core_selection_survives_toml_round_trip() {
    let mut layout = WindowLayout::default();
    layout
        .active_trade_core_by_group
        .insert("Binance Futures".to_string(), 42);

    let encoded = toml::to_string(&layout).expect("WindowLayout must serialize to TOML");
    let decoded: WindowLayout =
        toml::from_str(&encoded).expect("serialized WindowLayout must deserialize");

    assert_eq!(
        decoded.active_trade_core_by_group.get("Binance Futures"),
        Some(&42)
    );
}
