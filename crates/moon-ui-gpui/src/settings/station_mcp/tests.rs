//! AI-access page: choosing per-core permissions and reading a grant's reach.
//!
//! Explicit imports (no `use super::*`, which would bring in gpui's `test` macro).

use std::collections::{BTreeMap, BTreeSet};

use gpui::{IntoElement, ParentElement, Styled};
use moon_core::station_api::mcp::{CoreAccess, CoreInfo, GrantInfo, Scope};
use rust_i18n::t;

use super::{
    AccessEdit, MCP_CONSENT_W, MCP_CORE_COL_W, MCP_HISTORY_W, access_of, access_text, column_state,
    drop_write_scopes, edit_applied, edit_block, edit_in_sync, grant_lines, listed_access,
    mcp_column, mcp_columns, mcp_core_row, scope_column, scope_short, select_all, select_all_state,
    set_column, set_scope,
};
use crate::settings::common::{CHECKBOX_LEAD, floored_checkbox, prose};
use crate::settings::general::probe_host::{host, stub};
use crate::settings::layout_probe::{ProbeSpec, sweep};

/// Loading a client's rights and sending them back untouched must read as no change: otherwise
/// the form offers to "apply" what is already there, and any difference here is a silent edit.
#[test]
fn a_clients_permissions_load_and_compare_as_unchanged() {
    // Loads the dictionary on a stack that fits it: a first `t!()` on a test thread overflows.
    let _locale = crate::test_locale::force("en");
    let held = grant(
        "Claude",
        &[(1, &[Scope::Read]), (2, &[Scope::Read, Scope::Trading])],
    );
    // The Station's own order, not the stored one: a comparison by position must not see a change.
    let listed = cores(&[2, 1]);
    let loaded = access_of(&held);
    assert_eq!(listed_access(&listed, &loaded), held.access);
    assert!(edit_block(false, true, &listed_access(&listed, &loaded), &held).is_some());
}

/// A tick on the table is what the Apply button exists for.
#[test]
fn a_changed_tick_can_be_applied() {
    // Loads the dictionary on a stack that fits it: a first `t!()` on a test thread overflows.
    let _locale = crate::test_locale::force("en");
    let held = grant("Claude", &[(1, &[Scope::Read])]);
    let mut access = access_of(&held);
    set_scope(&mut access, 1, Scope::Trading, true);
    assert!(edit_block(false, true, &listed_access(&cores(&[1]), &access), &held).is_none());
}

/// Terminal A opens the form, Terminal B takes a permission away, and A refreshes, so A's latest
/// status is current. Its old ticks would then pass the Station's check and hand the permission
/// back. Each case here is a real moment in that form's life.
#[test]
fn a_form_goes_stale_only_when_the_permissions_moved_elsewhere() {
    let start = grant("Claude", &[(1, &[Scope::Read, Scope::Trading])]).access;
    let elsewhere = grant("Claude", &[(1, &[Scope::Read])]).access;
    let applied = grant("Claude", &[(1, &[Scope::Read, Scope::Settings])]).access;
    let mut edit = AccessEdit {
        grant_id: "g".into(),
        access: BTreeMap::new(),
        base: start.clone(),
        sent: None,
    };
    assert!(edit_in_sync(&start, &edit), "just opened");
    assert!(!edit_in_sync(&elsewhere, &edit), "changed by someone else");
    edit.sent = Some(applied.clone());
    assert!(edit_in_sync(&start, &edit), "refused: can be applied again");
    // Our own accepted change is not "in sync": the form must be gone by then, not reusable.
    assert!(
        !edit_in_sync(&applied, &edit),
        "accepted: closed, see below"
    );
}

