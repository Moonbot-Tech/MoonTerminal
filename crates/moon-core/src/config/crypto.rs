//! Encryption of `servers.enc`: one payload key, wrapped by one slot per machine plus an optional
//! password slot. See `crypto/slots.rs` for why the file is built that way.
//!
//! # The invariant this module exists to hold
//!
//! The file key is generated ONCE and lives in this process for as long as it runs. Every save
//! re-seals the payload with the same key under a fresh nonce and copies the existing slots
//! verbatim. Regenerating it per write would look harmless — the file would still open on this
//! machine — and would silently destroy the password slot on the first autosave, because the
//! password needed to re-wrap a new key is not in memory. That is the failure the user would only
//! discover on the machine where the password was the last way in.
//!
//! # The other gate
//!
//! [`encrypt_config`] REFUSES to produce bytes when the file on disk could not be opened. The old
//! implementation did not need this: a decryption failure aborted startup before anything could
//! write. Once a login window lets the user continue past that failure, an ordinary settings save
//! would happily replace an unreadable-but-recoverable file with a fresh one — destroying the very
//! key slots the user was about to type their password into.

mod format;
mod kdf;
pub mod launch;
mod machine;
mod slots;
mod wrap;

use std::sync::{Mutex, MutexGuard};

use zeroize::Zeroizing;

use format::Parsed;
use slots::{Header, Slot, KIND_MACHINE, KIND_PASSWORD};

/// Why the encrypted config could not be opened.
#[derive(Debug)]
pub enum AccessError {
    /// The file carries a password slot and no key on this machine opens it. Ask the user.
    NeedsPassword,
    /// Nothing here can open the file and it has no password slot, so there is no way in.
    ///
    /// Also covers a DAMAGED legacy file: that format has no header to check, so "the key does not
    /// open it" and "the bytes are corrupt" are the same AEAD failure and cannot be told apart.
    /// The consequence binds the UI: whatever it offers here must MOVE the file aside rather than
    /// delete it, because this state includes files that are intact and merely foreign.
    NoKey,
    /// The supplied password did not open the password slot.
    WrongPassword,
    /// The file is damaged, truncated, or written by a newer build.
    Damaged(String),
    /// The keyring, the RNG, or another local facility failed. Distinct from [`Self::NoKey`]
    /// because the key may well be present and merely unreachable right now — offering to start
    /// over would discard a recoverable config.
    Backend(String),
}

impl std::fmt::Display for AccessError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NeedsPassword => f.write_str("нужен пароль шифрования servers.enc"),
            Self::NoKey => f.write_str(
                "servers.enc не открывается ключом этой машины (нет парольного слота, \
                 либо файл повреждён)",
            ),
            Self::WrongPassword => f.write_str("неверный пароль шифрования"),
            Self::Damaged(detail) => write!(f, "servers.enc повреждён: {detail}"),
            Self::Backend(detail) => write!(f, "хранилище ключей недоступно: {detail}"),
        }
    }
}

impl std::error::Error for AccessError {}

/// What the currently loaded file says about who can open it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AccessState {
    /// The process holds the file key, so saving is possible.
    pub opened: bool,
    /// Machine slots in the file, including this machine's.
    pub machine_slots: usize,
    /// A password slot exists.
    pub password_set: bool,
    /// The slots in memory differ from the ones on disk, so the file should be rewritten.
    pub needs_rewrite: bool,
}

/// Largest number of machine slots one file may carry.
pub const MAX_MACHINE_SLOTS: usize = slots::MAX_MACHINE_SLOTS;

/// Key material for the config file this process is working with.
struct Material {
    key: Zeroizing<[u8; 32]>,
    header: Header,
    /// Slots changed since the file was read, so the next save must persist them.
    needs_rewrite: bool,
}

/// Redacted by hand, never derived.
///
/// `Material` is returned inside a `Result`, so anything calling `unwrap`/`expect` on one formats
/// it — a derived `Debug` would print the file key into a test failure, a panic message, and from
/// there into `panic.log`.
impl std::fmt::Debug for Material {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Material")
            .field("key", &"<redacted>")
            .field("slots", &self.header.slots.len())
            .field("needs_rewrite", &self.needs_rewrite)
            .finish()
    }
}

