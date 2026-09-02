//! Deterministic regression coverage for the Connections hierarchy.

use super::entries::{ConnEntry, EntryLabels, flatten_entries};
use super::sync_groups_from_servers;
use super::tab::{
    ServerRowMeta, apply_group_transport, pending_server_indices, visible_group_rows,
};
use crate::core_order::CoreOrder;
use moon_core::config::{
    AppConfig, FeedFlags, GroupConfig, GroupExitSettings, GroupTradeSettings, Secret, ServerConfig,
    TakeProfitMode, TransportVersion,
};
use moon_core::venue::CoreVenue;

/// Build one identified core's venue from its platform ordinal.
fn venue(code: u8) -> CoreVenue {
    CoreVenue::identify(code, "", None)
}

/// `connections/tab.rs:apply_group_transport` must update every selected group member only.
///
/// Breakage: narrowing the group filter or removing it leaves a core unchanged or changes another group's core, so one bulk selection no longer produces the requested reconnections on Save.
#[test]
fn group_transport_updates_every_member_and_leaves_other_groups_untouched() {
    let mut servers = vec![server("desk"), server("desk"), server("ops")];
    servers[0].transport = Some(TransportVersion::V0);
    servers[1].transport = Some(TransportVersion::V1);
    servers[2].transport = Some(TransportVersion::V0);
    assert!(apply_group_transport(
        &mut servers,
        "desk",
        TransportVersion::V2
    ));
    assert_eq!(servers[0].transport, Some(TransportVersion::V2));
    assert_eq!(servers[1].transport, Some(TransportVersion::V2));
    assert_eq!(servers[2].transport, Some(TransportVersion::V0));
}

/// `connections/tab.rs:apply_group_transport` must report changes only when a row changes mode.
///
/// Breakage: setting `changed` for an already-matching, empty, or absent group would trigger a needless config notification and reconnect cycle without any user-visible configuration change.
#[test]
fn group_transport_skips_already_matching_empty_and_missing_groups() {
    let mut servers = vec![server("desk"), server("ops")];
    servers[0].transport = Some(TransportVersion::V1);
    servers[1].transport = Some(TransportVersion::V0);
    let before = servers
        .iter()
        .map(|server| server.transport)
        .collect::<Vec<_>>();
    assert!(!apply_group_transport(
        &mut servers,
        "desk",
        TransportVersion::V1
    ));
    assert!(!apply_group_transport(
        &mut servers,
        "",
        TransportVersion::V2
    ));
    assert!(!apply_group_transport(
        &mut servers,
        "missing",
        TransportVersion::V2
    ));
    assert_eq!(
        servers
            .iter()
            .map(|server| server.transport)
            .collect::<Vec<_>>(),
        before
    );
}

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
        own_trade_config: false,
        strat_slots: None,
        manual_strategy: None,
        trade: None,
        transport: None,
        workspace_membership: moon_core::config::WorkspaceMembership::default(),
    }
}

/// `entries.rs:core_row_entry` must keep each sorted row's source `draft_index`; replacing it
/// with a ranked display position writes a name or key edit into a different core's saved config.
/// The same fixture keeps the unknown bucket first, so unidentified cores remain visible above
/// populated exchange sections.
#[test]
fn flatten_entries_group_known_names_and_keep_unknown_first() {
    let servers: Vec<ServerRowMeta> = vec![
        (1, 11, true, "default".to_string(), Some(venue(7))),
        (2, 12, true, "default".to_string(), None),
        (3, 13, true, "default".to_string(), Some(venue(4))),
        (4, 14, false, "default".to_string(), Some(venue(7))),
        (5, 15, true, "secondary".to_string(), None),
    ];

    let config = AppConfig::load(None, false).expect("test-binary config must load");
    let entries = flatten_entries(
        &servers,
        &[("default".to_string(), true, 0)],
        &CoreOrder::new(&config),
        EntryLabels {
            pending: "Pending",
            exchange: &|venue| {
                venue
                    .map(crate::controls::venue_label)
                    .unwrap_or_else(|| "Unknown".to_string())
            },
        },
    );
    let mut sections = Vec::<(String, Vec<usize>)>::new();
    for entry in entries {
        match entry {
            ConnEntry::ExchangeHeader { caption, .. } => sections.push((caption, Vec::new())),
            ConnEntry::CoreRow { draft_index, .. } => {
                if let Some((_, members)) = sections.last_mut() {
                    members.push(draft_index);
                }
            }
            ConnEntry::PendingHeader { .. } | ConnEntry::GroupHeader { .. } => {}
        }
    }

    assert_eq!(
        sections,
        vec![
            ("Unknown".to_string(), vec![1]),
            ("Binance Futures".to_string(), vec![2]),
            ("Bybit Spot".to_string(), vec![0, 3]),
        ]
    );
}

/// `entries.rs:flatten_entries` must keep `uid == 0` rows in its top section; reversing the
/// predicate replaces it with saved cores while new cores remain excluded from groups, hiding their
/// fields.
#[test]
fn pending_section_selects_only_unsaved_cores_and_excludes_them_from_groups() {
    let servers: Vec<ServerRowMeta> = vec![
        (1, 21, true, "default".to_string(), Some(venue(4))),
        (2, 0, true, "default".to_string(), None),
        (3, 0, true, "secondary".to_string(), None),
    ];

    assert_eq!(pending_server_indices(&servers), vec![1, 2]);
    let config = AppConfig::load(None, false).expect("test-binary config must load");
    let entries = flatten_entries(
        &servers,
        &[("default".to_string(), true, 0)],
        &CoreOrder::new(&config),
        EntryLabels {
            pending: "Pending",
            exchange: &|venue| {
                venue
                    .map(crate::controls::venue_label)
                    .unwrap_or_else(|| "Unknown".to_string())
            },
        },
    );
    assert_eq!(
        entries,
        vec![
            ConnEntry::PendingHeader {
                caption: "Pending".to_string(),
                member_count: 2,
            },
            ConnEntry::CoreRow {
                draft_index: 1,
                core_id: 2,
                uid: 0,
                active: true,
                indented: true,
            },
            ConnEntry::CoreRow {
                draft_index: 2,
                core_id: 3,
                uid: 0,
                active: true,
                indented: true,
            },
            ConnEntry::GroupHeader {
                name: "default".to_string(),
                active: true,
                icon: 0,
                member_count: 2,
            },
            ConnEntry::ExchangeHeader {
                group_index: 0,
                exchange_index: 0,
                caption: "Binance Futures".to_string(),
                member_count: 1,
                identified: true,
            },
            ConnEntry::CoreRow {
                draft_index: 0,
                core_id: 1,
                uid: 21,
                active: true,
                indented: true,
            },
        ]
    );
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
        (2, 21, true, "desk".to_string(), Some(venue(7))),
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
