use super::*;

#[test]
fn an_older_agreement_does_not_cover_the_current_one() {
    let old = EulaAcceptance {
        version: "moonbot-eula-2025-01".into(),
        accepted_at_ms: 1,
    };
    assert!(!old.covers_current());
}

#[test]
fn a_file_without_a_version_reads_as_not_accepted() {
    // A hand-edited or truncated file must make the wizard ask, never count as consent.
    let parsed: EulaAcceptance = serde_json::from_str("{}").unwrap();
    assert!(!parsed.covers_current());
}
