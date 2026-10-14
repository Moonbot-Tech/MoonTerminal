//! Places a coin is blacklisted. Each case names the edit that would hide a row the menu counts.

use std::time::Duration;

use moon_core::config::TakeProfitMode;
use moon_core::feed::{ClientSettings, FIELD_COINS_BLACK_LIST, StrategyRow, TempBlacklistRow};
use moon_core::session::CoreId;
use moon_core::session::store::CoreData;

use super::{CoinPlace, find_coin_places, places_summary};

const CORE_A: CoreId = 11;
const CORE_B: CoreId = 22;

/// Arrival instant the temporary-ban deadlines below are measured from.
const BAN_AT_MS: i64 = 1_000_000;
const BAN_REMAINING: Duration = Duration::from_secs(3_600);

fn settings(enabled: bool, text: &str) -> ClientSettings {
    ClientSettings {
        take_profit_pct: 0.0,
        take_profit_main_pct: 0.0,
        take_profit_extended: false,
        take_profit_mode: TakeProfitMode::Normal,
        fixed_sell_mode: false,
        stop_loss_pct: 0.0,
        trailing_drop_pct: 0.0,
        use_global_take_profit: false,
        global_take_profit_pct: 0.0,
        panic_if_price_drop: false,
        emu_mode: false,
        buy_iceberg: false,
        sell_iceberg: false,
        sign_orders: false,
        use_stop_market: false,
        vol_drop_level: 0,
        use_blacklist: enabled,
        blacklist_text: text.to_string(),
        fixed_sell_pcts: [0.0; 6],
        fixed_sell_slot: 1,
        use_manual_strategy: false,
        manual_strategy_id: 0,
    }
}

fn strategy(id: u64, name: &str, folder: &str, blacklist: Option<&str>) -> StrategyRow {
    let mut fields = Vec::new();
    if let Some(text) = blacklist {
        fields.push((FIELD_COINS_BLACK_LIST.to_string(), text.to_string()));
    }
    StrategyRow {
        id,
        name: name.to_string(),
        kind: "Kind".to_string(),
        kind_ordinal: 0,
        folder_path: folder.to_string(),
        checked: false,
        is_short: false,
        fields,
    }
}

fn core_with(
    list: Option<(bool, &str)>,
    strategies: Vec<StrategyRow>,
    bans: &[(&str, Duration)],
) -> CoreData {
    let mut data = CoreData::default();
    if let Some((enabled, text)) = list {
        data.client_settings = Some(settings(enabled, text));
    }
    data.strategies = strategies;
    if !bans.is_empty() {
        data.temp_blacklist_at_ms = Some(BAN_AT_MS);
        data.temp_blacklist = bans
            .iter()
            .map(|(symbol, remaining)| TempBlacklistRow {
                symbol: (*symbol).to_string(),
                remaining: *remaining,
            })
            .collect();
    }
    data
}

/// `XALPHAUSDT` resolves to `xalpha`. Any other market is a catalogue miss.
fn base_of(_core: CoreId, market: &str) -> Option<String> {
    (market == "XALPHAUSDT").then(|| "xalpha".to_string())
}

fn find(cores: &[(CoreId, &CoreData)], coin: &str) -> Vec<CoinPlace> {
    find_coin_places(cores.iter().map(|(id, data)| (*id, *data)), coin, base_of)
}

/// Folding only one side, or comparing bytes, would miss a coin the core spelled in the other case.
#[test]
fn listed_coin_matches_when_only_the_case_differs() {
    let core = core_with(
        Some((true, "xalpha")),
        vec![strategy(1, "UNIT_01", "fold", Some("XaLpHa"))],
        &[("XALPHAUSDT", BAN_REMAINING)],
    );
    let places = find(&[(CORE_A, &core)], "XALPHA");
    assert_eq!(
        places,
        vec![
            CoinPlace::Core {
                core: CORE_A,
                list_enabled: true,
            },
            CoinPlace::Temp {
                core: CORE_A,
                market: "XALPHAUSDT".to_string(),
                until_ms: BAN_AT_MS + 3_600_000,
            },
            CoinPlace::Strategy {
                core: CORE_A,
                sid: 1,
                folder: "fold".to_string(),
                strategy: "UNIT_01".to_string(),
            },
        ]
    );
}

/// Treating the pair as the coin, or stripping a quote inside the list compare, would list a
/// different market. The token is the coin on both sides.
#[test]
fn a_pair_token_is_not_the_coin_and_the_coin_is_not_the_pair() {
    let pair_listed = core_with(Some((true, "XALPHAUSDT")), vec![], &[]);
    assert!(
        find(&[(CORE_A, &pair_listed)], "XALPHA").is_empty(),
        "a pair in the list is not the coin"
    );
    let coin_listed = core_with(
        Some((true, "XALPHA")),
        vec![strategy(1, "UNIT_01", "", Some("XALPHA"))],
        &[],
    );
    assert!(
        find(&[(CORE_A, &coin_listed)], "XALPHAUSDT").is_empty(),
        "the coin in the list is not the pair"
    );
}