/// Process-wide state of the config file key.
enum Vault {
    /// Nothing read or created yet.
    Unopened,
    /// The file key is known.
    Open(Material),
    /// A file exists but could not be opened. Writing is refused while this holds.
    Sealed(SealReason),
}

/// Why the vault is sealed, kept so a later write can explain itself.
#[derive(Clone, Copy)]
enum SealReason {
    NeedsPassword,
    NoKey,
    Damaged,
    Backend,
}

impl SealReason {
    fn describe(self) -> &'static str {
        match self {
            Self::NeedsPassword => "файл не был расшифрован (нужен пароль)",
            Self::NoKey => "файл не был расшифрован (нет подходящего ключа)",
            Self::Damaged => "файл не был прочитан (повреждён)",
            Self::Backend => "файл не был прочитан (хранилище ключей недоступно)",
        }
    }
}

static VAULT: Mutex<Vault> = Mutex::new(Vault::Unopened);

/// Lock the vault, recovering from a poisoned mutex so one panic cannot disable config saving for
/// the rest of the session.
fn vault() -> MutexGuard<'static, Vault> {
    VAULT.lock().unwrap_or_else(|error| error.into_inner())
}

/// Decrypt `servers.enc` and remember its key material for later saves.
///
/// # Errors
///
/// [`AccessError::NeedsPassword`] when only a password can open it, [`AccessError::NoKey`] when
/// nothing can. Both leave the vault SEALED, so a subsequent save refuses rather than overwriting
/// the file.
pub fn decrypt_config(data: &[u8]) -> Result<Vec<u8>, AccessError> {
    // A file this process has ALREADY opened is opened again with the key it is holding, not by
    // going back to the keyring. That is not an optimization — it is what makes the password work
    // at all: `unlock_with_password` adds this machine's slot in MEMORY, and the file on disk is
    // only rewritten by the save that comes later. Re-deriving from the keyring here would look at
    // the unchanged header, find no slot for this machine, and ask for the password again — one
    // step after the user typed it correctly.
    if let Some(plain) = reopen_with_held_key(data) {
        return Ok(plain);
    }
    let outcome = match machine::key() {
        Ok(machine_key) => open_file(data, machine_key.as_deref()),
        Err(e) => Err(AccessError::Backend(e.to_string())),
    };
    let mut vault = vault();
    match outcome {
        Ok((plain, material)) => {
            *vault = Vault::Open(material);
            Ok(plain)
        }
        Err(error) => {
            *vault = Vault::Sealed(match error {
                AccessError::NeedsPassword => SealReason::NeedsPassword,
                AccessError::NoKey => SealReason::NoKey,
                AccessError::Damaged(_) => SealReason::Damaged,
                AccessError::Backend(_) | AccessError::WrongPassword => SealReason::Backend,
            });
            Err(error)
        }
    }
}

/// Open `data` with the encryption password, then add THIS machine as a slot.
///
/// Adding the machine slot is the whole point of the password: it is asked once per machine, not
/// once per launch. The caller must persist the file afterwards — [`AccessState::needs_rewrite`]
/// reports that, and until it is written the next launch asks again.
///
/// Runs Argon2id, which takes long enough to be visible. Callers on a UI thread must move it to a
/// worker.
pub fn unlock_with_password(data: &[u8], password: &str) -> Result<Vec<u8>, AccessError> {
    let parsed = format::parse(data).map_err(|e| AccessError::Damaged(e.to_string()))?;
    let Parsed::V1 {
        header,
        header_bytes,
        nonce,
        ciphertext,
    } = parsed
    else {
        // A legacy file has no password slot at all; it is machine-key-only by construction.
        return Err(AccessError::NoKey);
    };
    let slot = header
        .slots
        .iter()
        .find(|slot| slot.kind == KIND_PASSWORD)
        .ok_or(AccessError::NoKey)?;
    let file_key = wrap::unwrap_password(slot, password).map_err(|_| AccessError::WrongPassword)?;
    let plain = wrap::open(&file_key, nonce, ciphertext, header_bytes)
        .map_err(|e| AccessError::Damaged(e.to_string()))?;

    let mut material = Material {
        key: file_key,
        header,
        needs_rewrite: false,
    };
    // Best effort: an unusable keyring must not stop the user from getting in, it only means this
    // machine will ask again next time.
    match add_this_machine(&mut material) {
        Ok(()) => log::info!("servers.enc: доступ этой машины записан, пароль больше не нужен"),
        Err(e) => log::warn!("servers.enc: не удалось запомнить доступ этой машины: {e:#}"),
    }
    *vault() = Vault::Open(material);
    Ok(plain)
}

