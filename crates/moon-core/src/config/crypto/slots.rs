//! Key slots: the plaintext header of `servers.enc` describing every way its payload key can be
//! unwrapped.
//!
//! The payload is encrypted once with a random file key. That file key is then wrapped separately
//! by each slot, which is what makes the two things the user asked for possible at all:
//! - a copy of the file opens silently on every machine that has unwrapped it once, because each
//!   machine adds its OWN slot instead of replacing a single shared one. Without that, a file in a
//!   synchronized folder would ping-pong: each machine would overwrite the other's slot and lock it
//!   out on the next launch;
//! - the encryption password can be set, changed, or removed by rewriting ~200 bytes of header,
//!   with the payload untouched. Re-encrypting the payload under a password-derived key instead
//!   would make every existing backup unreadable the moment the password changed.
//!
//! Forward compatibility is deliberate: a slot is a struct with optional fields and a free-text
//! `kind`, NOT a serde enum. An older build must SKIP a slot type it does not understand and keep
//! using the ones it does — with an enum, one unknown `kind` makes the whole header fail to parse
//! and the file unreadable by the very build the user downgraded to in order to open it.

use serde::{Deserialize, Serialize};

/// Slot kind that wraps the file key with a key held by the OS keyring of one machine.
pub(super) const KIND_MACHINE: &str = "machine";
/// Slot kind that wraps the file key with a key derived from the user's password.
pub(super) const KIND_PASSWORD: &str = "password";

/// Largest number of machine slots kept in one file.
///
/// Machines are replaced, reinstalled, and reimaged, and every one of those events would otherwise
/// leave a slot behind forever. The oldest is evicted, so the file cannot grow without bound and a
/// long-dead machine does not keep the ability to open it.
pub(super) const MAX_MACHINE_SLOTS: usize = 8;

/// The complete plaintext header of an encrypted config file.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct Header {
    /// Format version of the header itself. Unknown values are rejected by the reader.
    pub version: u32,
    /// Launch-password verifier, or `None` when the launch gate is off.
    ///
    /// Lives beside the slots rather than inside the encrypted payload because it must be checked
    /// BEFORE any key is available. Editing it out is not a bypass: the header is the payload's
    /// additional authenticated data, so a header the user has edited no longer opens the file.
    /// See `crypto/launch.rs` for what this latch does and does not protect.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launch: Option<String>,
    /// Key slots, serialized as a TOML array of tables named `slot`.
    ///
    /// Declared LAST because TOML puts every array-of-tables after the plain values of the table
    /// containing it: a scalar field declared after this one would serialize into the final slot
    /// instead of the header, and `launch` is exactly such a field.
    #[serde(default, rename = "slot")]
    pub slots: Vec<Slot>,
}

impl Header {
    /// Current header version.
    pub const VERSION: u32 = 1;

    /// Build an empty header of the current version.
    pub fn new() -> Self {
        Self {
            version: Self::VERSION,
            launch: None,
            slots: Vec::new(),
        }
    }

    /// Whether any slot can be opened with a password.
    pub fn has_password_slot(&self) -> bool {
        self.slots.iter().any(|slot| slot.kind == KIND_PASSWORD)
    }

    /// Number of machine slots present.
    pub fn machine_slot_count(&self) -> usize {
        self.slots
            .iter()
            .filter(|slot| slot.kind == KIND_MACHINE)
            .count()
    }

    /// Install `slot` as this machine's slot, replacing the previous one with the same id.
    ///
    /// Replacing by id rather than appending keeps a machine that re-wraps its own slot (a rotated
    /// keyring entry, a repeated unlock) from consuming the whole budget by itself.
    pub fn put_machine_slot(&mut self, slot: Slot) {
        let id = slot.id.clone();
        self.slots
            .retain(|existing| existing.kind != KIND_MACHINE || existing.id != id);
        self.slots.push(slot);
        self.evict_oldest_machine_slots(id.as_deref());
    }

    /// Replace the password slot, or remove it when `slot` is `None`.
    pub fn set_password_slot(&mut self, slot: Option<Slot>) {
        self.slots.retain(|existing| existing.kind != KIND_PASSWORD);
        if let Some(slot) = slot {
            self.slots.push(slot);
        }
    }

    /// Drop every machine slot except the one identified by `keep_id`.
    ///
    /// The password slot is deliberately untouched: it is the user's way back in, and a "revoke
    /// the other machines" action that silently removed it would turn a security gesture into a
    /// lockout on the next machine.
    pub fn retain_only_machine(&mut self, keep_id: &str) {
        self.slots
            .retain(|slot| slot.kind != KIND_MACHINE || slot.id.as_deref() == Some(keep_id));
    }

    /// Enforce [`MAX_MACHINE_SLOTS`] by dropping the oldest machine slots first.
    ///
    /// `protect_id` is never evicted. The timestamps come from each machine's own clock, so a
    /// machine whose clock is behind writes the "oldest" slot in the file and would otherwise
    /// evict the slot it just created — silently never being remembered, and asking for the
    /// password again on every launch with no error to explain why.
    fn evict_oldest_machine_slots(&mut self, protect_id: Option<&str>) {
        let excess = self.machine_slot_count().saturating_sub(MAX_MACHINE_SLOTS);
        for _ in 0..excess {
            let Some(index) = self
                .slots
                .iter()
                .enumerate()
                .filter(|(_, slot)| slot.kind == KIND_MACHINE && slot.id.as_deref() != protect_id)
                .min_by_key(|(_, slot)| slot.created_ms)
                .map(|(index, _)| index)
            else {
                return;
            };
            self.slots.remove(index);
        }
    }
}

/// One way to unwrap the file key.
///
/// Every field except `kind`, `nonce`, and `wrapped` is optional so that a slot of an unknown kind
/// still round-trips through an older build instead of destroying the header.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct Slot {
    /// [`KIND_MACHINE`], [`KIND_PASSWORD`], or something a newer build introduced.
    pub kind: String,
    /// Machine slots: which machine this slot belongs to. See `machine::slot_id`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Base64 AES-GCM nonce used to wrap the file key.
    pub nonce: String,
    /// Base64 wrapped file key with its authentication tag.
    pub wrapped: String,
    /// Password slots: base64 KDF salt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub salt: Option<String>,
    /// Password slots: KDF name, so a future algorithm change does not silently reinterpret the
    /// parameters below.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kdf: Option<String>,
    /// Password slots: KDF memory cost in KiB.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub m_cost: Option<u32>,
    /// Password slots: KDF iteration count.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub t_cost: Option<u32>,
    /// Password slots: KDF parallelism.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub p_cost: Option<u32>,
    /// Creation time in Unix milliseconds, used only to choose an eviction victim.
    #[serde(default)]
    pub created_ms: i64,
    /// Every field this build does not know about, preserved verbatim.
    ///
    /// Without this, an older build that opens a file, changes one setting and saves would write
    /// back a newer build's slot STRIPPED of the fields that make it usable — leaving a slot that
    /// looks present and opens nothing. Serde drops unknown fields silently by default, so the
    /// loss would be invisible on the build that caused it.
    #[serde(flatten)]
    pub extra: toml::Table,
}

#[cfg(test)]
mod tests;
