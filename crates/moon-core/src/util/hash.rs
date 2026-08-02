//! Small non-cryptographic digests for in-process change detection.

/// FNV-1a offset basis.
const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;

/// FNV-1a prime.
const FNV_PRIME: u64 = 0x100_0000_01b3;

/// Digest bytes with FNV-1a.
///
/// For comparing a value against its own previous form inside one process — a change signature, a
/// cache key — never for anything persisted or exposed. `DefaultHasher` would do the same job but
/// carries no stability guarantee across releases, and this stays allocation-free and readable at
/// the call site.
///
/// Args:
///     bytes: Input to digest.
///
/// Returns:
///     64-bit digest.
pub fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = FNV_OFFSET;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}
