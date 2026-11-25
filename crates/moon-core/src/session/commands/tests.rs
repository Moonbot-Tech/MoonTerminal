//! Exact-text and echo-aware blacklist undo regressions.

use std::time::Instant;

use super::{BlacklistAdd, SentBlacklist, blacklist_token_edit};

/// Prepare the mutation and inverse used by the session add path.
fn add(base: (bool, String), coin: &str) -> ((bool, String), BlacklistAdd) {
    let (sent, undo) = blacklist_token_edit(base, coin, false, false);
    (sent.unwrap(), undo.unwrap())
}

/// Undo must restore formatting and the disabled flag, even after add normalizes the text.
#[test]
fn add_then_revert_restores_text_byte_for_byte() {
    for text in ["", "A,B", "A, B", "A,B,"] {
        let (sent, undo) = add((false, text.to_string()), "XALPHA");
        assert_eq!(
            undo.revert(sent.0, &sent.1, false),
            (false, text.to_string())
        );
    }
}

/// An undo issued before the echo must work from the newest queued text.
#[test]
fn add_a_then_b_before_echo_revert_a_keeps_b_and_flag() {
    let in_store = (false, String::new());
    let (sent, undo_a) = add(in_store.clone(), "XALPHA");
    let mut pending = SentBlacklist {
        before: in_store.clone(),
        sent,
        at: Instant::now(),
    };
    let (sent_b, _) = add(pending.effective(&in_store).clone(), "XBETA");
    assert_eq!(sent_b.1, "XALPHA,XBETA");
    pending.sent = sent_b;
    let effective = pending.effective(&in_store);
    assert_eq!(
        undo_a.revert(effective.0, &effective.1, false),
        (true, "XBETA".to_string())
    );
}

/// A competing edit owns its text and switch except for the inserted token.
#[test]
fn revert_after_intervening_edit_removes_only_token() {
    let (_, undo) = add((false, "A,B,".to_string()), "XALPHA");
    assert_eq!(
        undo.revert(true, "XALPHA, B, XCETUS,", false),
        (true, " B, XCETUS,".to_string())
    );
}

/// The preexisting enabled switch survives exact-text undo.
#[test]
fn revert_does_not_disable_flag_it_did_not_enable() {
    let (sent, undo) = add((true, "A, B".to_string()), "XALPHA");
    assert_eq!(
        undo.revert(sent.0, &sent.1, false),
        (true, "A, B".to_string())
    );
}

/// An existing token can enable the list, without being misrepresented as an insertion.
#[test]
fn enabling_existing_token_does_not_return_inverse() {
    let (sent, undo) =
        blacklist_token_edit((false, "A, XALPHA".to_string()), "xalpha", false, false);
    assert_eq!(sent, Some((true, "A, XALPHA".to_string())));
    assert!(undo.is_none());
}