/// Encrypt `plain` as the new contents of `servers.enc`.
///
/// # Errors
///
/// Refuses when the existing file could not be opened; see the module note.
pub fn encrypt_config(plain: &[u8]) -> anyhow::Result<Vec<u8>> {
    let mut vault = vault();
    refuse_if_sealed(&vault)?;
    if matches!(&*vault, Vault::Unopened) {
        // No file existed (or none was read): establish fresh material for this machine.
        let key = wrap::new_file_key()?;
        let mut material = Material {
            key,
            header: Header::new(),
            needs_rewrite: false,
        };
        add_this_machine(&mut material)?;
        *vault = Vault::Open(material);
    }
    let Vault::Open(material) = &mut *vault else {
        unreachable!("the vault was just established as open");
    };
    let header_bytes = format::header_bytes(&material.header)?;
    let (nonce, ciphertext) = wrap::seal(&material.key, plain, &header_bytes)?;
    // `needs_rewrite` is deliberately NOT cleared here. Producing bytes is not persisting them:
    // the caller still has to write the file, and clearing the flag on the optimistic path would
    // mark an unsaved slot as saved, with nothing left to ask for the retry. See
    // [`mark_persisted`].
    Ok(format::assemble(&header_bytes, &nonce, &ciphertext))
}

/// Record that the bytes from [`encrypt_config`] reached the disk.
///
/// Called by the writer after its atomic replace succeeds, so a failed write leaves
/// [`AccessState::needs_rewrite`] set and the next save tries again.
pub fn mark_persisted() {
    if let Vault::Open(material) = &mut *vault() {
        material.needs_rewrite = false;
    }
}

/// Decrypt a file that is NOT `servers.enc`, without touching the process key material.
///
/// Used for the legacy `config.enc` migration: its key material describes a file that is about to
/// be replaced, so adopting it as this process's material would carry a dead file's slots into the
/// new `servers.enc`.
pub fn decrypt_standalone(data: &[u8]) -> anyhow::Result<Vec<u8>> {
    let machine_key = machine::key()?
        .ok_or_else(|| anyhow::anyhow!("в OS keyring нет ключа шифрования конфига"))?;
    match format::parse(data)? {
        Parsed::Legacy { nonce, ciphertext } => wrap::open(&machine_key, nonce, ciphertext, &[]),
        Parsed::V1 {
            header,
            header_bytes,
            nonce,
            ciphertext,
        } => {
            let file_key = unwrap_for_machine(&header, &machine_key)?
                .ok_or_else(|| anyhow::anyhow!("файл не открывается ключом этой машины"))?;
            wrap::open(&file_key, nonce, ciphertext, header_bytes)
        }
    }
}

/// Set, change, or remove the encryption password.
///
/// Only the slot changes; the payload key is untouched, so existing backups stay readable with the
/// machine slots they already carry. The change reaches disk on the next config save.
///
/// Runs Argon2id when setting a password. See [`unlock_with_password`] about threads.
pub fn set_password(password: Option<&str>) -> Result<(), AccessError> {
    let mut vault = vault();
    let Vault::Open(material) = &mut *vault else {
        return Err(AccessError::NoKey);
    };
    let slot = match password {
        Some(password) => Some(
            wrap::password_slot(&material.key, password)
                .map_err(|e| AccessError::Backend(e.to_string()))?,
        ),
        None => None,
    };
    material.header.set_password_slot(slot);
    material.needs_rewrite = true;
    Ok(())
}

