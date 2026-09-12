//! The whole import in one pass — clipboard text to a saved group — where the unit tests of each
//! stage cannot see the seams between them: a keypad key that decodes but lands in the config
//! under a spelling the dispatcher would not match, a size that applies but the group's load-time
//! repair throws away.

use std::collections::HashSet;

use super::apply::apply_local;
use super::plan::{PlanContext, build_plan};
use super::schema_v7::build;
use super::transport::encode_mbsc7;
use crate::config::{
    AppConfig, ChartThemeSet, FeedFlags, GroupTradeSettings, HotkeysConfig, KeySlot,
    OrdersStyleSet, Secret, ServerConfig, WorkspaceMembership,
};

/// A clipboard buffer as Moonbot would write it, with the keypad on the six fixed-sell keys and a
/// scale on both preset rows that differs from everything the terminal ships.
fn clipboard() -> String {
    let mut payload = Vec::new();
    payload.extend_from_slice(b"MBSP");
    payload.push(7u8);
    payload.extend_from_slice(&1234u16.to_le_bytes());
    build::block(&mut payload, 1, &[0xAA; 3]);
    build::block(&mut payload, 2, &[0xBB; 5]);
    build::block(&mut payload, 3, &[]);
    build::block(&mut payload, 4, &build::theme_body());
    build::block(&mut payload, 5, &build::ini_body());
    let ui = build::ui_body(
        [50.0, 100.0, 250.0, 500.0, 1000.0, 2500.0],
        [0x70, 0x71, 0x72, 0x73, 0x74, 0x75], // F1..F6
        [0x6B, 0x6D, 0x2000 | 0x6B, 0x2000 | 0x6D, 0x6A, 0x6F], // keypad + - Shift+ Shift- * /
        [0.3, 0.7, 1.2, 2.5, 4.0, 8.0],
    );
    build::block(&mut payload, 6, &ui);
    encode_mbsc7(&payload)
}

/// Two groups, three cores, the third on its own per-core set.
fn config() -> AppConfig {
    let mut cfg = AppConfig::blank(None);
    for id in 1..=3u64 {
        cfg.servers.push(ServerConfig {
            id,
            uid: id,
            name: format!("s{id}"),
            active: true,
            show_window: true,
            feed: FeedFlags::default(),
            key: Secret::new(String::new()),
            group: if id <= 2 { "desk-a" } else { "desk-b" }.into(),
            market: "BINANCE_FUTURES".into(),
            color: [0xFF, 0xB3, 0x47],
            synthetic: false,
            chart_bundle: String::new(),
            default_alert_strategy: 0,
            own_trade_config: id == 3,
            strat_slots: None,
            manual_strategy: None,
            trade: None,
            transport: None,
            workspace_membership: WorkspaceMembership::default(),
        });
    }
    cfg
}

/// Pins the clipboard-to-group path, catching unknown-key refusals or changed application semantics.
#[test]
fn a_moonbot_buffer_lands_its_scales_and_keypad_keys_in_the_group() {
    let mb = super::parse_clipboard(&clipboard()).expect("the buffer parses");
    let (hotkeys, theme, orders) = (
        HotkeysConfig::default(),
        ChartThemeSet::default(),
        OrdersStyleSet::default(),
    );
    let plan = build_plan(
        &mb,
        &PlanContext {
            hotkeys: &hotkeys,
            theme: &theme,
            orders: &orders,
            ui_theme_light: false,
        },
    );
    // Every keypad key made it into the plan: the "not imported" list holds only the nine Moonbot
    // actions the terminal has no command for, never an unknown key.
    assert!(
        plan.unsupported_hotkeys
            .iter()
            .all(|u| !matches!(u.reason, super::preview::ImportReason::UnknownKey { .. })),
        "{:?}",
        plan.unsupported_hotkeys
    );
    let selected: HashSet<String> = plan.local_items().map(|c| c.id.clone()).collect();

    let mut cfg = config();
    let out = apply_local(&mut cfg, &plan, &selected, &[1, 2, 3]);
    assert!(out.unknown_ids.is_empty(), "{:?}", out.unknown_ids);

    // The keys, spelled as the dispatcher and the recorder spell them.
    assert_eq!(cfg.hotkeys.key(KeySlot::SellPreset(0)), "+");
    assert_eq!(cfg.hotkeys.key(KeySlot::SellPreset(1)), "-");
    assert_eq!(cfg.hotkeys.key(KeySlot::SellPreset(2)), "shift-+");
    assert_eq!(cfg.hotkeys.key(KeySlot::SellPreset(3)), "shift--");
    assert_eq!(cfg.hotkeys.key(KeySlot::SellPreset(4)), "*");
    assert_eq!(cfg.hotkeys.key(KeySlot::SellPreset(5)), "/");

    // The group of cores 1 and 2 took the scales; core 3's group did not, its core keeps its own.
    let trade = cfg.group("desk-a").trade;
    assert_eq!(
        trade.order_sizes_usd,
        [50.0, 100.0, 250.0, 500.0, 1000.0, 2500.0]
    );
    assert_eq!(trade.order_size_sel, 2); // bNum in `ui_body`
    assert_eq!(
        trade.exit.fixed_sell_pcts,
        [0.3f32, 0.7, 1.2, 2.5, 4.0, 8.0].map(f64::from)
    );
    assert_eq!(trade.exit.fixed_sell_slot, Some(2)); // sbNum 1 in `ui_body`
    assert_eq!(cfg.group("desk-b").trade, GroupTradeSettings::default());

    // What was written survives the trip through the file and the load-time repair unchanged.
    let text = toml::to_string(&trade).unwrap();
    let mut reloaded: GroupTradeSettings = toml::from_str(&text).unwrap();
    assert!(!reloaded.repair(), "{text}");
    assert_eq!(reloaded, trade);
}
