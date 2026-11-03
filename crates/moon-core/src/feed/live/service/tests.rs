use super::*;
use crate::feed::{ServiceAction, TelegramCmd, WatchdogCmd};

/// The request id arrives as a decimal string; the full `u64` range must survive the trip.
///
/// Mutation: parse as `i64`. A request id above `i64::MAX` — half of all random ids — would then
/// never settle its banner.
#[test]
fn request_ids_parse_across_the_whole_u64_range() {
    let raw = serde_json::json!({
        "domain": "telegram",
        "action": "pin",
        "request_id": u64::MAX.to_string(),
        "applied": true,
        "error": "",
        "state": { "settings_supported": true }
    });
    let result: ServiceSettingsResult = serde_json::from_value(raw).expect("valid result");
    let projected = result_from_proto(&result);
    assert_eq!(projected.request_id, Some(u64::MAX));
    assert_eq!(projected.domain, ServiceDomain::Telegram);
    assert!(projected.applied);
}

/// A dropped outgoing-bot action is reported like an observer one, so its banner stops waiting;
/// a reader or login intent nobody waits on reports nothing.
///
/// Mutation: report only `CoreCmd::Watchdog`. A channel reset dropped across an outage would then
/// leave the Remote block waiting for an answer that cannot come.
#[test]
fn dropped_reports_every_tracked_service_action_and_nothing_else() {
    let tracked = dropped(&CoreCmd::Telegram(TelegramCmd::ResetChannel), "offline");
    assert!(matches!(
        tracked,
        Some(FeedMsg::ServiceRequest {
            action: ServiceAction::TelegramResetChannel,
            sent: Err(_),
        })
    ));
    assert!(dropped(&CoreCmd::Telegram(TelegramCmd::Logout), "offline").is_none());
    assert!(dropped(&CoreCmd::Watchdog(WatchdogCmd::Refresh), "offline").is_none());
    assert!(dropped(&CoreCmd::Watchdog(WatchdogCmd::CancelBuys), "offline").is_some());
}
