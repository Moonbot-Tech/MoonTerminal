//! Vault tests: which key opens which file, and what happens when none does.
//!
//! Every test supplies the machine key explicitly through [`open_file`] instead of going through
//! `decrypt_config`. The keyring is real OS state shared with the developer's own installation, so
//! touching it from a test would either disturb a working config or fail on a CI runner that has
//! no key store — and the decision this module exists to make is the one BETWEEN keys, which does
//! not need one.

use super::*;

/// Build a v1 file whose payload is `plain`, wrapped for `machine_key` and optionally `password`.
fn build_file(plain: &[u8], machine_key: Option<&[u8; 32]>, password: Option<&str>) -> Vec<u8> {
    let file_key = wrap::new_file_key().unwrap();
    let mut header = Header::new();
    if let Some(machine_key) = machine_key {
        let id = machine::slot_id(machine_key);
        header.put_machine_slot(wrap::machine_slot(&file_key, machine_key, id).unwrap());
    }
    if let Some(password) = password {
        header.set_password_slot(Some(test_password_slot(&file_key, password)));
    }
    let header_bytes = format::header_bytes(&header).unwrap();
    let (nonce, ciphertext) = wrap::seal(&file_key, plain, &header_bytes).unwrap();
    format::assemble(&header_bytes, &nonce, &ciphertext)
}

/// Wrap the file key under a password at the cheapest KDF cost the algorithm accepts.
///
/// The shipped 64 MiB derivation is not what these tests are checking, and paying it twice per
/// password test would dominate the suite's runtime.
fn test_password_slot(file_key: &[u8; 32], password: &str) -> Slot {
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD as B64;

    let salt = [3u8; kdf::SALT_LEN];
    let params = kdf::KdfParams {
        m_cost: 8,
        t_cost: 1,
        p_cost: 1,
    };
    let wrapping = kdf::derive(password, &salt, params).unwrap();
    let (nonce, wrapped) = wrap::seal(&wrapping, file_key.as_slice(), &[]).unwrap();
    Slot {
        kind: KIND_PASSWORD.to_string(),
        id: None,
        nonce: B64.encode(nonce),
        wrapped: B64.encode(wrapped),
        salt: Some(B64.encode(salt)),
        kdf: Some(kdf::ALGORITHM.to_string()),
        m_cost: Some(params.m_cost),
        t_cost: Some(params.t_cost),
        p_cost: Some(params.p_cost),
        created_ms: 1,
        extra: Default::default(),
    }
}

/// The ordinary case: the machine that wrote the file opens it with no prompt and no rewrite.
#[test]
fn the_writing_machine_opens_its_own_file_silently() {
    let machine_key = [1u8; 32];
    let file = build_file(b"servers", Some(&machine_key), None);

    let (plain, material) = open_file(&file, Some(&machine_key)).unwrap();
    assert_eq!(plain, b"servers");
    assert!(
        !material.needs_rewrite,
        "opening a current file must not schedule a write"
    );
    assert_eq!(material.header.machine_slot_count(), 1);
}

/// The case the whole feature is for: the file arrives on a machine with no slot of its own. With
/// a password slot present it must ASK, not declare the file unreadable.
#[test]
fn a_foreign_machine_is_asked_for_the_password_when_one_exists() {
    let file = build_file(b"servers", Some(&[1u8; 32]), Some("correct horse"));

    let error = open_file(&file, Some(&[2u8; 32])).unwrap_err();
    assert!(
        matches!(error, AccessError::NeedsPassword),
        "expected a password prompt, got {error:?}"
    );
}

/// Without a password slot there is genuinely no way in, and saying so is what lets the UI offer
/// starting over instead of leaving the user at a prompt no password can satisfy.
#[test]
fn a_foreign_machine_without_a_password_slot_reports_no_key() {
    let file = build_file(b"servers", Some(&[1u8; 32]), None);

    assert!(matches!(
        open_file(&file, Some(&[2u8; 32])).unwrap_err(),
        AccessError::NoKey
    ));
    // An empty keyring is the same situation, not a different one.
    assert!(matches!(
        open_file(&file, None).unwrap_err(),
        AccessError::NoKey
    ));
}

/// The header is plaintext by necessity, so it must be authenticated: editing it — here, deleting
/// the password slot to force a downgrade — has to break decryption rather than silently change
/// which keys the reader believes in.
#[test]
fn editing_the_header_invalidates_the_payload() {
    let machine_key = [1u8; 32];
    let file = build_file(b"servers", Some(&machine_key), Some("correct horse"));

    let Parsed::V1 {
        mut header,
        nonce,
        ciphertext,
        ..
    } = format::parse(&file).unwrap()
    else {
        panic!("built file must be v1");
    };
    header.set_password_slot(None);
    let forged_header = format::header_bytes(&header).unwrap();
    let forged = format::assemble(&forged_header, nonce, ciphertext);

    let error = open_file(&forged, Some(&machine_key)).unwrap_err();
    assert!(
        matches!(error, AccessError::Damaged(_)),
        "a tampered header must not open, got {error:?}"
    );
}

