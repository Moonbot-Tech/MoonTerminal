//! Unit tests for normalized strategy-section label lookup.

use super::{SECTION_LABELS, section_label_key, section_title_eq};

/// `sections.rs::section_title_eq`: replacing normalized comparison with raw equality would leave
/// runtime spelling variants untranslated, so the section list would show raw Moonbot headings.
#[test]
fn section_titles_normalize_runtime_spelling_without_collisions() {
    let dynamic_slash = section_label_key("Dynamic White/Black List");
    assert_eq!(
        section_label_key("Dynamic White\\Black List"),
        dynamic_slash
    );
    assert!(dynamic_slash.is_some());
    assert_eq!(
        section_label_key("Triggers  Master / Slave"),
        section_label_key("Triggers Master / Slave")
    );
    assert_eq!(
        section_label_key("Filters/Base"),
        section_label_key("Filters / Base")
    );
    assert_eq!(section_label_key(" main "), section_label_key("Main"));
    assert_ne!(
        section_label_key("Filters / Delta"),
        section_label_key("Delta Modifiers")
    );
    assert_eq!(section_label_key("Not a Moonbot section"), None);

    for (left, _) in SECTION_LABELS {
        assert!(section_title_eq(left, left));
        for (right, _) in SECTION_LABELS {
            if left != right {
                assert!(
                    !section_title_eq(left, right),
                    "canonical section titles {left:?} and {right:?} must not normalize together"
                );
            }
        }
    }
}
