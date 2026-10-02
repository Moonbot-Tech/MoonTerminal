//! Missing settings enable distinct sounds, while persisted mute must survive TOML round trips.

use super::{TradeSounds, exchange_key};
use crate::config::layout::WindowLayout;
use crate::feed::ExchangeId;

#[test]
/// Old layouts must remain readable and missing edge settings receive distinct sounds.
fn absent_preferences_keep_backward_compatible_layout_defaults() {
    let layout: WindowLayout = toml::from_str("").unwrap();
    assert!(layout.trade_sounds.is_empty());
    assert_eq!(layout.trade_sound_volume_percent(), 100);
    assert_eq!(WindowLayout::default().trade_sound_volume_percent(), 100);
    let sounds: TradeSounds = toml::from_str("").unwrap();
    assert_eq!(sounds.open, "ringin");
    assert_eq!(sounds.close, "ringout");
}

/// Dropping the saved gain or replacing zero with a default would make muted trades audible.
#[test]
fn trade_volume_survives_restart_and_clamps_only_on_read() {
    for (stored, expected) in [(0, 0), (37, 37), (100, 100), (1000, 100)] {
        let layout: WindowLayout =
            toml::from_str(&format!("trade_sound_volume = {stored}")).unwrap();
        let restored: WindowLayout = toml::from_str(&toml::to_string(&layout).unwrap()).unwrap();
        assert_eq!(restored.trade_sound_volume_percent(), expected);
        assert_eq!(restored.trade_sound_volume, Some(stored));
    }
    let malformed: WindowLayout = toml::from_str("trade_sound_volume = 'invalid'").unwrap();
    assert_eq!(malformed.trade_sound_volume_percent(), 100);
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
