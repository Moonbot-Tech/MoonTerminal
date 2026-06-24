//! Шифрование конфига: AES-256-GCM, ключ хранится в OS keyring.
//! Формат файла: [nonce(12)] ++ [ciphertext+tag].

use aes_gcm::aead::Aead;
use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
use anyhow::{anyhow, Context};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;

const KEYRING_SERVICE: &str = "moon-terminal";
const KEYRING_USER: &str = "config-key-v1";
const NONCE_LEN: usize = 12;

/// Достаёт 32-байтовый ключ из OS keyring; при первом запуске генерирует и сохраняет.
fn data_key() -> anyhow::Result<[u8; 32]> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER).context("keyring entry")?;
    match entry.get_password() {
        Ok(b64) => {
            let bytes = B64.decode(b64).context("decode keyring key")?;
            let arr: [u8; 32] = bytes
                .try_into()
                .map_err(|_| anyhow!("keyring key has wrong length"))?;
            Ok(arr)
        }
        Err(keyring::Error::NoEntry) => {
            let mut key = [0u8; 32];
            getrandom::getrandom(&mut key).map_err(|e| anyhow!("getrandom key: {e}"))?;
            entry
                .set_password(&B64.encode(key))
                .context("store keyring key")?;
            log::info!("сгенерирован новый ключ шифрования конфига (сохранён в OS keyring)");
            Ok(key)
        }
        Err(e) => Err(anyhow!("keyring get: {e}")),
    }
}

pub fn encrypt(plain: &[u8]) -> anyhow::Result<Vec<u8>> {
    let key = data_key()?;
    let cipher = Aes256Gcm::new_from_slice(&key).map_err(|_| anyhow!("bad key length"))?;
    let mut nonce = [0u8; NONCE_LEN];
    getrandom::getrandom(&mut nonce).map_err(|e| anyhow!("getrandom nonce: {e}"))?;
    let ct = cipher
        .encrypt(Nonce::from_slice(&nonce), plain)
        .map_err(|_| anyhow!("encrypt failed"))?;
    let mut out = Vec::with_capacity(NONCE_LEN + ct.len());
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ct);
    Ok(out)
}

pub fn decrypt(data: &[u8]) -> anyhow::Result<Vec<u8>> {
    if data.len() < NONCE_LEN {
        return Err(anyhow!("config too short"));
    }
    let (nonce, ct) = data.split_at(NONCE_LEN);
    let key = data_key()?;
    let cipher = Aes256Gcm::new_from_slice(&key).map_err(|_| anyhow!("bad key length"))?;
    cipher
        .decrypt(Nonce::from_slice(nonce), ct)
        .map_err(|_| anyhow!("decrypt failed (неверный ключ или повреждён файл)"))
}
