use super::*;
use crate::market::{ArbQuote, ArbVenue};

fn quote(code: u8, price: f64) -> ArbQuote {
    ArbQuote {
        venue: ArbVenue::from_code(code),
        dex_name: String::new(),
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

/// A venue is identified by its CODE, so recolouring it keeps its place — and its NAME is the
/// protocol's, which nothing here can change.
#[test]
fn a_venue_is_identified_by_its_code() {
    let mut cfg = ArbViewCfg::default();
    let venue = ArbVenue::from_code(9);
    let row = cfg
        .venues
        .iter_mut()
        .find(|v| v.code == venue.code())
        .expect("known venue");
    row.color = Some(0x00FF00);

    let quotes = [quote(9, 101.0)];
    let rows = cfg.arrange(&quotes);

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].label, venue.default_name());
    assert_eq!(rows[0].color, Some(0x00FF00));
}

/// Venues are spelled as the REFERENCE TERMINAL spells them — the wire carries no display name —
/// and a code nothing covers prints its number rather than a word this build invented.
#[test]
fn a_venue_is_named_the_way_the_reference_terminal_names_it() {
    assert_eq!(ArbVenue::from_code(3).default_name(), "BinanceS");
    assert_eq!(ArbVenue::from_code(4).default_name(), "BinanceF");
    assert_eq!(ArbVenue::from_code(9).default_name(), "GateF");
    // The OKX pair arrives on codes no protocol constant covers; the reference terminal's own list
    // is what identifies them. See `default_name` for how to confirm the order in one move.
    assert_eq!(ArbVenue::from_code(14).default_name(), "OkxS");
    assert_eq!(ArbVenue::from_code(15).default_name(), "OkxF");
    // A code nothing covers still prints its number rather than a guessed exchange.
    assert_eq!(ArbVenue::from_code(200).default_name(), "#200");
    assert_eq!(ArbVenue::deployer(3).default_name(), "HL #3");
}

/// A hand-edited file cannot smuggle in a second row for one venue: the two would print the same
/// venue twice, and the lookup would hand one of them the other's settings.
#[test]
fn sanitize_drops_duplicate_venues() {
    let mut cfg = ArbViewCfg {
        venues: vec![
            ArbVenueCfg::new(ArbVenue::from_code(4)),
            ArbVenueCfg::new(ArbVenue::from_code(4)),
        ],
        show: ArbShow::Price,
        mark_blocked: false,
        min_abs_pct: 0.0,
    };

    cfg.sanitize();

    assert_eq!(cfg.venues.len(), 1);
}

/// The file states what was changed and reads back identically — it travels between machines like
/// `theme.toml`, so a round trip that lost the roster would lose the user's colours.
#[test]
fn the_roster_round_trips_through_toml() {
    let mut cfg = ArbViewCfg {
        show: ArbShow::Spread,
        min_abs_pct: 0.5,
        ..ArbViewCfg::default()
    };
    cfg.venues[1].color = Some(0x112233);
    cfg.venues[2].visible = false;

    let text = toml::to_string_pretty(&cfg).expect("serializes");
    let back: ArbViewCfg = toml::from_str(&text).expect("parses");

    assert_eq!(back, cfg);
}

/// A hand-edited floor that is negative or not a number would hide every venue or none of them
/// unpredictably; both read as "the column broke".
#[test]
fn a_broken_floor_shows_everything() {
    let mut cfg = ArbViewCfg {
        min_abs_pct: f32::NAN,
        ..ArbViewCfg::default()
    };
    cfg.sanitize();
    assert_eq!(cfg.min_abs_pct, 0.0);

    cfg.min_abs_pct = -3.0;
    cfg.sanitize();
    assert_eq!(cfg.min_abs_pct, 0.0);
}

/// The floor drops a venue from the column entirely — including one the roster has never heard of,
/// which is appended rather than listed.
#[test]
fn the_floor_applies_to_listed_and_unlisted_venues_alike() {
    let cfg = ArbViewCfg {
        min_abs_pct: 1.0,
        ..ArbViewCfg::default()
    };
    let quotes = [quote(4, 100.2), quote(ArbVenue::deployer(9).code(), 100.1)];

    let rows = cfg.arrange(&quotes);

    assert!(rows.is_empty(), "neither moved enough to be worth a line");
}

/// A deployer is named by the CORE when the core knows it: `AuthCheck` carries `known_dexes`, the
/// live quote carries the name out of it, and only a core that sent no list leaves the numbered
/// spelling standing.
#[test]
fn a_deployer_takes_the_name_its_core_supplied() {
    let cfg = ArbViewCfg::default();
    let deployer = ArbVenue::deployer(3);
    let mut named = quote(deployer.code(), 101.0);
    named.dex_name = "hyna".to_string();

    let rows = cfg.arrange(std::slice::from_ref(&named));
    assert_eq!(
        rows[0].label, "HL_hyna",
        "the terminal's prefix, the core's word"
    );

    let unnamed = [quote(deployer.code(), 101.0)];
    let rows = cfg.arrange(&unnamed);
    assert_eq!(rows[0].label, deployer.default_name(), "no list, numbered");

    // The settings window has no quote to read a name off, but it does have the list itself.
    let names = vec![String::new(), "xyz".into(), "flx".into(), "hyna".into()];
    let row = cfg.row(deployer).expect("the roster lists it");
    assert_eq!(row.label_with(&names), "HL_hyna");
}
