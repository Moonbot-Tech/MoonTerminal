//! How a journal entry reads, including entries the installed Station wrote before the journal
//! kept what changed.
//!
//! Explicit imports (no `use super::*`, which would bring in gpui's `test` macro).

use moon_core::station_api::mcp::{AuditChange, AuditEntry, CoreInfo};

use super::{access_line, change_line, core_names, headline, outcome_label};

fn entry() -> AuditEntry {
    AuditEntry {
        at_ms: 0,
        grant_id: "g".into(),
        action: "strategy_edit".into(),
        cores: vec![3],
        outcome: "confirmed".into(),
        operation: "op".into(),
        client_name: "Claude".into(),
        subject: Some("Night".into()),
        changes: Vec::new(),
        changes_omitted: 0,
    }
}

#[test]
fn a_change_reads_as_a_transition_only_when_there_was_an_earlier_value() {
    let edited = AuditChange {
        field: "StopLoss".into(),
        before: Some("2.5".into()),
        after: Some("3".into()),
    };
    assert_eq!(change_line(&edited), "StopLoss: 2.5 → 3");
    let placed = AuditChange {
        field: "price".into(),
        before: None,
        after: Some("100".into()),
    };
    assert_eq!(change_line(&placed), "price: 100");
}

/// Installed Stations hold entries with no client, subject or changes, and with the operation id
/// inside the outcome. They must read cleanly: no empty separators, no raw id on screen.
#[test]
fn an_entry_from_before_the_journal_reads_without_gaps() {
    // Loads the dictionary on a stack that fits it: a first `t!()` on a test thread overflows.
    let _locale = crate::test_locale::force("en");
    let old = AuditEntry {
        client_name: String::new(),
        subject: None,
        outcome: "admitted:abc123".into(),
        operation: String::new(),
        ..entry()
    };
    assert!(!outcome_label(&old.outcome).contains("abc123"));
    let line = headline(&old, &[], "Edit", "Accepted");
    assert!(!line.contains("·  ·"), "{line}");
    assert!(line.ends_with("#3 · Edit · Accepted"), "{line}");
}

#[test]
fn the_headline_names_who_changed_what_and_where() {
    let cores = [CoreInfo {
        uid: 3,
        name: "BinF1".into(),
    }];
    let line = headline(&entry(), &cores, "Edit", "Confirmed");
    assert!(
        line.ends_with("Claude · BinF1 · Edit «Night» · Confirmed"),
        "{line}"
    );
}

/// A core removed from the Station keeps its entries, under its number.
#[test]
fn a_removed_core_is_named_by_its_number() {
    assert_eq!(core_names(&entry(), &[]), "#3");
}

/// The record holds `#uid` and protocol words; the owner reads the core's name, and a core removed
/// since keeps its number rather than vanishing from the line about its permissions.
#[test]
fn a_change_of_access_reads_with_the_cores_name() {
    let cores = [CoreInfo {
        uid: 3,
        name: "BinF1".into(),
    }];
    let shout = |word: &str| word.to_uppercase();
    let widened = AuditChange {
        field: "#3".into(),
        before: Some("read".into()),
        after: Some("read, trading".into()),
    };
    assert_eq!(
        access_line(&widened, &cores, shout),
        "BinF1: READ → READ, TRADING"
    );
    let removed = AuditChange {
        field: "#9".into(),
        before: Some("read".into()),
        after: None,
    };
    assert_eq!(access_line(&removed, &cores, shout), "#9: READ → —");
}
