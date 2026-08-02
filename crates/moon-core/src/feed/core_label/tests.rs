use super::*;

/// Ids are chosen per test because the registry is process-wide and tests share it.
#[test]
fn a_label_carries_the_name_registered_for_that_id() {
    set_core_name(9001, "BB1");
    assert_eq!(core_label(9001).to_string(), "9001 «BB1»");
}

/// Regression target: resolving the name when the label is CREATED rather than when it is formatted
/// would freeze the first name a core ever had into every later line.
#[test]
fn a_renamed_core_logs_under_its_new_name() {
    set_core_name(9002, "Old");
    let label = core_label(9002);
    set_core_name(9002, "New");
    assert_eq!(label.to_string(), "9002 «New»");
}

/// A core that has not started yet — or has stopped — must still be identifiable by id rather than
/// producing an empty or misleading name.
#[test]
fn an_unknown_core_falls_back_to_the_bare_id() {
    assert_eq!(core_label(9003).to_string(), "9003");
    set_core_name(9003, "Gone");
    clear_core_name(9003);
    assert_eq!(core_label(9003).to_string(), "9003");
}
