//! The provider's RSA login must select modern signatures without changing the app key.

use super::{HashAlg, signing_key};
use russh::keys::Algorithm;

/// Passing `None` to the RSA signer selects SHA-1 and makes modern sshd refuse provider login.
#[test]
fn rsa_authentication_offers_sha2_even_without_server_extensions() {
    let key = crate::keys::parse(include_str!("../keys/fixtures/rsa-pkcs1.pem"), None)
        .expect("synthetic RSA key");
    for (advertised, expected) in [
        (Some(Some(HashAlg::Sha512)), HashAlg::Sha512),
        (Some(Some(HashAlg::Sha256)), HashAlg::Sha256),
        (None, HashAlg::Sha256),
        (Some(None), HashAlg::Sha256),
    ] {
        assert_eq!(
            signing_key(&key, advertised).algorithm(),
            Algorithm::Rsa {
                hash: Some(expected)
            }
        );
    }
}

/// RSA negotiation must not change the terminal's ed25519 app-key authentication algorithm.
#[test]
fn rsa_hash_negotiation_keeps_ed25519_unchanged() {
    use russh::keys::ssh_key::private::{Ed25519Keypair, KeypairData};
    let key = russh::keys::PrivateKey::new(
        KeypairData::from(Ed25519Keypair::from_seed(&[17u8; 32])),
        "synthetic app key",
    )
    .expect("ed25519 fixture");
    assert_eq!(
        signing_key(&key, Some(Some(HashAlg::Sha512))).algorithm(),
        Algorithm::Ed25519
    );
}
