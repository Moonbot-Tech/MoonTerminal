//! Slot bookkeeping: replacement by id, eviction, revocation, and forward compatibility.

use super::*;

/// Build a slot of the given kind, id and age.
fn slot(kind: &str, id: Option<&str>, created_ms: i64) -> Slot {
    Slot {
        kind: kind.to_string(),
        id: id.map(str::to_string),
        nonce: "n".to_string(),
        wrapped: "w".to_string(),
        salt: None,
        kdf: None,
        m_cost: None,
        t_cost: None,
        p_cost: None,
        created_ms,
        extra: Default::default(),
    }
}

/// A machine that re-wraps its own slot must replace it, not append. Appending would let one
/// machine consume the whole budget by itself and evict every other machine that shares the file.
#[test]
fn a_machine_replaces_its_own_slot_instead_of_adding_one() {
    let mut header = Header::new();
    header.put_machine_slot(slot(KIND_MACHINE, Some("me"), 1));
    header.put_machine_slot(slot(KIND_MACHINE, Some("me"), 2));
    header.put_machine_slot(slot(KIND_MACHINE, Some("other"), 3));

    assert_eq!(header.machine_slot_count(), 2);
    let mine: Vec<i64> = header
        .slots
        .iter()
        .filter(|s| s.id.as_deref() == Some("me"))
        .map(|s| s.created_ms)
        .collect();
    assert_eq!(mine, vec![2], "the newer wrap must have replaced the older");
}

/// The budget exists so a file that has travelled between many machines does not grow forever and
/// does not keep a decommissioned machine able to open it. The OLDEST must go.
#[test]
fn exceeding_the_budget_evicts_the_oldest_machine() {
    let mut header = Header::new();
    for index in 0..MAX_MACHINE_SLOTS as i64 + 3 {
        header.put_machine_slot(slot(KIND_MACHINE, Some(&format!("m{index}")), index));
    }
    assert_eq!(header.machine_slot_count(), MAX_MACHINE_SLOTS);
    assert!(
        header.slots.iter().all(|s| s.created_ms >= 3),
        "the three oldest machines should have been evicted"
    );
}

/// Eviction must never reach the password slot: it is not a machine, and it is the user's way back
/// in on a machine that has no slot at all.
#[test]
fn eviction_and_revocation_both_spare_the_password_slot() {
    let mut header = Header::new();
    header.set_password_slot(Some(slot(KIND_PASSWORD, None, 0)));
    for index in 0..MAX_MACHINE_SLOTS as i64 + 4 {
        header.put_machine_slot(slot(KIND_MACHINE, Some(&format!("m{index}")), index));
    }
    assert!(header.has_password_slot());

    header.retain_only_machine("m9");
    assert!(
        header.has_password_slot(),
        "revoking other machines must not remove the password slot"
    );
    assert_eq!(header.machine_slot_count(), 1);
}

/// Revocation keeps exactly the caller's machine, including when that id is not present — the
/// caller is then left with no machine slot rather than with someone else's.
#[test]
fn revocation_keeps_only_the_named_machine() {
    let mut header = Header::new();
    header.put_machine_slot(slot(KIND_MACHINE, Some("a"), 1));
    header.put_machine_slot(slot(KIND_MACHINE, Some("b"), 2));

    header.retain_only_machine("a");
    assert_eq!(header.machine_slot_count(), 1);
    assert_eq!(header.slots[0].id.as_deref(), Some("a"));
}

/// Setting a password twice must leave ONE password slot. Two would both be valid keys to the
/// file, so a "changed" password would silently keep opening with the old one.
#[test]
fn setting_a_password_twice_leaves_one_slot() {
    let mut header = Header::new();
    header.set_password_slot(Some(slot(KIND_PASSWORD, None, 1)));
    header.set_password_slot(Some(slot(KIND_PASSWORD, None, 2)));
    assert_eq!(
        header
            .slots
            .iter()
            .filter(|s| s.kind == KIND_PASSWORD)
            .count(),
        1
    );

    header.set_password_slot(None);
    assert!(!header.has_password_slot());
}

/// A slot kind introduced by a newer build must survive an older build reading and rewriting the
/// header. With a serde enum this deserialization fails outright and the file becomes unreadable
/// by the exact build the user downgraded to in order to open it.
#[test]
fn an_unknown_slot_kind_survives_a_read_write_cycle() {
    let source = r#"
version = 1

[[slot]]
kind = "smartcard"
nonce = "n"
wrapped = "w"
reader = "future-field"

[[slot]]
kind = "machine"
id = "me"
nonce = "n"
wrapped = "w"
created_ms = 5
"#;
    let mut header: Header = toml::from_str(source).unwrap();
    assert_eq!(header.slots.len(), 2);
    assert_eq!(header.machine_slot_count(), 1);

    // An unrelated edit must not drop the slot we do not understand, NOR the fields that make it
    // usable: a slot rewritten without them looks present and opens nothing.
    header.put_machine_slot(slot(KIND_MACHINE, Some("me"), 6));
    let text = toml::to_string(&header).unwrap();
    assert!(text.contains("smartcard"), "rewrote header as: {text}");
    assert!(
        text.contains("future-field"),
        "unknown slot fields were stripped: {text}"
    );

    // And the round trip must be stable, since these bytes are authenticated.
    let reparsed: Header = toml::from_str(&text).unwrap();
    assert_eq!(toml::to_string(&reparsed).unwrap(), text);
}