/// Defaulting a missing field to the schema example, or to `*`, would invent a row the server
/// never stored. An omitted field is an empty list.
#[test]
fn a_strategy_missing_the_blacklist_field_adds_no_row() {
    let core = core_with(None, vec![strategy(7, "UNIT_01", "fold", None)], &[]);
    assert!(
        find(&[(CORE_A, &core)], "XALPHA").is_empty(),
        "no CoinsBlackList field and no core list"
    );
}

/// Hiding the row while the checkbox is off would send the user looking in the strategy file for
/// a coin that is sitting in the core text.
#[test]
fn a_disabled_core_list_still_reports_the_coin() {
    let core = core_with(Some((false, "XALPHA")), vec![], &[]);
    assert_eq!(
        find(&[(CORE_A, &core)], "XALPHA"),
        vec![CoinPlace::Core {
            core: CORE_A,
            list_enabled: false,
        }]
    );
}

/// Keying the temporary list by the coin, or keeping a market the catalogue cannot name, would
/// show a lift button for a ban this core does not hold under that spelling.
#[test]
fn a_temp_ban_matches_through_base_of_and_skips_an_unnamed_market() {
    let core = core_with(
        None,
        vec![],
        &[
            ("XALPHAUSDT", BAN_REMAINING),
            ("NOMARKET", BAN_REMAINING),
            ("XALPHAUSDT", Duration::ZERO),
        ],
    );
    assert_eq!(
        find(&[(CORE_A, &core)], "XALPHA"),
        vec![CoinPlace::Temp {
            core: CORE_A,
            market: "XALPHAUSDT".to_string(),
            until_ms: BAN_AT_MS + 3_600_000,
        }],
        "the unnamed market and the expired row are not places"
    );
}

/// Interpreting `*` as a wildcard would report every coin that shares a prefix. The terminal has
/// no mask semantics; the star is five characters that do not equal the coin.
#[test]
fn a_star_mask_is_not_a_wildcard() {
    let core = core_with(
        Some((true, "XAL*")),
        vec![strategy(1, "UNIT_01", "", Some("XAL*"))],
        &[],
    );
    assert!(find(&[(CORE_A, &core)], "XALPHA").is_empty());
    assert!(find(&[(CORE_A, &core)], "XAL").is_empty());
    let listed = find(&[(CORE_A, &core)], "XAL*");
    assert_eq!(listed.len(), 2, "the mask token matches only itself");
}

/// Sorting by insertion order, or emitting strategies before the core list, would make the dialog
/// rows jump when the store iteration order changes.
#[test]
fn cores_come_out_in_id_order_with_core_then_temp_then_strategy() {
    let low = core_with(
        Some((true, "XALPHA")),
        vec![
            strategy(1, "UNIT_01", "fold/a", Some("XALPHA")),
            strategy(2, "UNIT_02", "fold/skip", None),
            strategy(3, "UNIT_03", "", Some("zzz, XALPHA")),
        ],
        &[("XALPHAUSDT", BAN_REMAINING)],
    );
    let high = core_with(
        Some((false, "XALPHA")),
        vec![strategy(9, "UNIT_09", "other", Some("OTHER"))],
        &[("NOMARKET", BAN_REMAINING)],
    );
    let places = find(&[(CORE_B, &high), (CORE_A, &low)], "XALPHA");
    assert_eq!(
        places,
        vec![
            CoinPlace::Core {
                core: CORE_A,
                list_enabled: true,
            },
            CoinPlace::Temp {
                core: CORE_A,
                market: "XALPHAUSDT".to_string(),
                until_ms: BAN_AT_MS + 3_600_000,
            },
            CoinPlace::Strategy {
                core: CORE_A,
                sid: 1,
                folder: "fold/a".to_string(),
                strategy: "UNIT_01".to_string(),
            },
            CoinPlace::Strategy {
                core: CORE_A,
                sid: 3,
                folder: String::new(),
                strategy: "UNIT_03".to_string(),
            },
            CoinPlace::Core {
                core: CORE_B,
                list_enabled: false,
            },
        ]
    );
}

/// Counting a core once per row, or once per connected core whether or not it matched, would
/// print the wrong `N places · K cores` caption.
#[test]
fn summary_counts_places_and_distinct_cores() {
    assert_eq!(places_summary(&[]), (0, 0));
    let one = core_with(
        Some((true, "XALPHA")),
        vec![strategy(1, "UNIT_01", "", Some("XALPHA"))],
        &[],
    );
    let on_one = find(&[(CORE_A, &one)], "XALPHA");
    assert_eq!(places_summary(&on_one), (2, 1));
    let other = core_with(Some((true, "XALPHA")), vec![], &[]);
    let on_two = find(&[(CORE_B, &other), (CORE_A, &one)], "XALPHA");
    assert_eq!(places_summary(&on_two), (3, 2));
}