/// A file from before key slots must keep opening with the keyring key, and must schedule its own
/// upgrade — nothing else would ever ask for that write, and without it the conversion is retried
/// on every launch forever.
#[test]
fn a_legacy_file_opens_and_schedules_its_upgrade() {
    let machine_key = [1u8; 32];
    let (nonce, ciphertext) = wrap::seal(&machine_key, b"legacy servers", &[]).unwrap();
    let mut file = nonce.to_vec();
    file.extend_from_slice(&ciphertext);

    let (plain, material) = open_file(&file, Some(&machine_key)).unwrap();
    assert_eq!(plain, b"legacy servers");
    assert!(
        material.needs_rewrite,
        "a legacy file must be rewritten in the new format"
    );
    assert_eq!(material.header.machine_slot_count(), 1);
    assert!(!material.header.has_password_slot());
}

/// The gate that makes the login window safe to add: once a file failed to open, saving must
/// refuse. Otherwise the next ordinary settings write replaces a recoverable file — the one the
/// user was about to type their password into — with a fresh one.
#[test]
fn a_sealed_vault_refuses_to_produce_file_bytes() {
    for reason in [
        SealReason::NeedsPassword,
        SealReason::NoKey,
        SealReason::Damaged,
        SealReason::Backend,
    ] {
        let error = refuse_if_sealed(&Vault::Sealed(reason))
            .expect_err("a sealed vault must refuse to write")
            .to_string();
        assert!(error.contains("запрещена"), "unexpected message: {error}");
    }
    refuse_if_sealed(&Vault::Unopened).expect("a fresh install must be allowed to write");
}

/// No prefix of a valid file may decrypt. The payload length is not declared anywhere in the
/// format, so a file cut short by a failed write or a half-synchronized cloud copy still parses —
/// the authentication tag is what has to reject it, and it must reject it as damage rather than
/// returning a shorter "config" that would then be parsed as TOML and saved back.
#[test]
fn no_truncation_of_a_valid_file_decrypts() {
    let machine_key = [1u8; 32];
    let file = build_file(b"servers = []", Some(&machine_key), None);

    for cut in 0..file.len() {
        assert!(
            open_file(&file[..cut], Some(&machine_key)).is_err(),
            "a file truncated to {cut} of {} bytes decrypted",
            file.len()
        );
    }
}

/// A file this process has already opened must reopen with the key it is HOLDING, not by asking
/// the keyring again.
///
/// This is the step that makes the encryption password work end to end. Unlocking with a password
/// adds this machine's slot in memory; the file on disk still has no such slot until the save that
/// follows. A reader that always re-derives from the keyring looks at that unchanged header, finds
/// nothing for this machine and reports "needs password" — one step after the user typed the
/// correct one, which startup then treats as a failure to open and quits.
#[test]
fn a_file_open_in_this_process_reopens_with_the_held_key() {
    let _guard = vault_test_lock();
    // Wrapped for a machine whose key we deliberately do NOT have, exactly like a file that just
    // arrived from elsewhere and was opened with its password.
    let file = build_file(b"servers = []", Some(&[9u8; 32]), None);
    let (_, material) = open_file(&file, Some(&[9u8; 32])).unwrap();
    *vault() = Vault::Open(material);

    let plain = decrypt_config(&file).expect("a held file key must reopen its own file");
    assert_eq!(plain, b"servers = []");

    // And the in-memory slots survive the reopen: they may already carry a machine slot that the
    // file on disk has not received yet, which is precisely what the following save persists.
    assert!(access_state().opened);
    *vault() = Vault::Unopened;
}

/// Serialize the tests that touch the process-wide vault.
///
/// Only these tests mutate it; every other test in this file works on `open_file` directly, which
/// is why they need no lock.
fn vault_test_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|error| error.into_inner())
}

/// A sealed vault must be reopenable after the unopenable file has been moved aside, or the empty
/// configuration the user chose to start from cannot be written at all.
#[test]
fn forgetting_a_sealed_file_allows_a_fresh_one() {
    let _guard = vault_test_lock();
    *vault() = Vault::Sealed(SealReason::NoKey);
    assert!(refuse_if_sealed(&vault()).is_err());

    forget_sealed_file();
    assert!(
        refuse_if_sealed(&vault()).is_ok(),
        "after the file is set aside, writing a new one must be allowed"
    );
    *vault() = Vault::Unopened;
}

/// Wrapping the same key for a second machine must not disturb the first: both slots open the
/// same payload afterwards. A shared file that opened on only one machine at a time is exactly the
/// ping-pong the per-machine slots exist to prevent.
#[test]
fn a_second_machine_can_be_added_without_locking_out_the_first() {
    let first = [1u8; 32];
    let second = [2u8; 32];
    let file = build_file(b"servers", Some(&first), None);

    let (_, mut material) = open_file(&file, Some(&first)).unwrap();
    add_machine(&mut material, &second).unwrap();
    let header_bytes = format::header_bytes(&material.header).unwrap();
    let (nonce, ciphertext) = wrap::seal(&material.key, b"servers", &header_bytes).unwrap();
    let shared = format::assemble(&header_bytes, &nonce, &ciphertext);

    assert_eq!(open_file(&shared, Some(&first)).unwrap().0, b"servers");
    assert_eq!(open_file(&shared, Some(&second)).unwrap().0, b"servers");
}
