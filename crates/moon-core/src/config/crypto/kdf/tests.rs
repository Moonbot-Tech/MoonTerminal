//! KDF tests. Deliberately run at the SMALLEST parameters Argon2 accepts rather than the shipped
//! ones: this checks wiring and clamping, and a 64 MiB derivation per assertion would make the
//! suite pay for cryptographic strength it is not testing.

use super::*;

/// Minimal-cost parameters for the assertions below.
fn cheap() -> KdfParams {
    KdfParams {
        m_cost: 8,
        t_cost: 1,
        p_cost: 1,
    }
}

/// The same password, salt and parameters must always yield the same key — the file is unwrapped
/// on a different machine and a different day than it was wrapped on.
#[test]
fn derivation_is_deterministic() {
    let first = derive("correct horse", b"0123456789abcdef", cheap()).unwrap();
    let second = derive("correct horse", b"0123456789abcdef", cheap()).unwrap();
    assert_eq!(first.as_ref(), second.as_ref());
}

/// The salt is what stops one derived key from opening two files, and stops a precomputed table
/// from opening any of them.
#[test]
fn the_salt_changes_the_key() {
    let first = derive("correct horse", b"0123456789abcdef", cheap()).unwrap();
    let second = derive("correct horse", b"fedcba9876543210", cheap()).unwrap();
    assert_ne!(first.as_ref(), second.as_ref());
}

/// Parameters are part of the key, so a file that records different costs must not be openable by
/// deriving at ours.
#[test]
fn the_cost_parameters_change_the_key() {
    let first = derive("correct horse", b"0123456789abcdef", cheap()).unwrap();
    let second = derive(
        "correct horse",
        b"0123456789abcdef",
        KdfParams {
            t_cost: 2,
            ..cheap()
        },
    )
    .unwrap();
    assert_ne!(first.as_ref(), second.as_ref());
}

/// `m_cost` is an allocation request read from a file that may be damaged or hostile. Accepting it
/// unchecked turns opening a config into an out-of-memory abort.
#[test]
fn an_impossible_memory_cost_is_refused_rather_than_attempted() {
    assert!(KdfParams::from_slot(u32::MAX, 3, 1).is_err());
    assert!(KdfParams::from_slot(64 * 1024, u32::MAX, 1).is_err());
    assert!(KdfParams::from_slot(64 * 1024, 3, 1024).is_err());
    assert!(KdfParams::from_slot(64 * 1024, 3, 1).is_ok());
}

/// The parameters we write must be ones we accept back. A raise that forgets the ceiling would
/// produce files this build cannot open.
#[test]
fn the_shipped_parameters_pass_our_own_ceiling() {
    let current = KdfParams::current();
    let accepted = KdfParams::from_slot(current.m_cost, current.t_cost, current.p_cost).unwrap();
    assert_eq!(accepted, current);
}
