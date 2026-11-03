use moon_core::feed::{
    CoreServiceResult, ServiceAction, ServiceDomain, ServiceTrack, ServiceVerdict,
};

use super::{Verdict, verdict, verdict_applied};

/// The banner reports only what THIS window sent, and waits while its request is open.
///
/// Mutation: draw the track's outcome unconditionally. Opening the window would then greet the
/// trader with a verdict on a request from an earlier session, or from another terminal.
#[test]
fn the_banner_reports_only_this_windows_requests() {
    let mut track = ServiceTrack::default();
    track.note_sent(ServiceAction::TelegramChannelPin, Ok(1));
    track.note_result(&CoreServiceResult {
        domain: ServiceDomain::Telegram,
        request_id: Some(1),
        applied: true,
        error: String::new(),
    });
    assert!(verdict(false, false, &track).is_none());
    assert!(matches!(
        verdict(true, true, &track),
        Some(Verdict::Waiting)
    ));
    assert!(matches!(
        verdict(true, false, &track),
        Some(Verdict::Settled(
            ServiceAction::TelegramChannelPin,
            ServiceVerdict::Applied
        ))
    ));
}

/// "Applied but not saved" is still applied: the change is live in the core's memory.
///
/// Mutation: count only `Applied`. A token the core accepted but could not persist would then stay
/// in the box as if it had never been taken.
#[test]
fn applied_but_not_saved_counts_as_applied() {
    assert!(verdict_applied(&ServiceVerdict::AppliedNotSaved(
        "disk".into()
    )));
    assert!(!verdict_applied(&ServiceVerdict::Refused(
        "bad token".into()
    )));
    assert!(!verdict_applied(&ServiceVerdict::Lost));
}
