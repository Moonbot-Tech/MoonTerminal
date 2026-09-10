//! Public authorization-boundary regressions for Telegram pairing.

use std::time::{Duration, Instant};

use moon_core::telegram::auth::{AuthReject, Authorization};

/// `telegram::auth::Authorization::is_authorized` must keep an unpaired chat outside the set,
/// and pairing must remain six-character, one-use, ten-minute admission; otherwise a stranger
/// who finds the public bot name can control the terminal without completing `/pair`.
#[test]
fn pairing_is_the_only_temporary_path_to_chat_authorization() {
    let started = Instant::now();
    let mut authorization = Authorization::from_authorized_chats([55]);

    assert!(authorization.is_authorized(55));
    assert!(
        !authorization.is_authorized(91),
        "an unpaired chat must be refused before it presents the current pairing code"
    );

    let code = authorization
        .issue_pairing_code(started)
        .expect("the operating system must provide pairing entropy in the test environment");
    assert_eq!(
        code.len(),
        6,
        "the Settings pairing code has six characters"
    );
    assert!(
        code.bytes()
            .all(|byte| b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789".contains(&byte)),
        "the displayed code must use the unambiguous pairing alphabet"
    );

    assert_eq!(
        authorization.pair(91, &code, started + Duration::from_secs(9 * 60 + 59)),
        Ok(moon_core::telegram::auth::PairingAccepted { chat_id: 91 })
    );
    assert!(authorization.is_authorized(91));
    assert_eq!(
        authorization.pair(92, &code, started + Duration::from_secs(9 * 60 + 59)),
        Err(AuthReject::BadPairingCode),
        "a successful /pair consumes the code so it cannot admit another chat"
    );

    let expiring = authorization
        .issue_pairing_code(started)
        .expect("the operating system must provide pairing entropy in the test environment");
    assert_eq!(
        authorization.pair(93, &expiring, started + Duration::from_secs(10 * 60)),
        Err(AuthReject::BadPairingCode),
        "the ten-minute boundary is exclusive"
    );
    assert!(!authorization.is_authorized(93));

    authorization.revoke_chat(91);
    assert!(
        !authorization.is_authorized(91),
        "revocation must remove a previously paired chat from the admission set"
    );
}