/// The form closes on the status that reports its change, not on the click: a refused change must
/// keep its ticks, and only the client it edits can close it.
#[test]
fn a_form_is_done_only_when_the_station_reports_what_it_sent() {
    let start = grant("Claude", &[(1, &[Scope::Read])]);
    let accepted = grant("Claude", &[(1, &[Scope::Read, Scope::Trading])]);
    let other = grant("ChatGPT", &[(1, &[Scope::Read, Scope::Trading])]);
    let mut edit = AccessEdit {
        grant_id: "Claude".into(),
        access: BTreeMap::new(),
        base: start.access.clone(),
        sent: None,
    };
    assert!(
        !edit_applied(&edit, std::slice::from_ref(&start)),
        "not sent"
    );
    edit.sent = Some(accepted.access.clone());
    assert!(
        !edit_applied(&edit, std::slice::from_ref(&start)),
        "refused"
    );
    assert!(
        !edit_applied(&edit, std::slice::from_ref(&other)),
        "another client"
    );
    assert!(
        edit_applied(&edit, std::slice::from_ref(&accepted)),
        "accepted"
    );
}

/// A heading tick is the only write that spans cores: it must reach every listed core and give
/// each the read it requires, or the Station refuses the whole consent.
#[test]
fn a_column_tick_grants_on_every_core_and_selects_it() {
    let listed = cores(&[1, 2]);
    let mut access = BTreeMap::new();
    assert_eq!(
        column_state(&listed, &access, Scope::Trading),
        (false, false)
    );
    set_scope(&mut access, 1, Scope::Trading, true);
    assert_eq!(
        column_state(&listed, &access, Scope::Trading),
        (false, true)
    );
    set_column(&mut access, &listed, Scope::Trading, true);
    assert_eq!(
        column_state(&listed, &access, Scope::Trading),
        (true, false)
    );
    assert_eq!(access[&2], BTreeSet::from([Scope::Read, Scope::Trading]));
}

/// Unticking takes back the permission, and only it: the cores stay readable, and a neighbouring
/// column keeps what was given there.
#[test]
fn a_column_untick_keeps_the_cores_and_the_other_columns() {
    let listed = cores(&[1]);
    let mut access = BTreeMap::new();
    set_scope(&mut access, 1, Scope::Trading, true);
    set_scope(&mut access, 1, Scope::Settings, true);
    set_column(&mut access, &listed, Scope::Trading, false);
    assert_eq!(access[&1], BTreeSet::from([Scope::Read, Scope::Settings]));
}

/// A core removed from the Station must not make the column read as complete.
#[test]
fn a_removed_core_does_not_complete_a_column() {
    let mut access = BTreeMap::new();
    set_scope(&mut access, 99, Scope::Trading, true);
    set_scope(&mut access, 1, Scope::Trading, true);
    assert_eq!(
        column_state(&cores(&[1, 2]), &access, Scope::Trading),
        (false, true)
    );
}

/// The widest access cell a real page draws: two clients, one holding everything.
fn widest_access() -> String {
    let every: &[Scope] = &[
        Scope::Read,
        Scope::Strategies,
        Scope::Trading,
        Scope::Settings,
    ];
    let grants = [
        grant("Claude", &[(1, every)]),
        grant("ChatGPT", &[(1, &[Scope::Read])]),
    ];
    access_text(&grants, 1, scope_short).expect("access")
}

