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

#[test]
fn names_an_rsa_key_instead_of_failing_to_parse_it() {
    let pem = "-----BEGIN RSA PRIVATE KEY-----\nMIIEpAIBAAKCAQEA\n-----END RSA PRIVATE KEY-----\n";
    assert_eq!(parse(pem, None), Err(KeyError::Unsupported("RSA".into())));
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
