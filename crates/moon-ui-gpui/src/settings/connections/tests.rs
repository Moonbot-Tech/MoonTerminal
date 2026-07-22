//! Deterministic regression coverage for the Connections hierarchy.

use super::tab::{ServerRowMeta, exchange_sections, pending_server_indices};

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