/// Forget a file that could not be opened, so a fresh one may be written in its place.
///
/// Called only after the unopenable file has been MOVED ASIDE. Without this the vault stays sealed
/// for the rest of the session and `encrypt_config` would refuse to write the empty configuration
/// the user just chose to start from — the refusal is there to protect a file that still exists.
pub fn forget_sealed_file() {
    let mut vault = vault();
    if matches!(&*vault, Vault::Sealed(_)) {
        *vault = Vault::Unopened;
    }
}

/// Whether a launch password is configured in the loaded file.
pub fn launch_password_is_set() -> bool {
    matches!(&*vault(), Vault::Open(material) if material.header.launch.is_some())
}

/// Check `password` against the stored launch verifier.
///
/// Returns false when no file is open or no launch password is set. A caller must therefore decide
/// whether to prompt using [`launch_password_is_set`] rather than by trying an empty password —
/// otherwise "no gate configured" and "wrong password" would be the same answer.
pub fn verify_launch_password(password: &str) -> bool {
    let vault = vault();
    let Vault::Open(material) = &*vault else {
        return false;
    };
    material
        .header
        .launch
        .as_deref()
        .is_some_and(|verifier| launch::verify(verifier, password))
}

/// Set or remove the launch password. Reaches disk with the next config save.
pub fn set_launch_password(password: Option<&str>) -> Result<(), AccessError> {
    let verifier = match password {
        Some(password) => {
            Some(launch::hash(password).map_err(|e| AccessError::Backend(e.to_string()))?)
        }
        None => None,
    };
    let mut vault = vault();
    let Vault::Open(material) = &mut *vault else {
        return Err(AccessError::NoKey);
    };
    material.header.launch = verifier;
    material.needs_rewrite = true;
    Ok(())
}

/// Drop every machine slot except this machine's.
///
/// The password slot is kept: see [`Header::retain_only_machine`].
pub fn forget_other_machines() -> Result<(), AccessError> {
    let machine_key = machine::key()
        .map_err(|e| AccessError::Backend(e.to_string()))?
        .ok_or(AccessError::NoKey)?;
    let id = machine::slot_id(&machine_key);
    let mut vault = vault();
    let Vault::Open(material) = &mut *vault else {
        return Err(AccessError::NoKey);
    };
    material.header.retain_only_machine(&id);
    material.needs_rewrite = true;
    Ok(())
}

/// Report what the loaded file says about access.
pub fn access_state() -> AccessState {
    match &*vault() {
        Vault::Open(material) => AccessState {
            opened: true,
            machine_slots: material.header.machine_slot_count(),
            password_set: material.header.has_password_slot(),
            needs_rewrite: material.needs_rewrite,
        },
        Vault::Unopened | Vault::Sealed(_) => AccessState::default(),
    }
}

/// Decrypt a config file with the machine key supplied by the caller.
///
/// The key is a parameter rather than a keyring read so this — the branch that decides between
/// "opened", "ask for a password" and "no way in" — can be tested without an OS key store. The
/// keyring lives in exactly two places: [`decrypt_config`] and [`add_this_machine`].
fn open_file(
    data: &[u8],
    machine_key: Option<&[u8; 32]>,
) -> Result<(Vec<u8>, Material), AccessError> {
    let parsed = format::parse(data).map_err(|e| AccessError::Damaged(e.to_string()))?;

    match parsed {
        Parsed::Legacy { nonce, ciphertext } => {
            let machine_key = machine_key.ok_or(AccessError::NoKey)?;
            let plain =
                wrap::open(machine_key, nonce, ciphertext, &[]).map_err(|_| AccessError::NoKey)?;
            // Adopt the legacy payload under fresh slot-based material. The file is rewritten on
            // the next save; `needs_rewrite` is what asks for one even if nothing else changed.
            let key = wrap::new_file_key().map_err(|e| AccessError::Backend(e.to_string()))?;
            let mut material = Material {
                key,
                header: Header::new(),
                needs_rewrite: true,
            };
            add_machine(&mut material, machine_key)
                .map_err(|e| AccessError::Backend(e.to_string()))?;
            log::info!("servers.enc: старый формат прочитан, при следующем сохранении обновится");
            Ok((plain, material))
        }
        Parsed::V1 {
            header,
            header_bytes,
            nonce,
            ciphertext,
        } => {
            let Some(machine_key) = machine_key else {
                return Err(missing_key_error(&header));
            };
            let Some(file_key) = unwrap_for_machine(&header, machine_key)
                .map_err(|e| AccessError::Damaged(e.to_string()))?
            else {
                return Err(missing_key_error(&header));
            };
            let plain = wrap::open(&file_key, nonce, ciphertext, header_bytes)
                .map_err(|e| AccessError::Damaged(e.to_string()))?;
            Ok((
                plain,
                Material {
                    key: file_key,
                    header,
                    needs_rewrite: false,
                },
            ))
        }
    }
}

