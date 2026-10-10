use super::*;

/// `command_vocab.rs:UpdateTarget::expected_suffix` must keep the letter's case and must not
/// turn a bare `MoonBot-` into an empty letter. Folding case, or treating the bare prefix as a
/// release, would judge `moonbot-r3` against `R3` and would accept a prefix-only name as installed.
#[test]
fn expected_suffix_strips_only_a_moonbot_prefix() {
    assert_eq!(
        UpdateTarget::Named("MoonBot-R3".to_string()).expected_suffix(),
        Some("R3")
    );
    assert_eq!(
        UpdateTarget::Named("moonbot-r3".to_string()).expected_suffix(),
        Some("r3")
    );
    assert_eq!(
        UpdateTarget::Named("MoonBot-".to_string()).expected_suffix(),
        Some("MoonBot-")
    );
    assert_eq!(
        UpdateTarget::Named("Custom".to_string()).expected_suffix(),
        Some("Custom")
    );
    assert_eq!(UpdateTarget::Release.expected_suffix(), None);
}
