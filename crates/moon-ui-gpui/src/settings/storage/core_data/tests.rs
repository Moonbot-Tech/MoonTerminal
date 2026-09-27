//! "Data by core" merge and layout tests.
//!
//! Explicit imports (no `use super::*`, which would bring in gpui's `test` macro).

use std::collections::HashMap;

use super::super::super::connections::ConnEntry;
use super::{CoreCounts, CoreLine, Group, Line, lines, merge};

fn line(uid: u64, name: &str, group: Group) -> CoreLine {
    CoreLine {
        counts: CoreCounts {
            uid,
            ..CoreCounts::default()
        },
        name: name.to_string(),
        group,
        syncing: None,
    }
}

fn core_entry(uid: u64) -> ConnEntry {
    ConnEntry::CoreRow {
        draft_index: 0,
        core_id: uid,
        uid,
        active: true,
        indented: true,
    }
}

/// A core present in only one store still gets a row, and the report name wins over the
/// strategies one.
///
/// Breaks on: `core_data.rs:merge` building rows from the report replica alone. A core deleted
/// from Connections whose reports are gone but whose strategy history stayed is exactly the ghost
/// a user comes to remove, and it would be invisible.
#[test]
fn every_store_contributes_its_cores() {
    let mut strategies = HashMap::new();
    strategies.insert(
        1,
        moon_core::strat_db::CoreStratCounts {
            name: "Old".into(),
            heads: 3,
            versions: 40,
        },
    );
    strategies.insert(
        2,
        moon_core::strat_db::CoreStratCounts {
            name: "OnlyStrat".into(),
            heads: 1,
            versions: 2,
        },
    );
    let traces = HashMap::from([(3, 7)]);
    let warnings = HashMap::from([(1, 5)]);

    let rows = merge(vec![(1, "New".into(), 100)], strategies, traces, warnings);

    assert_eq!(rows.len(), 3);
    assert_eq!(
        rows[0],
        CoreCounts {
            uid: 1,
            stored_name: "New".into(),
            reports: 100,
            strategies: 3,
            versions: 40,
            traces: 0,
            warnings: 5,
        }
    );
    assert_eq!(rows[1].stored_name, "OnlyStrat");
    assert_eq!(rows[2].traces, 7);
    assert!(rows[2].stored_name.is_empty());
}

/// Configured cores follow the Connections tree and its order; every other core follows under one
/// section, by name; each group knows its own cores for its checkbox.
///
/// Breaks on: `core_data.rs:lines` sorting cores by itself instead of taking the Connections
/// order, dropping the cores that are not in Connections — the ghosts this list exists for — or
/// crediting a core to the wrong group, which makes a group's checkbox select another group's
/// cores.
#[test]
fn cores_follow_the_connections_tree_then_the_rest() {
    let entries = vec![
        ConnEntry::GroupHeader {
            name: "default".into(),
            active: true,
            icon: 0,
            member_count: 2,
        },
        ConnEntry::ExchangeHeader {
            group_index: 0,
            exchange_index: 0,
            caption: "Binance Futures".into(),
            member_count: 2,
            identified: true,
        },
        core_entry(4),
        core_entry(3),
        ConnEntry::GroupHeader {
            name: "second".into(),
            active: true,
            icon: 1,
            member_count: 1,
        },
        ConnEntry::ExchangeHeader {
            group_index: 1,
            exchange_index: 0,
            caption: "Gate".into(),
            member_count: 1,
            identified: true,
        },
        core_entry(9),
    ];
    let cores: HashMap<u64, CoreLine> = [
        line(3, "b", Group::Connected),
        line(4, "a", Group::Offline),
        line(9, "z", Group::Connected),
        line(23, "AAA", Group::Archive),
        line(7, "aaa", Group::Archive),
    ]
    .into_iter()
    .map(|c| (c.counts.uid, c))
    .collect();

    let out = lines(&entries, cores, "archive");
    let shape: Vec<String> = out
        .iter()
        .map(|l| match l {
            Line::Group { name, uids, .. } => format!("{name}:{uids:?}"),
            Line::Section { caption, count, .. } => format!("[{caption} {count}]"),
            Line::Core(c) => c.counts.uid.to_string(),
        })
        .collect();
    assert_eq!(
        shape,
        [
            "default:[4, 3]",
            "[Binance Futures 2]",
            "4",
            "3",
            "second:[9]",
            "[Gate 1]",
            "9",
            "[archive 2]",
            "7",
            "23",
        ]
    );
}

/// A core whose catch-up is running cannot be chosen.
///
/// Breaks on: `CoreLine::selectable` ignoring `syncing`. A page requested before the wipe lands
/// after it and records a frontier from the dead download, so the next interruption resumes
/// above a hole — the defect this section exists to repair.
#[test]
fn a_downloading_core_is_not_selectable() {
    let mut core = line(1, "a", Group::Connected);
    assert!(core.selectable());
    core.syncing = Some("43%".into());
    assert!(!core.selectable());
}
