//! Byte layout of the encrypted config file and its one-way migration from the legacy format.
//!
//! ```text
//! v1:     "MOONENC1" | header_len: u32 LE | header: TOML | nonce: 12 | ciphertext+tag
//! legacy:                                                  nonce: 12 | ciphertext+tag
//! ```
//!
//! The header is PLAINTEXT — it has to be, since it is what tells us which key can open the file —
//! but it is not unauthenticated: its exact bytes are fed to AES-GCM as additional authenticated
//! data, so editing the header invalidates the payload rather than silently changing which slots
//! the reader believes in.
//!
//! A legacy file is identified by the absence of the magic, not by a version field it never had.
//! Those files are still produced by every build before key slots existed, and they are still
//! sitting in `backups/settings/`, so the reader keeps understanding them permanently.

use anyhow::{anyhow, Context};

use super::slots::Header;

/// File magic. Chosen at 8 bytes so it cannot be confused with a legacy file's 12-byte random
/// nonce by anything short of a deliberate collision.
const MAGIC: [u8; 8] = *b"MOONENC1";

/// AES-GCM nonce length in bytes.
pub(super) const NONCE_LEN: usize = 12;

/// Largest header accepted while reading.
///
/// The length prefix comes from the file, so a damaged or hostile one would otherwise ask for an
/// arbitrary allocation before anything has been authenticated. Eight slots of a few hundred bytes
/// do not approach this.
const MAX_HEADER_LEN: usize = 64 * 1024;

/// A parsed encrypted file, borrowed from the caller's buffer.
///
/// `Debug` is safe to derive: every field is either public metadata or already-encrypted bytes.
#[derive(Debug)]
pub(super) enum Parsed<'a> {
    /// Pre-slot format: one payload encrypted directly with the OS keyring key.
    Legacy {
        nonce: &'a [u8],
        ciphertext: &'a [u8],
    },
    /// Current format.
    V1 {
        header: Header,
        /// Exact header bytes, which are the AES-GCM additional authenticated data.
        header_bytes: &'a [u8],
        nonce: &'a [u8],
        ciphertext: &'a [u8],
    },
}

/// Split `data` into its parts without decrypting anything.
pub(super) fn parse(data: &[u8]) -> anyhow::Result<Parsed<'_>> {
    if !data.starts_with(&MAGIC) {
        // Legacy: the whole file is nonce plus ciphertext.
        if data.len() < NONCE_LEN {
            return Err(anyhow!("config file is too short to contain a nonce"));
        }
        let (nonce, ciphertext) = data.split_at(NONCE_LEN);
        return Ok(Parsed::Legacy { nonce, ciphertext });
    }

    let rest = &data[MAGIC.len()..];
    let (len_bytes, rest) = rest
        .split_at_checked(4)
        .ok_or_else(|| anyhow!("config file ends inside its header length"))?;
    let header_len = u32::from_le_bytes(
        len_bytes
            .try_into()
            .expect("split_at_checked returned exactly four bytes"),
    ) as usize;
    if header_len > MAX_HEADER_LEN {
        return Err(anyhow!(
            "config header claims {header_len} bytes, above the {MAX_HEADER_LEN} limit"
        ));
    }
    let (header_bytes, rest) = rest
        .split_at_checked(header_len)
        .ok_or_else(|| anyhow!("config file ends inside its header"))?;
    let (nonce, ciphertext) = rest
        .split_at_checked(NONCE_LEN)
        .ok_or_else(|| anyhow!("config file ends inside its nonce"))?;

    let header: Header = toml::from_str(
        std::str::from_utf8(header_bytes).context("config header is not valid UTF-8")?,
    )
    .context("config header is not valid TOML")?;
    if header.version != Header::VERSION {
        return Err(anyhow!(
            "config header version {} is newer than this build understands ({})",
            header.version,
            Header::VERSION
        ));
    }
    Ok(Parsed::V1 {
        header,
        header_bytes,
        nonce,
        ciphertext,
    })
}

/// Serialize a header to the exact bytes that will be stored and authenticated.
///
/// Returned separately from [`assemble`] because the caller must encrypt with these bytes as the
/// additional authenticated data BEFORE the file can be built — the payload commits to the header,
/// so the header cannot be re-serialized afterwards.
pub(super) fn header_bytes(header: &Header) -> anyhow::Result<Vec<u8>> {
    let text = toml::to_string(header).context("serialize config header")?;
    let bytes = text.into_bytes();
    if bytes.len() > MAX_HEADER_LEN {
        return Err(anyhow!(
            "config header grew to {} bytes, above the {MAX_HEADER_LEN} limit",
            bytes.len()
        ));
    }
    Ok(bytes)
}

/// Build the complete file from an already-serialized header and an encrypted payload.
pub(super) fn assemble(header_bytes: &[u8], nonce: &[u8], ciphertext: &[u8]) -> Vec<u8> {
    let mut out =
        Vec::with_capacity(MAGIC.len() + 4 + header_bytes.len() + nonce.len() + ciphertext.len());
    out.extend_from_slice(&MAGIC);
    out.extend_from_slice(&(header_bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(header_bytes);
    out.extend_from_slice(nonce);
    out.extend_from_slice(ciphertext);
    out
}

#[cfg(test)]
mod tests;
