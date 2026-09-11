//! Role migration and explicit grants must never turn a viewer into an implicit owner.
use super::{TelegramConfig, TelegramReportAccess};

/// Revocation and owner transfer retire pending deliveries; captions and set ordering do not.
#[test]
fn delivery_generation_changes_for_permissions_but_not_captions() {
    let mut before = TelegramConfig {
        authorized_chat_ids: vec![11, 22],
        ..Default::default()
    };
    before.chat_profile_mut(22).core_uids = vec![3, 7];
    let mut after = before.clone();
    after.chat_profile_mut(22).name = "Client".into();
    after.chat_profile_mut(22).core_uids = vec![7, 3, 3];
    assert!(before.same_chat_permissions(&after));
    after.chat_profile_mut(22).core_uids = vec![3];
    assert!(!before.same_chat_permissions(&after));
    after = before.clone();
    after.set_owner(22);
    assert!(!before.same_chat_permissions(&after));
    after = before.clone();
    after.authorized_chat_ids.retain(|id| *id != 22);
    assert!(!before.same_chat_permissions(&after));
}

/// Old multi-chat files keep exactly one owner; later pairings start without data access.
#[test]
fn legacy_pairing_order_has_one_owner_and_new_viewers_are_empty() {
    let mut cfg: TelegramConfig = toml::from_str("authorized_chat_ids = [11, 22]").unwrap();
    assert_eq!(cfg.report_access(11), Some(TelegramReportAccess::Owner));
    assert_eq!(
        cfg.report_access(22),
        Some(TelegramReportAccess::Viewer(vec![]))
    );
    cfg.authorized_chat_ids.push(33);
    assert_eq!(
        cfg.report_access(33),
        Some(TelegramReportAccess::Viewer(vec![]))
    );
    assert_eq!(cfg.report_access(44), None);
}

/// Owner transfer cannot preserve invisible grants that reactivate on demotion or revocation.
#[test]
fn transfer_revocation_and_round_trip_preserve_explicit_roles() {
    let mut cfg = TelegramConfig {
        authorized_chat_ids: vec![11, 22],
        ..Default::default()
    };
    cfg.chat_profile_mut(11).core_uids = vec![7];
    cfg.chat_profile_mut(22).core_uids = vec![9];
    assert!(cfg.set_owner(22));
    let saved: TelegramConfig = toml::from_str(&toml::to_string(&cfg).unwrap()).unwrap();
    assert_eq!(saved.report_access(22), Some(TelegramReportAccess::Owner));
    assert_eq!(
        saved.report_access(11),
        Some(TelegramReportAccess::Viewer(vec![]))
    );
    assert!(!cfg.set_owner(99));
    cfg.authorized_chat_ids.retain(|id| *id != 22);
    assert_eq!(cfg.owner(), None);
    assert_eq!(cfg.report_access(22), None);
    assert_eq!(
        cfg.report_access(11),
        Some(TelegramReportAccess::Viewer(vec![]))
    );
}

/// Stable uid grants survive serialization; zero never names an unsaved core.
#[test]
fn viewer_grants_are_canonical_and_do_not_authorize_unpaired_profiles() {
    let mut cfg = TelegramConfig {
        authorized_chat_ids: vec![11, 22],
        ..Default::default()
    };
    cfg.chat_profile_mut(22).core_uids = vec![7, 0, 3, 7];
    cfg.chat_profile_mut(33).core_uids = vec![8];
    assert_eq!(
        cfg.report_access(22),
        Some(TelegramReportAccess::Viewer(vec![3, 7]))
    );
    assert_eq!(cfg.report_access(33), None);
}
