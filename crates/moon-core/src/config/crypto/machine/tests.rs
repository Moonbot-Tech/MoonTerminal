//! Slot-id tests. The keyring itself is deliberately untested here: it is OS state shared with
//! the developer's real installation, and a test that wrote to it would either corrupt a working
//! config or fail on a CI runner with no key store.

use super::*;

/// The id is how a reader finds its own slot, so it must be a pure function of the key: the same
/// machine has to recognize its slot on every launch, across restarts and versions.
#[test]
fn the_slot_id_is_stable_for_one_key() {
    let key = [7u8; 32];
    assert_eq!(slot_id(&key), slot_id(&key));
}

/// Two machines must not collide onto one slot id — a collision means one machine overwrites the
/// other's slot and locks it out of a file they share.
#[test]
fn different_keys_get_different_slot_ids() {
    let mut other = [7u8; 32];
    other[31] = 8;
    assert_ne!(slot_id(&[7u8; 32]), slot_id(&other));
}

/// The id is published in the plaintext header, so it must be a hash rather than any part of the
/// key. Catching a rewrite that "simplified" this to an encoding of the key itself is the entire
/// point of this test.
#[test]
fn the_slot_id_does_not_expose_the_key() {
    let key = [7u8; 32];
    let id = slot_id(&key);
    let decoded = B64.decode(&id).unwrap();
    assert_eq!(decoded.len(), SLOT_ID_LEN);
    assert!(
        !decoded.iter().all(|byte| *byte == 7),
        "the id must not be the key material"
    );
    assert!(
        !B64.encode(key).contains(id.as_str()),
        "the id must not be a prefix of the encoded key"
    );
}