/// Refuse to produce file bytes while the vault is sealed.
///
/// Separated from [`encrypt_config`] so the refusal can be tested without a key store: the branch
/// it guards is the one standing between a failed decryption and the permanent loss of the slots
/// that failure was about.
fn refuse_if_sealed(vault: &Vault) -> anyhow::Result<()> {
    if let Vault::Sealed(reason) = vault {
        anyhow::bail!(
            "запись servers.enc запрещена: {} — перезапись уничтожила бы ключи доступа",
            reason.describe()
        );
    }
    Ok(())
}

/// Decrypt `data` with the file key this process is already holding, if it opens it.
///
/// Returns `None` when no key is held or it does not open this payload, so the caller falls back
/// to the ordinary keyring path. The in-memory slots are deliberately left untouched: they may
/// already contain a machine slot that the file on disk has not received yet, and replacing them
/// with the header just read would discard exactly that.
fn reopen_with_held_key(data: &[u8]) -> Option<Vec<u8>> {
    let vault = vault();
    let Vault::Open(material) = &*vault else {
        return None;
    };
    let Ok(Parsed::V1 {
        header_bytes,
        nonce,
        ciphertext,
        ..
    }) = format::parse(data)
    else {
        return None;
    };
    wrap::open(&material.key, nonce, ciphertext, header_bytes).ok()
}

/// Choose between "ask for the password" and "there is no way in".
fn missing_key_error(header: &Header) -> AccessError {
    if header.has_password_slot() {
        AccessError::NeedsPassword
    } else {
        AccessError::NoKey
    }
}

/// Try to unwrap the file key with this machine's slot.
///
/// Returns `Ok(None)` when no slot belongs to this machine. A slot whose id matches but whose
/// contents do not open is an error rather than a miss: that is a damaged or tampered file, not a
/// different machine.
fn unwrap_for_machine(
    header: &Header,
    machine_key: &[u8; 32],
) -> anyhow::Result<Option<Zeroizing<[u8; 32]>>> {
    let id = machine::slot_id(machine_key);
    let Some(slot) = header
        .slots
        .iter()
        .find(|slot| slot.kind == KIND_MACHINE && slot.id.as_deref() == Some(id.as_str()))
    else {
        return Ok(None);
    };
    wrap::unwrap_machine(slot, machine_key).map(Some)
}

/// Add or refresh this machine's slot in `material`, creating the keyring key if needed.
///
/// Creation is confined here and to nothing else: this is the only situation in which a machine
/// legitimately establishes a key, namely when it is being recorded as able to open a file it can
/// already read.
fn add_this_machine(material: &mut Material) -> anyhow::Result<()> {
    let machine_key = machine::key_or_create()?;
    add_machine(material, &machine_key)
}

/// Add or refresh a machine slot for the supplied key.
fn add_machine(material: &mut Material, machine_key: &[u8; 32]) -> anyhow::Result<()> {
    let id = machine::slot_id(machine_key);
    let slot: Slot = wrap::machine_slot(&material.key, machine_key, id)?;
    material.header.put_machine_slot(slot);
    material.needs_rewrite = true;
    Ok(())
}

#[cfg(test)]
mod tests;
