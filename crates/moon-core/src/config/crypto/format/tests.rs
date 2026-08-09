//! Byte-format tests: the v1 layout, legacy detection, and what a damaged file must not do.

use super::*;
use crate::config::crypto::slots::{Header, Slot};

/// Build one machine-shaped slot with recognizable base64 content.
fn slot(id: &str) -> Slot {
    Slot {
        kind: "machine".to_string(),
        id: Some(id.to_string()),
        nonce: "AAAAAAAAAAAAAAAA".to_string(),
        wrapped: "BBBBBBBBBBBBBBBB".to_string(),
        salt: None,
        kdf: None,
        m_cost: None,
        t_cost: None,
        p_cost: None,
        created_ms: 7,
        extra: Default::default(),
    }
}

/// The header must survive a write/read cycle byte for byte, because those exact bytes are the
/// AES-GCM additional authenticated data. A re-serialization that differs in whitespace or field
/// order would authenticate against different bytes and make the payload undecryptable.
#[test]
fn a_header_round_trips_through_the_file_layout() {
    let mut header = Header::new();
    header.put_machine_slot(slot("first"));
    header.put_machine_slot(slot("second"));
    let bytes = header_bytes(&header).unwrap();
    let file = assemble(&bytes, &[9u8; NONCE_LEN], b"payload");

    let Parsed::V1 {
        header: parsed,
        header_bytes: parsed_bytes,
        nonce,
        ciphertext,
    } = parse(&file).unwrap()
    else {
        panic!("a file built by assemble must parse as v1");
    };
    assert_eq!(parsed_bytes, bytes.as_slice());
    assert_eq!(nonce, [9u8; NONCE_LEN]);
    assert_eq!(ciphertext, b"payload");
    assert_eq!(parsed.machine_slot_count(), 2);
    assert_eq!(header_bytes(&parsed).unwrap(), bytes);
}

/// Every file written before key slots existed starts with a random nonce, and copies of them sit
/// in `backups/settings/` forever. Reading one must keep working.
#[test]
fn a_file_without_the_magic_is_read_as_legacy() {
    let legacy: Vec<u8> = (0..40u8).collect();
    let Parsed::Legacy { nonce, ciphertext } = parse(&legacy).unwrap() else {
        panic!("a file with no magic is the legacy layout");
    };
    assert_eq!(nonce, &legacy[..NONCE_LEN]);
    assert_eq!(ciphertext, &legacy[NONCE_LEN..]);
}

/// A truncated file must be reported, not indexed into. Every length short of a complete header is
/// a slice that would panic in the frame the config is loaded on.
///
/// Note what this does NOT claim: a v1 file truncated inside its PAYLOAD still parses, because the
/// ciphertext is "whatever remains" and its length is not declared anywhere. Detecting that is the
/// authentication tag's job, and `crypto/tests.rs` asserts it — a length field here would only add
/// a second, unauthenticated opinion about the same thing.
#[test]
fn no_truncation_of_a_file_can_make_the_parser_index_out_of_bounds() {
    let header = Header::new();
    let bytes = header_bytes(&header).unwrap();
    let file = assemble(&bytes, &[1u8; NONCE_LEN], b"payload");

    for cut in 0..file.len() {
        // Reaching the assertions at all is most of the point: a slice-based parser panics here.
        match parse(&file[..cut]) {
            Ok(Parsed::V1 {
                header_bytes: parsed_bytes,
                nonce,
                ..
            }) => {
                assert_eq!(nonce.len(), NONCE_LEN);
                assert_eq!(parsed_bytes, bytes.as_slice());
            }
            // Below the magic, a prefix is indistinguishable from a legacy file. That is a
            // legitimate reading and fails later, at decryption.
            Ok(Parsed::Legacy { nonce, .. }) => {
                assert_eq!(nonce.len(), NONCE_LEN);
                assert!(cut < super::MAGIC.len());
            }
            Err(_) => {}
        }
    }
}

/// The header length is read from the file before anything is authenticated, so a damaged or
/// hostile one must not become an allocation request.
#[test]
fn an_absurd_header_length_is_refused_before_allocating() {
    let mut file = Vec::new();
    file.extend_from_slice(&super::MAGIC);
    file.extend_from_slice(&u32::MAX.to_le_bytes());
    file.extend_from_slice(b"whatever");
    let error = parse(&file).unwrap_err().to_string();
    assert!(error.contains("limit"), "unexpected error: {error}");
}

/// A file from a future build must say so instead of being parsed as far as it happens to fit.
#[test]
fn a_newer_header_version_is_refused() {
    let mut header = Header::new();
    header.version = Header::VERSION + 1;
    let bytes = header_bytes(&header).unwrap();
    let file = assemble(&bytes, &[0u8; NONCE_LEN], b"x");
    let error = parse(&file).unwrap_err().to_string();
    assert!(error.contains("newer"), "unexpected error: {error}");
}
