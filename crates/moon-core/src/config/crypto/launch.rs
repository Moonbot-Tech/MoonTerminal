//! The launch-password verifier: a hash stored in the config header, checked at startup.
//!
//! # What this is and is not
//!
//! This is a LOCAL LATCH. It stops someone who sits down at an unlocked machine from opening the
//! terminal. It is explicitly allowed to be four characters, and it therefore provides no
//! resistance to anyone who can read the file: the core keys are wrapped by the machine key and
//! open without it, so bypassing the latch is a matter of reading `servers.enc` directly rather
//! than of guessing anything. Everything here is sized for that honest purpose.
//!
//! # Where it lives
//!
//! In the plaintext header, not in the encrypted payload. Both are equally tamper-proof — the
//! header is the payload's additional authenticated data, so editing it out invalidates the file —
//! and the header is readable before any key is available, which is exactly when the latch has to
//! be checked. The cost is that the hash itself is visible; against a four-character password that
//! changes nothing that was not already true of the paragraph above.
//!
//! # Format
//!
//! `argon2id$m=<KiB>,t=<n>,p=<n>$<salt-b64>$<key-b64>`, a deliberately small subset of the PHC
//! string layout. Parameters travel with the hash so they can be raised without invalidating
//! existing files.

use anyhow::{anyhow, Context};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;

use super::kdf::{self, KdfParams};

/// Memory cost for the launch verifier, in KiB.
///
/// A quarter of the file password's, because this one runs on EVERY launch while the file password
/// runs once per machine. 16 MiB lands around 40 ms, which keeps startup honest.
const LAUNCH_M_COST: u32 = 16 * 1024;
/// Iteration count for the launch verifier.
const LAUNCH_T_COST: u32 = 2;
/// Parallelism for the launch verifier.
const LAUNCH_P_COST: u32 = 1;

/// Salt length in bytes.
const SALT_LEN: usize = 16;

/// Hash `password` into a storable verifier string.
pub fn hash(password: &str) -> anyhow::Result<String> {
    let mut salt = [0u8; SALT_LEN];
    getrandom::getrandom(&mut salt).map_err(|e| anyhow!("getrandom salt: {e}"))?;
    let params = KdfParams {
        m_cost: LAUNCH_M_COST,
        t_cost: LAUNCH_T_COST,
        p_cost: LAUNCH_P_COST,
    };
    let key = kdf::derive(password, &salt, params)?;
    Ok(format!(
        "{}$m={},t={},p={}${}${}",
        kdf::ALGORITHM,
        params.m_cost,
        params.t_cost,
        params.p_cost,
        B64.encode(salt),
        B64.encode(key.as_ref())
    ))
}

/// Check `password` against a verifier produced by [`hash`].
///
/// Returns false for a malformed verifier rather than an error: the caller's only two options are
/// "let them in" and "do not", and a damaged verifier must resolve to the second without a way to
/// turn the parse failure itself into an entry path.
pub fn verify(verifier: &str, password: &str) -> bool {
    let Ok((params, salt, expected)) = parse(verifier) else {
        log::warn!("launch password verifier is malformed; refusing entry");
        return false;
    };
    let Ok(actual) = kdf::derive(password, &salt, params) else {
        return false;
    };
    constant_time_eq(actual.as_ref(), &expected)
}

/// Split a verifier string into its parts.
fn parse(verifier: &str) -> anyhow::Result<(KdfParams, Vec<u8>, Vec<u8>)> {
    let mut parts = verifier.split('$');
    let algorithm = parts.next().context("verifier has no algorithm")?;
    if algorithm != kdf::ALGORITHM {
        return Err(anyhow!("unsupported launch verifier algorithm"));
    }
    let costs = parts.next().context("verifier has no parameters")?;
    let salt = B64
        .decode(parts.next().context("verifier has no salt")?)
        .context("decode verifier salt")?;
    let expected = B64
        .decode(parts.next().context("verifier has no hash")?)
        .context("decode verifier hash")?;

    let mut m_cost = None;
    let mut t_cost = None;
    let mut p_cost = None;
    for field in costs.split(',') {
        let (name, value) = field.split_once('=').context("malformed cost parameter")?;
        let value: u32 = value.parse().context("non-numeric cost parameter")?;
        match name {
            "m" => m_cost = Some(value),
            "t" => t_cost = Some(value),
            "p" => p_cost = Some(value),
            _ => return Err(anyhow!("unknown cost parameter '{name}'")),
        }
    }
    let params = KdfParams::from_slot(
        m_cost.context("verifier has no m")?,
        t_cost.context("verifier has no t")?,
        p_cost.context("verifier has no p")?,
    )?;
    Ok((params, salt, expected))
}

/// Compare two byte strings without an early exit.
///
/// A `==` here leaks how many leading bytes matched through timing. That is a weak channel against
/// a local latch, but the correct comparison costs one line.
fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0u8, |acc, (a, b)| acc | (a ^ b))
        == 0
}

#[cfg(test)]
mod tests;
