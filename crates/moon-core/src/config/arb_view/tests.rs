use super::*;
use crate::market::{ArbQuote, ArbVenue};

fn quote(code: u8, price: f64) -> ArbQuote {
    ArbQuote {
        venue: ArbVenue::from_code(code),
        price,
        my_price: 100.0,
        spread_pct: (price - 100.0),
        deposit_blocked: false,
        withdraw_blocked: false,
    }
}

/// The shipped roster holds every venue a READ can produce — the named ones and the deployer
/// indices the reader scans — and shows all of them. Two reasons, and the second is the subtle one:
/// a column that started empty would look broken, and a venue absent from the roster prints but
/// cannot be renamed, recoloured or moved.
#[test]
fn the_default_roster_holds_every_venue_a_read_can_produce() {
    let cfg = ArbViewCfg::default();
    assert_eq!(
        cfg.venues.len(),
        ArbVenue::KNOWN.len() + ArbVenue::DEPLOYERS_SCANNED as usize
    );
    assert!(cfg.venues.iter().all(|v| v.visible));
    assert!(cfg.venues.iter().all(|v| v.name.is_empty()));
    for index in 0..ArbVenue::DEPLOYERS_SCANNED {
        let deployer = ArbVenue::deployer(index);
        assert!(
            cfg.row(deployer).is_some(),
            "{deployer:?} can be renamed only if the roster lists it"
        );
    }
}

/// The roster decides the ORDER, not the core: the map the quotes come out of has none, and a
/// column whose rows swapped places between revisions would reshape every run under it.
#[test]
fn rows_print_in_roster_order_and_skip_hidden_venues() {
    let mut cfg = ArbViewCfg {
        venues: vec![
            ArbVenueCfg::new(ArbVenue::from_code(9)),
            ArbVenueCfg::new(ArbVenue::from_code(4)),
            ArbVenueCfg::new(ArbVenue::from_code(3)),
        ],
        ..ArbViewCfg::default()
    };
    cfg.venues[1].visible = false;
    let quotes = vec![quote(3, 101.0), quote(4, 102.0), quote(9, 103.0)];

    let rows = cfg.arrange(&quotes);

    let codes: Vec<u8> = rows.iter().map(|r| r.quote.venue.code()).collect();
    assert_eq!(codes, vec![9, 3], "roster order, hidden venue dropped");
}

/// A venue the file has never heard of still prints — appended, at its defaults. It is a venue the
/// CORE started reporting, and hiding it until somebody configures it would hide its existence.
#[test]
fn an_unconfigured_venue_is_appended_rather_than_dropped() {
    let cfg = ArbViewCfg {
        venues: vec![ArbVenueCfg::new(ArbVenue::from_code(4))],
        ..ArbViewCfg::default()
    };
    let deployer = ArbVenue::deployer(2);
    let quotes = vec![quote(deployer.code(), 99.0), quote(4, 102.0)];

    let rows = cfg.arrange(&quotes);

    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].quote.venue.code(), 4);
    assert_eq!(rows[1].quote.venue, deployer);
    assert_eq!(rows[1].label, deployer.default_name());
}

/// A venue is identified by its CODE, so renaming it keeps its colour and its place.
#[test]
fn a_renamed_venue_keeps_its_row() {
    let mut cfg = ArbViewCfg::default();
    let venue = ArbVenue::from_code(9);
    let row = cfg
        .venues
        .iter_mut()
        .find(|v| v.code == venue.code())
        .expect("known venue");
    row.name = "Гейт фьючи".to_string();
    row.color = Some(0x00FF00);

    let quotes = [quote(9, 101.0)];
    let rows = cfg.arrange(&quotes);

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].label, "Гейт фьючи");
    assert_eq!(rows[0].color, Some(0x00FF00));
}

/// A hand-edited file cannot smuggle in a second row for one venue, or a name wide enough to push
/// the prices off the pane.
#[test]
fn sanitize_drops_duplicates_and_cuts_names() {
    let mut cfg = ArbViewCfg {
        venues: vec![
            ArbVenueCfg::new(ArbVenue::from_code(4)),
            ArbVenueCfg::new(ArbVenue::from_code(4)),
        ],
        show: ArbShow::Price,
        mark_blocked: false,
    };
    cfg.venues[0].name = "Ы".repeat(ARB_NAME_MAX + 10);

    cfg.sanitize();

    assert_eq!(cfg.venues.len(), 1);
    assert_eq!(cfg.venues[0].name.chars().count(), ARB_NAME_MAX);
}

/// The file states what was changed and reads back identically — it travels between machines like
/// `theme.toml`, so a round trip that lost the roster would lose the user's colours.
#[test]
fn the_roster_round_trips_through_toml() {
    let mut cfg = ArbViewCfg {
        show: ArbShow::Spread,
        ..ArbViewCfg::default()
    };
    cfg.venues[0].name = "Мой Binance".to_string();
    cfg.venues[1].color = Some(0x112233);
    cfg.venues[2].visible = false;

    let text = toml::to_string_pretty(&cfg).expect("serializes");
    let back: ArbViewCfg = toml::from_str(&text).expect("parses");

    assert_eq!(back, cfg);
}
