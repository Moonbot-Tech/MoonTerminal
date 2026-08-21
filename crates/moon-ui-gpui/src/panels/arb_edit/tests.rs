use moon_core::config::{ArbShow, ArbViewCfg};
use moon_core::market::ArbVenue;

/// Moving a venue moves its whole ROW — the name and the colour travel with it. The list is
/// reordered by swapping entries, so a swap that lost anything would show up here first.
#[test]
fn moving_a_venue_carries_its_name_and_colour() {
    let mut cfg = ArbViewCfg::default();
    cfg.venues[0].name = "Мой спот".to_string();
    cfg.venues[0].color = Some(0x00FF00);
    let moved = cfg.venues[0].clone();

    cfg.venues.swap(0, 1);

    assert_eq!(cfg.venues[1], moved);
}

/// The window writes through `sanitize`, so a name typed past the limit is cut on the way in rather
/// than at some later load — which is what keeps the file and the screen showing the same string.
#[test]
fn a_typed_name_is_cut_where_the_file_would_cut_it() {
    let mut cfg = ArbViewCfg::default();
    cfg.venues[0].name = "Очень длинное имя площадки".to_string();

    cfg.sanitize();

    assert_eq!(
        cfg.venues[0].name.chars().count(),
        moon_core::config::ARB_NAME_MAX
    );
}

/// Reset returns the shipped roster: every venue, shown, unnamed, in the theme's colour.
#[test]
fn reset_restores_the_shipped_roster() {
    let mut cfg = ArbViewCfg::default();
    cfg.venues.truncate(2);
    cfg.venues[0].visible = false;
    cfg.show = ArbShow::Spread;

    cfg = ArbViewCfg::default();

    assert_eq!(
        cfg.venues.len(),
        ArbVenue::KNOWN.len() + ArbVenue::DEPLOYERS_SCANNED as usize
    );
    assert_eq!(cfg.show, ArbShow::PriceAndSpread);
    assert!(cfg.venues.iter().all(|v| v.visible && v.color.is_none()));
}
