//! This machine's wrapping key, held by the OS keyring, and the slot id derived from it.
//!
//! One behavioural change from the pre-slot implementation matters more than the rest: reading no
//! longer CREATES a key. The old `data_key` generated and stored a fresh random key whenever the
//! keyring had no entry, including when it was only being asked to open an existing file — so
//! carrying `servers.enc` to a clean machine silently minted a key that could never open it, and
//! left that key behind in the keyring. Creation now happens only where a key is actually being
//! established: writing a new file, or adding this machine to one that was just unlocked.

use anyhow::{anyhow, Context};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

/// Keyring service name.
const KEYRING_SERVICE: &str = "moon-terminal";
/// Keyring entry name. Unchanged from the pre-slot format on purpose: the key it holds is reused
/// as this machine's wrapping key, so existing installations keep opening their files with no
/// migration step and no second prompt.
const KEYRING_USER: &str = "config-key-v1";

/// Domain separator for the slot id, so the published id cannot be confused with any other value
/// derived from the same key.
const SLOT_ID_DOMAIN: &[u8] = b"moonterminal-slot-id";

/// Length of a slot id in bytes before base64.
const SLOT_ID_LEN: usize = 16;

/// Read this machine's wrapping key, without creating one.
///
/// # Returns
///
/// `Ok(None)` when the keyring simply has no entry, which is the normal state of a machine that
/// has never run the terminal and of one that received a config file from elsewhere.
///
/// # Errors
///
/// Returns an error when the keyring is present but unusable, or holds a malformed value. That is
/// deliberately NOT folded into `None`: "no key here" and "the key store is broken" lead to
/// different outcomes, and treating a locked keyring as absent would offer to start over with an
/// empty config while the real key was still sitting there.
pub(super) fn key() -> anyhow::Result<Option<Zeroizing<[u8; 32]>>> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER).context("keyring entry")?;
    match entry.get_password() {
        Ok(encoded) => {
            let bytes = B64.decode(encoded).context("decode keyring key")?;
            let key: [u8; 32] = bytes
                .try_into()
                .map_err(|_| anyhow!("keyring key has wrong length"))?;
            Ok(Some(Zeroizing::new(key)))
        }
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(anyhow!("keyring get: {e}")),
    }
}

/// Read this machine's wrapping key, generating and storing one when the keyring has no entry.
///
/// Call this only when a key is genuinely being established. See the module note.
pub(super) fn key_or_create() -> anyhow::Result<Zeroizing<[u8; 32]>> {
    if let Some(existing) = key()? {
        return Ok(existing);
    }
    let mut fresh = Zeroizing::new([0u8; 32]);
    getrandom::getrandom(fresh.as_mut()).map_err(|e| anyhow!("getrandom key: {e}"))?;
    keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER)
        .context("keyring entry")?
        .set_password(&B64.encode(fresh.as_ref()))
        .context("store keyring key")?;
    log::info!("сгенерирован новый ключ шифрования конфига (сохранён в OS keyring)");
    Ok(fresh)
}

/// Identify a machine slot from its wrapping key.
///
/// A one-way hash of a 256-bit random key, so publishing it in the plaintext header reveals
/// nothing about the key. Storing the id avoids a second keyring entry and lets a reader pick its
/// own slot directly instead of trial-decrypting every machine slot in the file.
pub(super) fn slot_id(key: &[u8; 32]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(SLOT_ID_DOMAIN);
    hasher.update(key);
    B64.encode(&hasher.finalize()[..SLOT_ID_LEN])
}

#[cfg(test)]
mod tests;
