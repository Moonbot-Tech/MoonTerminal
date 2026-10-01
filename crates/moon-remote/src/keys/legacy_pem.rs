//! Decode AES-192-CBC and AES-256-CBC envelopes of legacy PKCS#1 RSA PEM files.
//!
//! russh handles plain and AES-128-CBC PKCS#1 itself, but treats the larger AES variants' ciphertext
//! as plain DER. Keep this format adapter local to provider key import.

use aes::cipher::{BlockModeDecrypt, KeyIvInit, block_padding::Pkcs7};
use data_encoding::BASE64;
use russh::keys::PrivateKey;
use zeroize::Zeroizing;

use super::KeyError;

/// Decrypt OpenSSL's traditional PEM envelope, then let russh validate the RSA key.
pub(super) fn parse(text: &str, passphrase: Option<&str>) -> Result<PrivateKey, KeyError> {
    let passphrase = passphrase.ok_or(KeyError::NeedsPassphrase)?;
    let (cipher, iv_hex) = text
        .lines()
        .find_map(|line| line.strip_prefix("DEK-Info: "))
        .and_then(|header| header.split_once(','))
        .map(|(cipher, iv)| (cipher, iv.trim()))
        .filter(|(_, iv)| iv.len() == 32 && iv.is_ascii())
        .ok_or_else(|| KeyError::Unreadable("invalid PEM cipher IV".into()))?;
    let mut iv = [0u8; 16];
    for (index, byte) in iv.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&iv_hex[index * 2..index * 2 + 2], 16)
            .map_err(|_| KeyError::Unreadable("invalid PEM cipher IV".into()))?;
    }
    let body: String = text
        .lines()
        .filter(|line| !line.is_empty() && !line.starts_with("-----") && !line.contains(':'))
        .collect();
    let mut ciphertext = Zeroizing::new(
        BASE64
            .decode(body.as_bytes())
            .map_err(|error| KeyError::Unreadable(error.to_string()))?,
    );
    // EVP_BytesToKey uses MD5(previous digest || password || IV's first eight bytes).
    // This is the file format's KDF, never a signature or a new key-encryption format.
    let mut key = Zeroizing::new([0u8; 32]);
    let mut previous = Zeroizing::new([0u8; 16]);
    for round in 0..2 {
        let mut digest = md5::Context::new();
        if round != 0 {
            digest.consume(*previous);
        }
        digest.consume(passphrase.as_bytes());
        digest.consume(&iv[..8]);
        *previous = digest.finalize().0;
        key[round * 16..round * 16 + 16].copy_from_slice(&*previous);
    }
    let decrypted = match cipher {
        "AES-192-CBC" => cbc::Decryptor::<aes::Aes192>::new_from_slices(&key[..24], &iv)
            .map_err(|_| KeyError::Unreadable("invalid PEM cipher parameters".into()))?
            .decrypt_padded::<Pkcs7>(&mut ciphertext),
        "AES-256-CBC" => cbc::Decryptor::<aes::Aes256>::new_from_slices(&*key, &iv)
            .map_err(|_| KeyError::Unreadable("invalid PEM cipher parameters".into()))?
            .decrypt_padded::<Pkcs7>(&mut ciphertext),
        _ => return Err(KeyError::Unsupported(cipher.into())),
    }
    .map_err(|_| KeyError::WrongPassphrase)?;
    let body = Zeroizing::new(BASE64.encode(decrypted));
    let pem = Zeroizing::new(format!(
        "-----BEGIN RSA PRIVATE KEY-----\n{}\n-----END RSA PRIVATE KEY-----\n",
        *body
    ));
    russh::keys::decode_secret_key(&pem, None).map_err(|_| KeyError::WrongPassphrase)
}