/// The table's fixed columns can push a body past its edge, and its two page columns can overlap
/// or cut text, none of which a build or the logic tests see. Checks every language at every
/// window size, the narrow ones included.
#[gpui::test]
fn the_core_table_and_its_columns_fit_the_body(cx: &mut gpui::TestAppContext) {
    sweep(
        cx,
        |window, view, size| {
            host(move |_, cx| {
                // The heading's labelled checkboxes are the widest thing in a permission column.
                let head = mcp_core_row(
                    floored_checkbox(
                        "probe-all",
                        &t!("station_mcp.select_all"),
                        prose(
                            "probe-all-label".to_string(),
                            t!("station_mcp.select_all").to_string(),
                            false,
                            cx,
                        )
                        .pl(gpui::px(CHECKBOX_LEAD)),
                        cx,
                    ),
                    [Scope::Strategies, Scope::Trading, Scope::Settings].map(|scope| {
                        floored_checkbox(
                            &format!("probe-head-{}", scope.as_str()),
                            &scope_column(scope),
                            prose(
                                format!("probe-label-{}", scope.as_str()),
                                scope_column(scope),
                                false,
                                cx,
                            )
                            .pl(gpui::px(CHECKBOX_LEAD)),
                            cx,
                        )
                    }),
                    prose(
                        "probe-access-head".to_string(),
                        t!("station_mcp.col_access").to_string(),
                        true,
                        cx,
                    )
                    .flex_1()
                    .into_any_element(),
                    cx,
                );
                let row = mcp_core_row(
                    stub("probe-core", MCP_CORE_COL_W, 24.0),
                    [
                        stub("probe-strategies", 16.0, 16.0),
                        stub("probe-trading", 16.0, 16.0),
                        stub("probe-settings", 16.0, 16.0),
                    ],
                    prose("probe-access".to_string(), widest_access(), false, cx)
                        .flex_1()
                        .into_any_element(),
                    cx,
                );
                mcp_columns(
                    Some(mcp_column(MCP_CONSENT_W, cx).child(head).child(row)),
                    mcp_column(MCP_HISTORY_W, cx).child(stub("probe-history", 200.0, 40.0)),
                    cx,
                )
                .into_any_element()
            })(window, view, size)
        },
        |_| ProbeSpec {
            container: "probe-body",
            inside: [
                "probe-all-box",
                "probe-head-strategies-box",
                "probe-head-trading-box",
                "probe-head-settings-box",
                "probe-access-head",
                "probe-core",
                "probe-strategies",
                "probe-trading",
                "probe-settings",
                "probe-access",
                "probe-history",
            ]
            .map(String::from)
            .to_vec(),
            no_overlap: vec![
                // A label wider than its column would run into the next heading.
                [
                    "probe-all-box",
                    "probe-head-strategies-box",
                    "probe-head-trading-box",
                    "probe-head-settings-box",
                    "probe-access-head",
                ]
                .map(String::from)
                .to_vec(),
                [
                    "probe-core",
                    "probe-strategies",
                    "probe-access",
                    "probe-history",
                ]
                .map(String::from)
                .to_vec(),
            ],
            wraps: vec![("probe-access".to_string(), widest_access())],
            ..Default::default()
        },
    );
}

fn cores(uids: &[u64]) -> Vec<CoreInfo> {
    uids.iter()
        .map(|&uid| CoreInfo {
            uid,
            name: format!("core{uid}"),
        })
        .collect()
}

fn grant(client: &str, access: &[(u64, &[Scope])]) -> GrantInfo {
    GrantInfo {
        id: client.to_string(),
        client_name: client.to_string(),
        access: access
            .iter()
            .map(|(core, scopes)| CoreAccess {
                core: *core,
                scopes: scopes.to_vec(),
            })
            .collect(),
        created_ms: 0,
        last_access_ms: None,
        requested: None,
    }
}

fn selected(uids: &[u64]) -> BTreeMap<u64, BTreeSet<Scope>> {
    uids.iter()
        .map(|&uid| (uid, BTreeSet::from([Scope::Read])))
        .collect()
}

fn name(scope: Scope) -> String {
    scope.as_str().to_string()
}

/// A core removed from the Station stays in the selection until the next toggle. Comparing sizes
/// would call this partial choice complete, and the next click would clear it.
#[test]
fn a_removed_core_in_the_selection_does_not_complete_it() {
    assert_eq!(
        select_all_state(&cores(&[1, 2]), &selected(&[1, 99])),
        (false, true)
    );
}

/// The Station rejects a consent naming an unknown core, and that core has no row to unselect it:
/// sending the raw selection would fail the approval with nothing on screen to fix.
#[test]
fn a_removed_core_never_reaches_the_approval() {
    let sent = listed_access(&cores(&[1, 2]), &selected(&[2, 99]));
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].core, 2);
    assert!(listed_access(&cores(&[1]), &selected(&[99])).is_empty());
}

