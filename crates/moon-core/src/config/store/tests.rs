//! Tests for the config-store guards.

use super::refuse_blind_overwrite;

/// The four states of "may this save replace servers.enc", stated as a table so a future edit to
/// the condition has to disagree with the table rather than with a sentence.
///
/// The dangerous cell is the third: a file exists and this process never opened it. That is
/// `MOON_CONFIG_PLAINTEXT` with a real installation present, and letting it through replaces the
/// user's cores and their password slot with synthetic test material.
#[test]
fn only_an_unopened_existing_file_blocks_a_write() {
    // No file yet: a first launch must be able to create one.
    assert!(refuse_blind_overwrite(false, false).is_ok());
    // No file, config opened: nothing to lose either way.
    assert!(refuse_blind_overwrite(false, true).is_ok());
    // A file exists and was never opened: refuse.
    assert!(refuse_blind_overwrite(true, false).is_err());
    // A file exists and we hold its key material: the normal save.
    assert!(refuse_blind_overwrite(true, true).is_ok());
}

/// The refusal has to explain itself in the settings footer, where it is the only thing the user
/// will see. "Failed to save" with no reason reads as a bug and invites a retry loop.
#[test]
fn the_refusal_says_why() {
    let message = refuse_blind_overwrite(true, false).unwrap_err().to_string();
    assert!(message.contains("не был прочитан"), "got: {message}");
}
