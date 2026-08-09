//! AES-256-GCM primitives: sealing the payload under the file key, and wrapping the file key
//! inside one slot.
//!
//! Two distinct uses of the same cipher live here so the difference stays visible:
//! - the PAYLOAD is sealed with the header bytes as additional authenticated data, so the header
//!   cannot be edited without invalidating the payload;
//! - a SLOT wraps 32 bytes of key material with no additional data, because the slot is a
//!   standalone envelope that must stay valid while the header around it changes.

use aes_gcm::aead::{Aead, Payload};
use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
use anyhow::{anyhow, Context};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use zeroize::Zeroizing;

use super::format::NONCE_LEN;
use super::kdf::{self, KdfParams};
use super::slots::{Slot, KIND_MACHINE, KIND_PASSWORD};
use crate::util::now_unix_ms_i64;

/// Build a cipher from a 32-byte key.
fn cipher(key: &[u8; 32]) -> anyhow::Result<Aes256Gcm> {
    Aes256Gcm::new_from_slice(key).map_err(|_| anyhow!("bad key length"))
}

/// Generate a fresh random nonce.
fn nonce() -> anyhow::Result<[u8; NONCE_LEN]> {
    let mut nonce = [0u8; NONCE_LEN];
    getrandom::getrandom(&mut nonce).map_err(|e| anyhow!("getrandom nonce: {e}"))?;
    Ok(nonce)
}

/// Generate a fresh random file key.
pub(super) fn new_file_key() -> anyhow::Result<Zeroizing<[u8; 32]>> {
    let mut key = Zeroizing::new([0u8; 32]);
    getrandom::getrandom(key.as_mut()).map_err(|e| anyhow!("getrandom file key: {e}"))?;
    Ok(key)
}

/// Seal `plain` under `key`, binding it to `aad`.
///
/// Returns the nonce and the ciphertext with its tag. A fresh random nonce per write is what makes
/// reusing one file key across every save safe.
pub(super) fn seal(
    key: &[u8; 32],
    plain: &[u8],
    aad: &[u8],
) -> anyhow::Result<([u8; NONCE_LEN], Vec<u8>)> {
    let nonce = nonce()?;
    let ciphertext = cipher(key)?
        .encrypt(Nonce::from_slice(&nonce), Payload { msg: plain, aad })
        .map_err(|_| anyhow!("encrypt failed"))?;
    Ok((nonce, ciphertext))
}

/// Open a payload sealed by [`seal`].
pub(super) fn open(
    key: &[u8; 32],
    nonce: &[u8],
    ciphertext: &[u8],
    aad: &[u8],
) -> anyhow::Result<Vec<u8>> {
    cipher(key)?
        .decrypt(
            Nonce::from_slice(nonce),
            Payload {
                msg: ciphertext,
                aad,
            },
        )
        .map_err(|_| anyhow!("decrypt failed (wrong key or damaged file)"))
}

/// Wrap `file_key` for this machine.
pub(super) fn machine_slot(
    file_key: &[u8; 32],
    machine_key: &[u8; 32],
    slot_id: String,
) -> anyhow::Result<Slot> {
    let (nonce, wrapped) = seal(machine_key, file_key.as_slice(), &[])?;
    Ok(Slot {
        kind: KIND_MACHINE.to_string(),
        id: Some(slot_id),
        nonce: B64.encode(nonce),
        wrapped: B64.encode(wrapped),
        salt: None,
        kdf: None,
        m_cost: None,
        t_cost: None,
        p_cost: None,
        created_ms: now_unix_ms_i64(),
        extra: Default::default(),
    })
}

/// Wrap `file_key` under a key derived from `password`.
pub(super) fn password_slot(file_key: &[u8; 32], password: &str) -> anyhow::Result<Slot> {
    let mut salt = [0u8; kdf::SALT_LEN];
    getrandom::getrandom(&mut salt).map_err(|e| anyhow!("getrandom salt: {e}"))?;
    let params = KdfParams::current();
    let wrapping = kdf::derive(password, &salt, params)?;
    let (nonce, wrapped) = seal(&wrapping, file_key.as_slice(), &[])?;
    Ok(Slot {
        kind: KIND_PASSWORD.to_string(),
        id: None,
        nonce: B64.encode(nonce),
        wrapped: B64.encode(wrapped),
        salt: Some(B64.encode(salt)),
        kdf: Some(kdf::ALGORITHM.to_string()),
        m_cost: Some(params.m_cost),
        t_cost: Some(params.t_cost),
        p_cost: Some(params.p_cost),
        created_ms: now_unix_ms_i64(),
        extra: Default::default(),
    })
}

/// Unwrap the file key from a machine slot.
pub(super) fn unwrap_machine(
    slot: &Slot,
    machine_key: &[u8; 32],
) -> anyhow::Result<Zeroizing<[u8; 32]>> {
    unwrap_with(slot, machine_key)
}

/// Unwrap the file key from a password slot.
///
/// The KDF name is checked rather than assumed: a future slot written by a newer build with a
/// different algorithm carries the same parameter fields, and deriving Argon2id from them would
/// produce a wrong key and report it to the user as a wrong password.
pub(super) fn unwrap_password(slot: &Slot, password: &str) -> anyhow::Result<Zeroizing<[u8; 32]>> {
    let algorithm = slot.kdf.as_deref().unwrap_or(kdf::ALGORITHM);
    if algorithm != kdf::ALGORITHM {
        return Err(anyhow!(
            "password slot uses unsupported key derivation '{algorithm}'"
        ));
    }
    let salt = B64
        .decode(slot.salt.as_deref().context("password slot has no salt")?)
        .context("decode password slot salt")?;
    let params = KdfParams::from_slot(
        slot.m_cost.context("password slot has no m_cost")?,
        slot.t_cost.context("password slot has no t_cost")?,
        slot.p_cost.context("password slot has no p_cost")?,
    )?;
    let wrapping = kdf::derive(password, &salt, params)?;
    unwrap_with(slot, &wrapping)
}

/// Shared unwrap step once the wrapping key is known.
fn unwrap_with(slot: &Slot, wrapping: &[u8; 32]) -> anyhow::Result<Zeroizing<[u8; 32]>> {
    let nonce = B64.decode(&slot.nonce).context("decode slot nonce")?;
    let wrapped = B64.decode(&slot.wrapped).context("decode wrapped key")?;
    // The cipher hands back a plain `Vec`, which is the file key in the clear. Wrap it before
    // anything else can happen to it: copying into the array below leaves the original buffer in
    // the allocator, and without this it is never wiped.
    let unwrapped = Zeroizing::new(open(wrapping, &nonce, &wrapped, &[])?);
    let key: [u8; 32] = unwrapped
        .as_slice()
        .try_into()
        .map_err(|_| anyhow!("slot holds a key of the wrong length"))?;
    Ok(Zeroizing::new(key))
}