#[test]
fn select_all_tells_none_some_and_all_apart() {
    let listed = cores(&[1, 2]);
    assert_eq!(select_all_state(&listed, &selected(&[])), (false, false));
    assert_eq!(select_all_state(&listed, &selected(&[2])), (false, true));
    assert_eq!(select_all_state(&listed, &selected(&[1, 2])), (true, false));
}

/// With no core listed, nothing is selected even when a stale id remains.
#[test]
fn an_empty_core_list_is_never_all_selected() {
    assert_eq!(select_all_state(&[], &selected(&[1])), (false, false));
}

/// A write permission without read would be refused by the Station, and it is what a click on a
/// lone trading box means: this core, for trading.
#[test]
fn a_write_permission_selects_its_core_for_reading_too() {
    let mut access = BTreeMap::new();
    set_scope(&mut access, 1, Scope::Trading, true);
    assert_eq!(access[&1], BTreeSet::from([Scope::Read, Scope::Trading]));
}

/// Unselecting a core must not leave its trading permission behind, unseen until the next tick.
#[test]
fn unselecting_a_core_drops_its_write_permissions() {
    let mut access = BTreeMap::new();
    set_scope(&mut access, 1, Scope::Trading, true);
    set_scope(&mut access, 1, Scope::Read, false);
    assert!(access.is_empty());
}

#[test]
fn unticking_a_write_permission_keeps_the_core_selected() {
    let mut access = BTreeMap::new();
    set_scope(&mut access, 1, Scope::Trading, true);
    set_scope(&mut access, 1, Scope::Trading, false);
    assert_eq!(access[&1], BTreeSet::from([Scope::Read]));
}

/// One click must not grant trading everywhere, and must not undo trading already given to one core.
#[test]
fn select_all_grants_reading_and_keeps_what_was_ticked() {
    let mut access = BTreeMap::new();
    set_scope(&mut access, 2, Scope::Trading, true);
    select_all(&mut access, &cores(&[1, 2]));
    assert_eq!(access[&1], BTreeSet::from([Scope::Read]));
    assert_eq!(access[&2], BTreeSet::from([Scope::Read, Scope::Trading]));
}

/// The form outlives the consent it sent. A tick left on would hand trading to the next client.
#[test]
fn write_permissions_do_not_carry_over_to_the_next_consent() {
    let mut access = BTreeMap::new();
    set_scope(&mut access, 1, Scope::Trading, true);
    drop_write_scopes(&mut access);
    assert_eq!(access[&1], BTreeSet::from([Scope::Read]));
}

/// The cell states a client's reach over a trading account: per core, and only where it holds.
#[test]
fn the_access_cell_states_each_cores_own_permissions() {
    let grants = [
        grant(
            "Claude",
            &[(1, &[Scope::Read]), (2, &[Scope::Read, Scope::Trading])],
        ),
        grant("ChatGPT", &[(2, &[Scope::Read])]),
    ];
    assert_eq!(
        access_text(&grants, 1, name).as_deref(),
        Some("Claude: read")
    );
    assert_eq!(
        access_text(&grants, 2, name).as_deref(),
        Some("Claude: read, trading · ChatGPT: read")
    );
    assert_eq!(access_text(&grants, 3, name), None);
}

/// A grant over 28 cores must read as who holds what, with a removed core still named by number.
#[test]
fn a_grant_reads_as_one_line_per_set_of_permissions() {
    let held = grant(
        "Claude",
        &[
            (1, &[Scope::Read]),
            (2, &[Scope::Read, Scope::Trading]),
            (9, &[Scope::Read]),
        ],
    );
    assert_eq!(
        grant_lines(&held, &cores(&[1, 2]), name),
        vec![
            "read: #1 core1, #9".to_string(),
            "read, trading: #2 core2".to_string(),
        ]
    );
}
