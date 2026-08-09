//! Boundary between the Settings security UI and the `servers.enc` key backend.
//!
//! Everything here forwards to `moon_core::config::crypto`, which owns the file key and its slots.
//! The indirection earns its place by keeping the UI free of the backend's shapes: the panel deals
//! in "set this password / remove that one", while the backend deals in wrapped keys and slots.
//!
//! What lands on disk and when: setting a password only re-wraps the file key IN MEMORY. The
//! config save that follows is what writes `servers.enc`, which is why `apply_security` runs
//! before it rather than after.

use moon_core::config::crypto;

/// Largest number of machine key slots one `servers.enc` may carry.
pub(super) const MAX_MACHINE_SLOTS: usize = crypto::MAX_MACHINE_SLOTS;

/// What the current `servers.enc` says about access, as far as the UI needs to know.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) struct VaultState {
    /// A launch password is stored and will be asked for at startup.
    pub launch_password_set: bool,
    /// A password key slot exists, so the file can be opened on a machine with no keyring entry.
    pub file_password_set: bool,
    /// Machine key slots currently in the file, including this machine's.
    pub machine_slots: usize,
    /// The config file was opened this run, so the passwords can be edited at all.
    ///
    /// False in `MOON_CONFIG_PLAINTEXT` mode and after a failed decryption; the panel disables
    /// itself rather than collecting a password nothing can store.
    pub editable: bool,
}

/// Read the current access state of `servers.enc`.
pub(super) fn state() -> VaultState {
    let access = crypto::access_state();
    VaultState {
        launch_password_set: crypto::launch_password_is_set(),
        file_password_set: access.password_set,
        machine_slots: access.machine_slots,
        editable: access.opened,
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
    /// The config file was never opened, so there is nothing to attach a password to.
    NotEditable,
    /// The key backend refused, e.g. the OS keyring is unavailable.
    Backend,
}

impl VaultError {
    /// Localization key describing this failure to the user.
    pub(super) fn message_key(self) -> &'static str {
        match self {
            Self::NotEditable => "security.err.not_editable",
            Self::Backend => "security.err.backend",
        }
    }
}

/// Apply a change to the stored access material.
///
/// Both passwords are applied together, and the FILE password is applied first: it is the one that
/// can fail on a key-derivation or keyring problem, and a failure after the launch password had
/// already been set would leave a half-applied pair.
///
/// Runs Argon2id when a file password is set — see the note in `crypto` about not doing that on a
/// UI thread once the operation grows a progress indicator. It is accepted here because it happens
/// on an explicit Save click, at most once per click.
pub(super) fn apply(change: VaultChange) -> Result<(), VaultError> {
    if let Some(request) = change.file_password {
        crypto::set_password(request.as_deref()).map_err(map_error)?;
    }
    if let Some(request) = change.launch_password {
        crypto::set_launch_password(request.as_deref()).map_err(map_error)?;
    }
    Ok(())
}

/// Drop every machine key slot except this machine's.
pub(super) fn forget_other_machines() -> Result<(), VaultError> {
    crypto::forget_other_machines().map_err(map_error)
}

/// Translate a backend failure into the two cases the panel can explain.
fn map_error(error: crypto::AccessError) -> VaultError {
    match error {
        crypto::AccessError::NoKey => VaultError::NotEditable,
        _ => VaultError::Backend,
    }
}
