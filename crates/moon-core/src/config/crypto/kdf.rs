//! Argon2id key derivation for the password slot.
//!
//! The threat this answers is offline: whoever holds a copy of `servers.enc` can try passwords as
//! fast as their hardware allows, with no rate limit we could impose. A memory-hard KDF is what
//! makes each attempt cost real RAM and time instead of a hash.
//!
//! Parameters are stored in the slot rather than assumed, so raising them later leaves existing
//! files readable. They are also CLAMPED on the way in: the numbers come from a file that may be
//! damaged or hostile, and `m_cost` is an allocation request — an unchecked one turns opening a
//! config into an out-of-memory abort.

use anyhow::{anyhow, Context};
use argon2::{Algorithm, Argon2, Params, Version};
use zeroize::Zeroizing;

/// KDF name written into the slot.
pub(super) const ALGORITHM: &str = "argon2id";

/// Memory cost in KiB used for new password slots (64 MiB).
///
/// Costs roughly 100-250 ms on a normal desktop, which is acceptable for something asked once per
/// machine rather than once per launch.
const NEW_M_COST: u32 = 64 * 1024;
/// Iteration count used for new password slots.
const NEW_T_COST: u32 = 3;
/// Parallelism used for new password slots.
const NEW_P_COST: u32 = 1;

/// Largest memory cost accepted from a file (1 GiB).
const MAX_M_COST: u32 = 1024 * 1024;
/// Largest iteration count accepted from a file.
const MAX_T_COST: u32 = 16;
/// Largest parallelism accepted from a file.
const MAX_P_COST: u32 = 8;

/// Salt length in bytes for new password slots.
pub(super) const SALT_LEN: usize = 16;

/// Cost parameters of one password slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct KdfParams {
    pub m_cost: u32,
    pub t_cost: u32,
    pub p_cost: u32,
}

impl KdfParams {
    /// Parameters for a newly created password slot.
    pub fn current() -> Self {
        Self {
            m_cost: NEW_M_COST,
            t_cost: NEW_T_COST,
            p_cost: NEW_P_COST,
        }
    }

    /// Accept parameters read from a slot, refusing values that cannot be honoured safely.
    ///
    /// Refusing is deliberate rather than clamping down to something workable: a file asking for
    /// costs we will not pay cannot be opened correctly by lowering them, so pretending otherwise
    /// would only produce a wrong key and an "invalid password" message for a correct password.
    pub fn from_slot(m_cost: u32, t_cost: u32, p_cost: u32) -> anyhow::Result<Self> {
        if m_cost > MAX_M_COST || t_cost > MAX_T_COST || p_cost > MAX_P_COST {
            return Err(anyhow!(
                "password slot demands unsupported KDF cost (m={m_cost}, t={t_cost}, p={p_cost})"
            ));
        }
        Ok(Self {
            m_cost,
            t_cost,
            p_cost,
        })
    }
}

/// Derive a 32-byte wrapping key from `password` and `salt`.
///
/// The result is wrapped in [`Zeroizing`] so it is wiped when dropped; the password itself belongs
/// to the caller, which holds it for the shortest possible time.
///
/// # Errors
///
/// Returns an error for parameters Argon2 rejects and for derivation failures.
pub(super) fn derive(
    password: &str,
    salt: &[u8],
    params: KdfParams,
) -> anyhow::Result<Zeroizing<[u8; 32]>> {
    let argon_params = Params::new(params.m_cost, params.t_cost, params.p_cost, Some(32))
        .map_err(|e| anyhow!("argon2 parameters rejected: {e}"))?;
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, argon_params);
    let mut key = Zeroizing::new([0u8; 32]);
    argon
        .hash_password_into(password.as_bytes(), salt, key.as_mut())
        .map_err(|e| anyhow!("argon2 derivation failed: {e}"))
        .context("derive key from encryption password")?;
    Ok(key)
}

#[cfg(test)]
mod tests;
