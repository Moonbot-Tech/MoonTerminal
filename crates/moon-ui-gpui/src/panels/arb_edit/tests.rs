use moon_core::config::{ArbShow, ArbViewCfg};
use moon_core::market::ArbVenue;

/// Moving a venue moves its whole ROW — its visibility and colour travel with it. The list is
/// reordered by swapping entries, so a swap that lost anything would show up here first.
#[test]
fn moving_a_venue_carries_its_settings() {
    let mut cfg = ArbViewCfg::default();
    cfg.venues[0].color = Some(0x00FF00);
    cfg.venues[0].visible = false;
    let moved = cfg.venues[0].clone();

    cfg.venues.swap(0, 1);

    assert_eq!(cfg.venues[1], moved);
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
