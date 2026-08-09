//! Boundary between the Settings security UI and the `servers.enc` key backend.
//!
//! **Every function here is a STUB.** The UI is being built before the crypto so the two can land
//! independently while `moon-core` is being edited elsewhere; phase 2 replaces these bodies with
//! calls into `moon_core::config::crypto` (key slots: one wrapped file key per machine, plus an
//! optional password-derived slot) without changing this module's shape or its callers.
//!
//! What phase 2 has to provide, and why each item is here:
//! - [`state`] — what the file currently carries, so the checkboxes can start in the right
//!   position instead of always unchecked;
//! - [`apply`] — the one write path, because setting a password re-wraps the file key rather than
//!   re-encrypting the payload, and both passwords must land in the same `AppConfig::save`;
//! - [`forget_other_machines`] — the recovery route when `servers.enc` has travelled and picked up
//!   slots for machines the user no longer controls.
//!
//! [`PREVIEW`] is the single flag the temporary UI notice keys off. Deleting it in phase 2 makes
//! the compiler point at every place the stub is still visible to the user.

/// Whether this build's security section is a non-functional preview.
///
/// Phase 2 deletes this constant along with the notice it drives.
pub(super) const PREVIEW: bool = true;

/// Largest number of machine key slots one `servers.enc` may carry.
///
/// Mirrors the planned backend cap: slots exist so a file synchronized between machines opens
/// silently on each of them, and an unbounded list would grow forever as machines are replaced.
pub(super) const MAX_MACHINE_SLOTS: usize = 8;

/// What the current `servers.enc` says about access, as far as the UI needs to know.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) struct VaultState {
    /// A launch password is stored and will be asked for at startup.
    pub launch_password_set: bool,
    /// A password key slot exists, so the file can be opened on a machine with no keyring entry.
    pub file_password_set: bool,
    /// Machine key slots currently in the file, including this machine's.
    pub machine_slots: usize,
}

/// Read the current access state of `servers.enc`.
///
/// STUB: reports a fresh single-machine file with no passwords, which is what a normal existing
/// installation looks like today.
pub(super) fn state() -> VaultState {
    VaultState {
        launch_password_set: false,
        file_password_set: false,
        machine_slots: 1,
    }
}

/// One requested change to the stored access material.
///
/// `None` means "leave as is"; `Some(None)` means "remove"; `Some(Some(password))` means "set to
/// this". The two passwords travel together because they are applied by one config save.
#[derive(Clone, Default)]
pub(super) struct VaultChange {
    pub launch_password: Option<Option<String>>,
    pub file_password: Option<Option<String>>,
}

/// Why a requested change could not be applied.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum VaultError {
    /// The backend is not implemented in this build.
    NotImplemented,
}

/// Apply a change to the stored access material.
///
/// STUB: always refuses, so nothing in this build can leave the user believing a password was
/// stored. Phase 2 derives the key on a worker thread — never inline — and writes `servers.enc`
/// through the existing atomic config-pair path.
pub(super) fn apply(change: VaultChange) -> Result<(), VaultError> {
    // Log the INTENT only. A password must never reach the log, which is written to disk and
    // pasted into bug reports; `describe` deliberately cannot see the value.
    log::info!(
        "security: change requested (launch: {}, servers.enc: {}) — key backend not implemented",
        describe(&change.launch_password),
        describe(&change.file_password)
    );
    Err(VaultError::NotImplemented)
}

/// Render one requested change as a word, never as its value.
fn describe(request: &Option<Option<String>>) -> &'static str {
    match request {
        None => "unchanged",
        Some(None) => "clear",
        Some(Some(_)) => "set",
    }
}

/// Drop every machine key slot except this machine's.
///
/// STUB: refuses for the same reason as [`apply`]. The real implementation keeps the local slot
/// and the password slot, so the user cannot lock themselves out by clicking it.
pub(super) fn forget_other_machines() -> Result<(), VaultError> {
    Err(VaultError::NotImplemented)
}
