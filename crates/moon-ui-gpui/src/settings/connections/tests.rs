//! Deterministic regression coverage for the Connections hierarchy.

use super::sync_groups_from_servers;
use super::tab::{ServerRowMeta, exchange_sections, pending_server_indices, visible_group_rows};
use moon_core::config::{
    FeedFlags, GroupConfig, GroupExitSettings, GroupTradeSettings, Secret, ServerConfig,
    TakeProfitMode,
};

/// Build a minimal server fixture for preview-group synchronization.
fn server(group: &str) -> ServerConfig {
    ServerConfig {
        id: 1,
        uid: 1,
        name: "alpha".to_string(),
        active: true,
        show_window: true,
        feed: FeedFlags::default(),
        key: Secret::new(""),
        group: group.to_string(),
        market: "BTCUSDT".to_string(),
        color: [1, 2, 3],
        synthetic: false,
        chart_bundle: String::new(),
        default_alert_strategy: 0,
    }
}

/// `tab.rs:exchange_sections` must keep the unknown bucket first; changing its insertion to append
/// after known exchanges hides inactive or unidentified cores below the populated exchange list.
#[test]
fn exchange_sections_group_known_names_and_keep_unknown_first() {
    let servers: Vec<ServerRowMeta> = vec![
        (
            1,
            11,
            true,
            "default".to_string(),
            Some("Bybit".to_string()),
        ),
        (2, 12, true, "default".to_string(), None),
        (
            3,
            13,
            true,
            "default".to_string(),
            Some("Binance Futures".to_string()),
        ),
        (
            4,
            14,
            false,
            "default".to_string(),
            Some("Bybit".to_string()),
        ),
        (5, 15, true, "secondary".to_string(), None),
    ];

    let sections = exchange_sections(&servers, "default");
    let names: Vec<Option<&str>> = sections.iter().map(|(name, _)| *name).collect();
    let members: Vec<Vec<usize>> = sections
        .iter()
        .map(|(_, members)| members.clone())
        .collect();

    assert_eq!(names, vec![None, Some("Binance Futures"), Some("Bybit")]);
    assert_eq!(members, vec![vec![1], vec![2], vec![0, 3]]);
}

/// `tab.rs:pending_server_indices` must select `uid == 0`; reversing that predicate replaces the
/// top section with saved cores while new cores remain excluded from groups, hiding their fields.
#[test]
fn pending_section_selects_only_unsaved_cores_and_excludes_them_from_groups() {
    let servers: Vec<ServerRowMeta> = vec![
        (
            1,
            21,
            true,
            "default".to_string(),
            Some("Binance Futures".to_string()),
        ),
        (2, 0, true, "default".to_string(), None),
        (3, 0, true, "secondary".to_string(), None),
    ];

    assert_eq!(pending_server_indices(&servers), vec![1, 2]);
    assert_eq!(exchange_sections(&servers, "default")[0].1, vec![0]);
}

/// Regression target: restoring the old retain-and-recreate loop in
/// `connections::sync_groups_from_servers` makes `desk -> "" -> desk` replace every visible local
/// manual-trading setting before the user presses Save.
#[test]
fn retyping_a_group_name_preserves_its_complete_local_state_until_save() {
    let original = GroupConfig {
        name: "desk".to_string(),
        active: false,
        icon: 17,
        trade: GroupTradeSettings {
            order_sizes_usd: [10.0, 20.0, 30.0, 40.0, 50.0, 60.0],
            order_size_sel: 4,
            exit: GroupExitSettings {
                take_profit_pct: 12.0,
                take_profit_mode: TakeProfitMode::Normal,
                fixed_sell_pcts: [1.0, 2.0, 3.0, 4.0, 5.0, 6.0],
                fixed_sell_slot: Some(5),
                stop_loss_pct: -7.5,
                stop_loss_enabled: true,
                use_stop_market: true,
            },
        },
    };
    let mut servers = vec![server("desk")];
    let mut groups = vec![original.clone()];

    servers[0].group.clear();
    sync_groups_from_servers(&servers, &mut groups);
    servers[0].group = "desk".to_string();
    sync_groups_from_servers(&servers, &mut groups);

    assert_eq!(
        groups.iter().find(|group| group.name == "desk"),
        Some(&original),
        "retyping must preserve the complete original GroupConfig"
    );
    assert!(
        groups.iter().any(|group| group.name.is_empty()),
        "the intermediate row stays in preview until AppConfig::save_impl prunes it"
    );
}

/// Regression target: removing the server-name filter from `tab.rs:visible_group_rows` renders
/// every retained intermediate value as another group header while a user types a group name.
#[test]
fn only_current_server_group_names_become_visible_branches() {
    let servers: Vec<ServerRowMeta> = vec![
        (1, 0, true, String::new(), None),
        (2, 21, true, "desk".to_string(), Some("Bybit".to_string())),
        (3, 22, true, "desk".to_string(), None),
        (4, 23, true, "ops".to_string(), None),
    ];
    let mut empty = GroupConfig::new("");
    empty.active = false;
    empty.icon = 7;
    let mut desk = GroupConfig::new("desk");
    desk.active = false;
    desk.icon = 17;
    let mut ops = GroupConfig::new("ops");
    ops.icon = 27;
    let groups = vec![
        GroupConfig::new("d"),
        empty,
        GroupConfig::new("de"),
        desk,
        GroupConfig::new("des"),
        ops,
    ];

    assert_eq!(
        visible_group_rows(&servers, &groups),
        vec![
            (String::new(), false, 7),
            ("desk".to_string(), false, 17),
            ("ops".to_string(), true, 27),
        ],
        "pending, shared, and separate current names must keep their stored metadata"
    );
}
