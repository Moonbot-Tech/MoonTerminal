use super::*;
use russh::keys::ssh_key::LineEnding;
use russh::keys::ssh_key::private::{Ed25519Keypair, KeypairData};

/// A throwaway ed25519 key from a fixed seed, as an OpenSSH file.
fn ed25519_file() -> String {
    let pair = KeypairData::from(Ed25519Keypair::from_seed(&[7u8; 32]));
    let key = PrivateKey::new(pair, "test").expect("key");
    key.to_openssh(LineEnding::LF).expect("encode").to_string()
}

#[test]
fn reads_a_plain_openssh_ed25519_key() {
    let key = parse(&ed25519_file(), None).expect("a plain key reads");
    let line = authorized_line(&key).expect("public line");
    assert!(line.starts_with("ssh-ed25519 "), "{line}");
}

#[test]
fn a_passphrase_given_to_a_plain_key_is_not_a_wrong_one() {
    // The reader ignores a passphrase a plain file does not need.
    assert!(parse(&ed25519_file(), Some("unused")).is_ok());
}

/// Removing RSA support breaks provider login; independently generated keys must sign verifiable
/// SHA-2 signatures in every accepted container, with the same public identity.
#[test]
fn provider_rsa_formats_parse_and_sign() {
    use russh::keys::{Algorithm, HashAlg};

    let expected = russh::keys::PublicKey::from_openssh(include_str!("fixtures/rsa.pub"))
        .expect("independently serialized public key");
    for file in [
        include_str!("fixtures/rsa-pkcs1.pem"),
        include_str!("fixtures/rsa-openssh.pem"),
        include_str!("fixtures/rsa-putty.ppk"),
        include_str!("fixtures/rsa-pkcs8.pem"),
    ] {
        for password in [None, Some("unused")] {
            let key = parse(file, password).expect("plain provider RSA key");
            assert_eq!(key.public_key().key_data(), expected.key_data());
            let signature = key
                .sign(
                    "provider-login",
                    HashAlg::Sha256,
                    b"synthetic login challenge",
                )
                .expect("RSA signing enabled");
            assert!(matches!(
                signature.signature().algorithm(),
                Algorithm::Rsa {
                    hash: Some(HashAlg::Sha256 | HashAlg::Sha512)
                }
            ));
            expected
                .verify("provider-login", b"synthetic login challenge", &signature)
                .expect("signature verifies against independently serialized public key");
            assert!(
                expected
                    .verify("provider-login", b"changed challenge", &signature)
                    .is_err()
            );
        }
    }
}

/// Losing decryption or its error classification blocks provider keys and passphrase retries.
#[test]
fn encrypted_provider_rsa_formats_require_the_right_passphrase() {
    let expected = russh::keys::PublicKey::from_openssh(include_str!("fixtures/rsa.pub"))
        .expect("independently serialized public key");
    for file in [
        include_str!("fixtures/rsa-pkcs1-encrypted.pem"),
        include_str!("fixtures/rsa-pkcs1-aes128.pem"),
        include_str!("fixtures/rsa-openssh-encrypted.pem"),
        include_str!("fixtures/rsa-putty-encrypted.ppk"),
        include_str!("fixtures/rsa-pkcs8-encrypted.pem"),
    ] {
        assert_eq!(parse(file, None), Err(KeyError::NeedsPassphrase));
        assert_eq!(parse(file, Some("wrong")), Err(KeyError::WrongPassphrase));
        let key = parse(file, Some("fixture-passphrase")).expect("encrypted provider RSA key");
        assert_eq!(key.public_key().key_data(), expected.key_data());
        let signature = key
            .sign("provider-login", russh::keys::HashAlg::Sha512, b"challenge")
            .expect("decrypted RSA key signs");
        expected
            .verify("provider-login", b"challenge", &signature)
            .expect("decrypted key signature verifies");
    }
}

/// Replacing the supported-type guard with an unrestricted parser must not enable DSA login.
#[test]
fn names_unsupported_dsa_keys() {
    let pem = include_str!("fixtures/dsa.pem");
    assert_eq!(parse(pem, None), Err(KeyError::Unsupported("DSA".into())));
}

#[test]
fn asks_for_the_passphrase_of_an_encrypted_putty_or_pkcs8_file() {
    let ppk = "PuTTY-User-Key-File-3: ssh-ed25519\nEncryption: aes256-cbc\nComment: x\n";
    assert_eq!(parse(ppk, None), Err(KeyError::NeedsPassphrase));
    let pkcs8 =
        "-----BEGIN ENCRYPTED PRIVATE KEY-----\nMIGb\n-----END ENCRYPTED PRIVATE KEY-----\n";
    assert_eq!(parse(pkcs8, None), Err(KeyError::NeedsPassphrase));
    assert_eq!(parse(pkcs8, Some("wrong")), Err(KeyError::WrongPassphrase));
}

#[test]
fn a_file_that_is_no_key_is_unreadable_with_or_without_a_passphrase() {
    assert!(matches!(parse("hello", None), Err(KeyError::Unreadable(_))));
    assert!(matches!(
        parse("hello", Some("x")),
        Err(KeyError::Unreadable(_))
    ));
}
