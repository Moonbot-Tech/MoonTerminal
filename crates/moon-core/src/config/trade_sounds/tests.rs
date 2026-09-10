//! Missing settings enable distinct sounds, while persisted mute must survive TOML round trips.

use super::{TradeSounds, exchange_key};
use crate::config::layout::WindowLayout;
use crate::feed::ExchangeId;

#[test]
/// Old layouts must remain readable and missing edge settings receive distinct sounds.
fn absent_preferences_keep_backward_compatible_layout_defaults() {
    let layout: WindowLayout = toml::from_str("").unwrap();
    assert!(layout.trade_sounds.is_empty());
    let sounds: TradeSounds = toml::from_str("").unwrap();
    assert_eq!(sounds.open, "ringin");
    assert_eq!(sounds.close, "ringout");
}

#[test]
/// An explicit mute must not deserialize into an enabled default after restart.
fn muted_open_and_custom_close_survive_layout_roundtrip() {
    let mut layout = WindowLayout::default();
    layout.trade_sounds.insert(
        exchange_key(ExchangeId::new(1)),
        TradeSounds {
            open: String::new(),
            close: "ding2".into(),
        },
    );
    let restored: WindowLayout = toml::from_str(&toml::to_string(&layout).unwrap()).unwrap();
    let sounds = &restored.trade_sounds["1:0"];
    assert!(sounds.open.is_empty());
    assert_eq!(sounds.close, "ding2");
    assert_ne!(
        exchange_key(ExchangeId::with_dex(1, "xyz")),
        exchange_key(ExchangeId::new(1))
    );
}
