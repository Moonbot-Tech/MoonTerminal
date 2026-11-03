use moon_core::feed::{
    ServiceAction, ServiceTrack, ServiceVerdict, WatchdogEditError, WatchdogEntryEdit,
    WatchdogSettingsEdit,
};

use super::super::pages::watchdog_ids as ids;
use super::{
    Awaiting, Verdict, WatchdogBlock, invalid_row, stage_input, verdict, verdict_takes_edit,
};

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

/// The banner reports only what THIS window sent, and waits while its request is open.
///
/// Mutation: draw the track's outcome unconditionally. Opening the window would then greet the
/// trader with a verdict on a request from an earlier session, or from another terminal.
#[test]
fn the_banner_reports_only_this_windows_requests() {
    let mut track = ServiceTrack::default();
    track.note_sent(ServiceAction::WatchdogApply, Ok(1));
    track.note_result(&moon_core::feed::CoreServiceResult {
        domain: moon_core::feed::ServiceDomain::Watchdog,
        request_id: Some(1),
        applied: true,
        error: String::new(),
    });
    let mut block = WatchdogBlock::default();
    assert!(verdict(&block, &track).is_none());
    block.sent_here = true;
    block.awaiting = Some(Awaiting {
        action: ServiceAction::WatchdogRemove,
        sent_before: track.sent,
        removed: Some("core-1".to_string()),
    });
    assert!(matches!(verdict(&block, &track), Some(Verdict::Waiting)));
    block.awaiting = None;
    assert!(matches!(
        verdict(&block, &track),
        Some(Verdict::Settled(
            ServiceAction::WatchdogApply,
            ServiceVerdict::Applied
        ))
    ));
}
