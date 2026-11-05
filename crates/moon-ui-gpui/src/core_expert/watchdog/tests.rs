use moon_core::feed::{
    ServiceAction, ServiceVerdict, WatchdogEditError, WatchdogEntryEdit, WatchdogSettingsEdit,
};

use super::super::pages::watchdog_ids as ids;
use super::{WatchdogBlock, invalid_row, stage_input, verdict_takes_edit};

/// Minimal observer snapshots keep account identity separate from row position.
fn snapshot(ids: &[&str]) -> std::sync::Arc<moon_core::feed::CoreWatchdogState> {
    std::sync::Arc::new(moon_core::feed::CoreWatchdogState {
        entries: ids
            .iter()
            .map(|id| moon_core::feed::CoreWatchdogEntry {
                id: (*id).to_string(),
                name: (*id).to_string(),
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    })
}

/// Keeping the old index after A disappears would highlight, edit and remove C instead of B.
#[test]
fn a_snapshot_push_keeps_the_picked_account() {
    let mut block = WatchdogBlock::default();
    block.take_snapshot(Some(snapshot(&["ACC_A", "ACC_B", "ACC_C"])));
    block.selected = Some(1);
    let generation = block.generation;
    let pushed = snapshot(&["ACC_B", "ACC_C"]);
    block.take_snapshot(Some(pushed.clone()));

    assert_eq!(block.selected, Some(0));
    let edit = block.edit_view(Some(&pushed));
    assert_eq!(
        super::saved_row_id(edit.as_ref(), block.selected).as_deref(),
        Some("ACC_B")
    );
    assert_eq!(
        super::input_value(ids::NAME, edit.as_ref(), block.selected),
        "ACC_B"
    );
    assert_ne!(
        block.generation, generation,
        "editors must read the pushed values"
    );
}

/// Keeping B's index after B disappears would let minus remove C without the trader picking it.
#[test]
fn a_snapshot_push_clears_a_vanished_pick() {
    let mut block = WatchdogBlock::default();
    block.take_snapshot(Some(snapshot(&["ACC_A", "ACC_B", "ACC_C"])));
    block.selected = Some(1);
    let pushed = snapshot(&["ACC_A", "ACC_C"]);
    block.take_snapshot(Some(pushed.clone()));

    assert_eq!(block.selected, None);
    let edit = block.edit_view(Some(&pushed));
    assert_eq!(super::saved_row_id(edit.as_ref(), block.selected), None);
    assert_eq!(
        super::input_value(ids::NAME, edit.as_ref(), block.selected),
        ""
    );
}

/// Remapping a draft's index against pushed rows would redirect edits away from its picked B.
#[test]
fn a_snapshot_push_preserves_the_drafts_row_order() {
    let initial = snapshot(&["ACC_A", "ACC_B", "ACC_C"]);
    let mut block = WatchdogBlock::default();
    block.take_snapshot(Some(initial.clone()));
    block.selected = Some(1);
    let mut draft = initial.edit();
    draft.entries[1].name = "ACC_B_EDITED".to_string();
    block.draft = Some(draft.clone());
    let generation = block.generation;

    block.take_snapshot(Some(snapshot(&["ACC_B", "ACC_C"])));

    assert_eq!(block.selected, Some(1));
    assert_eq!(block.draft, Some(draft));
    assert_eq!(block.generation, generation);
}

fn edit_with_row() -> WatchdogSettingsEdit {
    WatchdogSettingsEdit {
        entries: vec![WatchdogEntryEdit {
            id: "core-1".to_string(),
            price: 1.0,
            time: 10,
            ..WatchdogEntryEdit::default()
        }],
        ..WatchdogSettingsEdit::default()
    }
}

/// An emptied key box keeps the saved key; it must never reach the core as an empty credential.
///
/// Mutation: stage `Some(String::new())`. The core then rejects the whole Apply for a box the
/// trader merely cleared.
#[test]
fn an_emptied_credential_box_keeps_the_saved_one() {
    let mut edit = edit_with_row();
    stage_input(&mut edit, Some(0), ids::KEY, "abcdef");
    assert_eq!(edit.entries[0].api_key.as_deref(), Some("abcdef"));
    stage_input(&mut edit, Some(0), ids::KEY, "   ");
    assert_eq!(edit.entries[0].api_key, None);
}

/// A box that does not parse must make Apply refuse, not send the last good number.
///
/// Mutation: ignore an unparsable price or age. The core would then receive the previous value the
/// trader believes they replaced.
#[test]
fn an_unparsable_threshold_is_refused_by_validation() {
    let mut edit = edit_with_row();
    stage_input(&mut edit, Some(0), ids::PRICE, "1,5x");
    assert_eq!(edit.validate(), Err(WatchdogEditError::Price { index: 0 }));
    stage_input(&mut edit, Some(0), ids::PRICE, "0.5");
    stage_input(&mut edit, Some(0), ids::TIME, "2.5");
    assert_eq!(edit.validate(), Err(WatchdogEditError::Time { index: 0 }));
    stage_input(&mut edit, Some(0), ids::TIME, "30");
    assert_eq!(edit.entries[0].time, 30);
    assert_eq!(edit.validate(), Ok(()));
}

/// The settings above the table are one line each; a pasted line break must not reach the core.
#[test]
fn the_exclusion_list_stays_on_one_line() {
    let mut edit = edit_with_row();
    stage_input(&mut edit, None, ids::BLACKLIST, "BTC,\r\nETH");
    assert_eq!(edit.blacklist, "BTC,ETH");
    // A row field with no row picked changes nothing.
    stage_input(&mut edit, None, ids::NAME, "acc");
    assert_eq!(edit.entries[0].name, "");
}

/// Only an applied edit is rebuilt from the core's state; a refusal keeps the trader's edit.
///
/// Mutation: drop the edit on every verdict. A rejected key would then wipe everything the trader
/// typed, including the other rows.
#[test]
fn only_an_applied_edit_is_rebuilt_from_the_core() {
    assert!(verdict_takes_edit(
        ServiceAction::WatchdogApply,
        &ServiceVerdict::AppliedNotSaved("disk".into())
    ));
    assert!(!verdict_takes_edit(
        ServiceAction::WatchdogApply,
        &ServiceVerdict::Refused("bad key".into())
    ));
    assert!(!verdict_takes_edit(
        ServiceAction::WatchdogRemove,
        &ServiceVerdict::Applied
    ));
}

/// A confirmed Remove takes the row out of the edit; an edit left with no change is dropped.
///
/// Mutation: keep the removed row in the edit. The next Apply would then upsert it back under an id
/// the core just deleted.
#[test]
fn a_confirmed_remove_leaves_the_edit() {
    let snapshot = moon_core::feed::CoreWatchdogState {
        entries: vec![moon_core::feed::CoreWatchdogEntry {
            id: "core-2".to_string(),
            ..Default::default()
        }],
        ..Default::default()
    };
    let mut block = WatchdogBlock::default();
    let mut draft = snapshot.edit();
    draft.entries.insert(
        0,
        WatchdogEntryEdit {
            id: "core-1".to_string(),
            ..WatchdogEntryEdit::default()
        },
    );
    block.draft = Some(draft);
    block.forget_removed("core-1", Some(&snapshot));
    assert!(block.draft.is_none(), "nothing left to apply");

    let mut draft = snapshot.edit();
    draft.server_ip = "203.0.113.5".to_string();
    draft.entries.push(WatchdogEntryEdit {
        id: "core-1".to_string(),
        ..WatchdogEntryEdit::default()
    });
    block.draft = Some(draft);
    block.forget_removed("core-1", Some(&snapshot));
    let kept = block.draft.as_ref().expect("the address edit stays");
    assert_eq!(kept.entries.len(), 1);
    assert_eq!(kept.entries[0].id, "core-2");
}

#[test]
fn a_local_refusal_names_its_row() {
    assert_eq!(invalid_row(&WatchdogEditError::ServerIp), None);
    assert_eq!(
        invalid_row(&WatchdogEditError::NewRowCredentials { index: 2 }),
        Some(2)
    );
}
