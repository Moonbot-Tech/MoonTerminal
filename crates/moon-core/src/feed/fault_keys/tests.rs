use super::*;

/// One of every raw fault known today. A new variant must be added here by hand: `fault_kind`'s
/// exhaustive match forces it a kind, this list is what then checks that kind has a label.
fn every_fault() -> Vec<ConnFaultKind> {
    vec![
        ConnFaultKind::KeyUnparsable { empty: true },
        ConnFaultKind::KeyUnparsable { empty: false },
        ConnFaultKind::LocalBindFailed {
            consecutive_failures: 1,
        },
        ConnFaultKind::Aborted,
        ConnFaultKind::ConnectTimedOut { timeout_ms: 1 },
        ConnFaultKind::NotAuthenticated,
        ConnFaultKind::InitStepTimedOut {
            step: None,
            raw_step: String::new(),
        },
        ConnFaultKind::StartupStalled,
        ConnFaultKind::InitStepFailed {
            step: None,
            raw_step: String::new(),
            message: String::new(),
        },
    ]
}

/// Every kind the Mini App can receive has exactly one short label, and every label is a kind.
///
/// Breakage: a new `fault_kind` value missing from the table reaches the page with no label to
/// show, and a stale table row names a kind no core can produce.
#[test]
fn every_fault_kind_has_exactly_one_short_label() {
    let kinds: Vec<&str> = every_fault().iter().map(fault_kind).collect();
    for kind in &kinds {
        let rows = FAULT_KIND_SHORT_KEYS
            .iter()
            .filter(|(k, _)| k == kind)
            .count();
        assert_eq!(rows, 1, "{kind} must have exactly one short label");
    }
    for (kind, _) in FAULT_KIND_SHORT_KEYS {
        assert!(
            kinds.contains(kind),
            "{kind} is not a kind fault_kind returns"
        );
    }
}

/// Both selections name keys from the one short-label family.
#[test]
fn both_selections_use_the_short_label_keys() {
    for (_, key) in FAULT_KIND_SHORT_KEYS {
        assert!(key.starts_with("core_status.fault.short."), "{key}");
    }
    assert_eq!(
        failure_short_key(&FailureClass::Aborted),
        "core_status.fault.short.aborted"
    );
}
