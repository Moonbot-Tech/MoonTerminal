//! The provider's private key for the first login: read in any format a provider hands out.
//!
//! OpenSSH, PuTTY (`.ppk`) and PKCS#8 files carrying an ed25519, ECDSA or RSA key, plus
//! provider RSA keys in PKCS#1 PEM. Other key types are refused by name.

use anyhow::Context;
use russh::keys::PrivateKey;

mod legacy_pem;

/// Why a key file could not be used.
#[derive(Debug, PartialEq)]
pub enum KeyError {
    /// The file is encrypted and no passphrase was given: ask for one and read it again.
    NeedsPassphrase,
    /// The passphrase does not open the file.
    WrongPassphrase,
    /// A key type this build does not sign with, by its name.
    Unsupported(String),
    /// Not a private key this reader knows.
    Unreadable(String),
}

impl std::fmt::Display for KeyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NeedsPassphrase => f.write_str("the key is encrypted: its passphrase is needed"),
            Self::WrongPassphrase => f.write_str("the passphrase does not open the key"),
            Self::Unsupported(kind) => write!(f, "{kind} keys are not supported: use ed25519"),
            Self::Unreadable(why) => write!(f, "not a private key: {why}"),
        }
    }
}

impl std::error::Error for KeyError {}

/// Read a private key from the text of its file.
///
/// Args:
///     text: The whole key file.
///     passphrase: The passphrase of an encrypted file; `None` for a plain one.
///
/// Returns:
///     The decrypted key.
pub fn parse(text: &str, passphrase: Option<&str>) -> Result<PrivateKey, KeyError> {
    use russh::keys::Error;
    if text.contains("-----BEGIN DSA PRIVATE KEY-----") {
        return Err(KeyError::Unsupported("DSA".into()));
    }
    // PuTTY and PKCS#8 fail to parse, rather than report "encrypted", without their passphrase.
    if passphrase.is_none() && encrypted_container(text) {
        return Err(KeyError::NeedsPassphrase);
    }
    if text.contains("-----BEGIN RSA PRIVATE KEY-----") && text.contains("DEK-Info: AES-256-CBC,") {
        return legacy_pem::parse(text, passphrase);
    }
    // A plain PKCS#8 file must not be handed to russh's encrypted-PKCS#8 decoder.
    let password = if text.contains("-----BEGIN PRIVATE KEY-----") && !encrypted_container(text) {
        None
    } else {
        passphrase
    };
    match russh::keys::decode_secret_key(text, password) {
        Ok(key) => match key.algorithm() {
            russh::keys::Algorithm::Ed25519
            | russh::keys::Algorithm::Ecdsa { .. }
            | russh::keys::Algorithm::Rsa { .. } => Ok(key),
            kind => Err(KeyError::Unsupported(kind.to_string())),
        },
        Err(Error::KeyIsEncrypted) => Err(KeyError::NeedsPassphrase),
        Err(Error::UnsupportedKeyType {
            key_type_string, ..
        }) => Err(KeyError::Unsupported(key_type_string)),
        // A failure with a passphrase is the passphrase's only when the file IS encrypted; an
        // OpenSSH file says so when read without one.
        Err(e) if passphrase.is_some() => match encrypted_container(text)
            || matches!(
                russh::keys::decode_secret_key(text, None),
                Err(Error::KeyIsEncrypted)
            ) {
            true => Err(KeyError::WrongPassphrase),
            false => Err(KeyError::Unreadable(e.to_string())),
        },
        Err(e) => Err(KeyError::Unreadable(e.to_string())),
    }
}

/// The key's public half as an `authorized_keys` line.
pub fn authorized_line(key: &PrivateKey) -> anyhow::Result<String> {
    key.public_key()
        .to_openssh()
        .context("encode the public key")
}

/// A PuTTY file with a cipher, or an encrypted PKCS#8 / PKCS#5 PEM.
fn encrypted_container(text: &str) -> bool {
    let ppk_cipher = text.trim_start().starts_with("PuTTY-User-Key-File-")
        && text.lines().any(|l| {
            l.strip_prefix("Encryption:")
                .is_some_and(|v| v.trim() != "none")
        });
    ppk_cipher
        || text.contains("-----BEGIN ENCRYPTED PRIVATE KEY-----")
        || text.contains("Proc-Type: 4,ENCRYPTED")
}

#[cfg(test)]
mod tests;
