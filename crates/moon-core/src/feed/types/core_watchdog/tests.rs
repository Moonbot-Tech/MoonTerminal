use super::*;

fn row(id: &str) -> WatchdogEntryEdit {
    WatchdogEntryEdit {
        id: id.to_string(),
        name: "acc".to_string(),
        price: 1.5,
        time: 30,
        ..WatchdogEntryEdit::default()
    }
}

fn edit(entries: Vec<WatchdogEntryEdit>) -> WatchdogSettingsEdit {
    WatchdogSettingsEdit {
        server_ip: "203.0.113.7".to_string(),
        entries,
        ..WatchdogSettingsEdit::default()
    }
}

#[test]
fn an_existing_row_without_credentials_keeps_the_saved_ones() {
    assert_eq!(edit(vec![row("core-1")]).validate(), Ok(()));
}

#[test]
fn a_new_row_needs_both_credentials() {
    let mut new = row("");
    new.api_key = Some("abcdef".to_string());
    assert_eq!(
        edit(vec![row("core-1"), new.clone()]).validate(),
        Err(WatchdogEditError::NewRowCredentials { index: 1 })
    );
    new.api_secret = Some("ghijkl".to_string());
    assert_eq!(edit(vec![new]).validate(), Ok(()));
}

#[test]
fn a_credential_of_five_characters_is_refused_and_six_pass() {
    let mut r = row("core-1");
    r.api_secret = Some("12345".to_string());
    assert_eq!(
        edit(vec![r.clone()]).validate(),
        Err(WatchdogEditError::ApiSecret { index: 0 })
    );
    r.api_secret = Some("123456".to_string());
    assert_eq!(edit(vec![r.clone()]).validate(), Ok(()));
    // An empty credential is a SUPPLIED one the core rejects, not "keep the saved value".
    r.api_key = Some(String::new());
    assert_eq!(
        edit(vec![r]).validate(),
        Err(WatchdogEditError::ApiKey { index: 0 })
    );
}

#[test]
fn price_and_time_refuse_what_moonproto_refuses() {
    let mut r = row("core-1");
    r.price = f64::NAN;
    assert_eq!(
        edit(vec![r.clone()]).validate(),
        Err(WatchdogEditError::Price { index: 0 })
    );
    r.price = 0.0;
    r.time = -1;
    assert_eq!(
        edit(vec![r]).validate(),
        Err(WatchdogEditError::Time { index: 0 })
    );
}

#[test]
fn the_sender_address_is_empty_or_ipv4() {
    let mut e = edit(Vec::new());
    e.server_ip = String::new();
    assert_eq!(e.validate(), Ok(()));
    e.server_ip = "0.0.0.0".to_string();
    assert_eq!(e.validate(), Ok(()));
    e.server_ip = "vds.example".to_string();
    assert_eq!(e.validate(), Err(WatchdogEditError::ServerIp));
}

#[test]
fn edit_keeps_core_ids_and_carries_no_credentials() {
    let state = CoreWatchdogState {
        entries: vec![CoreWatchdogEntry {
            id: "assigned".to_string(),
            key_set: true,
            secret_set: true,
            price: 2.0,
            time: 15,
            ..CoreWatchdogEntry::default()
        }],
        ..CoreWatchdogState::default()
    };
    let e = state.edit();
    assert_eq!(e.entries[0].id, "assigned");
    assert!(e.entries[0].api_key.is_none() && e.entries[0].api_secret.is_none());
    assert_eq!(e.entries[0].time, 15);
}

#[test]
fn debug_never_prints_a_credential() {
    let mut r = row("");
    r.api_key = Some("PUBLIC-KEY-TEXT".to_string());
    r.api_secret = Some("SECRET-TEXT".to_string());
    let text = format!("{:?}", WatchdogCmd::Apply(edit(vec![r.clone()])));
    assert!(!text.contains("SECRET-TEXT") && !text.contains("PUBLIC-KEY-TEXT"));
    let text = format!("{:?}", edit(vec![r]));
    assert!(!text.contains("SECRET-TEXT") && !text.contains("PUBLIC-KEY-TEXT"));
}

fn result(request_id: u64, applied: bool, error: &str) -> CoreServiceResult {
    CoreServiceResult {
        domain: ServiceDomain::Watchdog,
        request_id: Some(request_id),
        applied,
        error: error.to_string(),
    }
}

#[test]
fn only_the_newest_request_is_settled() {
    let mut track = ServiceTrack::default();
    track.note_sent(ServiceAction::WatchdogApply, Ok(1));
    track.note_sent(ServiceAction::WatchdogRemove, Ok(2));
    // The first request's answer arrives after the second request left: it settles nothing.
    assert!(!track.note_result(&result(1, true, "")));
    assert!(track.pending.is_some());
    assert!(track.note_result(&result(2, true, "")));
    assert_eq!(
        track.outcome,
        Some((ServiceAction::WatchdogRemove, ServiceVerdict::Applied))
    );
}

#[test]
fn applied_with_an_error_reads_as_not_saved() {
    let mut track = ServiceTrack::default();
    track.note_sent(ServiceAction::WatchdogApply, Ok(7));
    track.note_result(&result(7, true, "disk write failed"));
    assert_eq!(
        track.outcome,
        Some((
            ServiceAction::WatchdogApply,
            ServiceVerdict::AppliedNotSaved("disk write failed".to_string())
        ))
    );
}

/// A surface must not read the previous request's verdict as the answer to the one it just queued.
///
/// Mutation: settle on `pending.is_none()` alone. Between the click and the feed registering the
/// request, the track holds the OLD outcome with nothing pending, and the window would drop the
/// user's edit on an answer to something else.
#[test]
fn settled_since_waits_for_the_request_to_be_registered() {
    let mut track = ServiceTrack::default();
    track.note_sent(ServiceAction::WatchdogApply, Ok(1));
    track.note_result(&result(1, true, ""));
    let before = track.sent;
    assert!(!track.settled_since(before), "queued, not registered yet");
    track.note_sent(ServiceAction::WatchdogApply, Ok(2));
    assert!(!track.settled_since(before), "registered, not answered");
    track.note_result(&result(2, false, "invalid key"));
    assert!(track.settled_since(before));
    // A request refused before it left settles as well.
    let before = track.sent;
    track.note_sent(ServiceAction::WatchdogRemove, Err("no link".into()));
    assert!(track.settled_since(before));
}

#[test]
fn a_lost_link_settles_the_pending_request_and_nothing_else() {
    let mut tracks = ServiceTracks::default();
    assert!(!tracks.watchdog.note_lost());
    tracks
        .watchdog
        .note_sent(ServiceAction::WatchdogCancelBuys, Ok(3));
    assert!(tracks.watchdog.note_lost());
    assert!(
        !tracks.telegram.note_lost(),
        "one block's loss settles only its own track"
    );
    assert_eq!(
        tracks.watchdog.outcome,
        Some((ServiceAction::WatchdogCancelBuys, ServiceVerdict::Lost))
    );
    // The answer to a request already settled as lost changes nothing.
    assert!(!tracks.watchdog.note_result(&result(3, true, "")));
}
