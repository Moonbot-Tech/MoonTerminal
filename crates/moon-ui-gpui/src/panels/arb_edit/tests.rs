use moon_core::config::{ArbShow, ArbViewCfg};
use moon_core::market::ArbVenue;

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
