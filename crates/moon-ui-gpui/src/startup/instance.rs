//! One live MoonTerminal process per canonical install/data directory.
//!
//! Identity is [`moon_core::config::paths::instance_identity_dir`], never the process name and
//! never the working directory of a shortcut. A copy in a different folder may run at the same
//! time. The lock is a named mutex on Windows and flock on Unix so a crash cannot leave a stale
//! PID that bricks the next launch.

use moon_core::config::paths;

/// Result of trying to become the owner of this install directory.
pub(super) enum Acquire {
    /// This process now owns the install directory for its lifetime.
    Primary(InstanceGuard),
    /// Another live process already owns it; that process has been asked to come to the front.
    AlreadyRunning,
}

/// Process-lifetime ownership of the install-directory lock and the activation wake.
pub(super) struct InstanceGuard {
    inner: Inner,
}

enum Inner {
    #[cfg(windows)]
    Windows {
        mutex: windows::Win32::Foundation::HANDLE,
        event: windows::Win32::Foundation::HANDLE,
    },
    #[cfg(unix)]
    Unix {
        _lock: std::fs::File,
        wake: std::path::PathBuf,
    },
    #[cfg(not(any(windows, unix)))]
    Unsupported,
}

/// Whether this launch must not take the install lock.
///
/// FireTest and `--fixture` have to be able to run beside a live terminal. The UI-atlas walk is
/// the same: a lock would put a six-minute crawl behind the already-running process instead of
/// driving its own windows. Updater helper modes never reach this function — they never enter
/// [`super::run`].
///
/// Args:
///     firetest: Whether `--debug-script` selected a FireTest scenario.
///
/// Returns:
///     `true` when startup should continue without the lock.
pub(super) fn lock_exempt(firetest: bool) -> bool {
    if firetest {
        return true;
    }
    if paths::has_data_dir_override() {
        return true;
    }
    #[cfg(uidoc)]
    {
        if std::env::args().any(|arg| arg == "--ui-atlas") {
            return true;
        }
    }
    false
}

/// Named mutex for this install directory.
///
/// `Global\` is machine-wide: Fast User Switching and Remote Desktop must not start a second
/// writer against the same portable folder. `Local\` is per session and would allow two live
/// processes on one path. An interactive process can create these objects without
/// `SeCreateGlobalPrivilege`.
#[cfg_attr(not(windows), allow(dead_code))]
pub(super) fn mutex_name(token: &str) -> String {
    format!(r"Global\MoonTerminal-{token}")
}

/// Named event the owner peeks to bring its windows forward.
///
/// Same `Global\` namespace as [`mutex_name`]: a second launch from another session has to wake
/// the process that already owns the folder, not a session-local event nobody is watching.
#[cfg_attr(not(windows), allow(dead_code))]
pub(super) fn event_name(token: &str) -> String {
    format!(r"Global\MoonTerminal-wake-{token}")
}

/// Take the install lock, or ask the existing owner to activate and yield.
///
/// Returns:
///     [`Acquire::Primary`] holding kernel objects / the flock for process lifetime, or
///     [`Acquire::AlreadyRunning`] after a best-effort wake.
///
/// Errors:
///     Unexpected OS failures creating the lock. A second instance that cannot open the wake
///     still returns [`Acquire::AlreadyRunning`] — the product forbids an error dialog.
pub(super) fn acquire() -> anyhow::Result<Acquire> {
    let dir = paths::instance_identity_dir();
    let token = paths::instance_identity_token(&dir);
    log::info!("instance identity {} ({token})", dir.display());
    for attempt in 0..5 {
        match acquire_named(&token)? {
            Named::Primary(guard) => return Ok(Acquire::Primary(guard)),
            Named::Signaled => return Ok(Acquire::AlreadyRunning),
            Named::Unconfirmed => {
                if attempt == 4 {
                    return Ok(Acquire::AlreadyRunning);
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        }
    }
    Ok(Acquire::AlreadyRunning)
}

/// One attempt to own or wake the install lock.
enum Named {
    Primary(InstanceGuard),
    /// Existing owner was signaled and should activate.
    Signaled,
    /// A lock existed but the wake object was gone — owner may have just died.
    Unconfirmed,
}

/// Peek whether a second launch asked this process to come to the front.
///
/// Args:
///     none — reads the wake owned by `self`.
///
/// Returns:
///     `true` once per signal (auto-reset event on Windows, wake-file consume on Unix).
pub(super) fn poll_activation(guard: &InstanceGuard) -> bool {
    guard.wake_watch().poll()
}

/// Copy of the wake side of the lock, for a window that must peek without owning the mutex.
///
/// The login prompt holds this while `BootInput` still owns the [`InstanceGuard`]. The watch
/// must not close kernel handles; dropping the guard does that.
#[derive(Clone)]
pub(super) struct WakeWatch {
    #[cfg(windows)]
    event: windows::Win32::Foundation::HANDLE,
    #[cfg(unix)]
    wake: std::path::PathBuf,
    #[cfg(not(any(windows, unix)))]
    _unused: (),
}

impl InstanceGuard {
    /// Wake object the login window polls until it is dismissed.
    pub(super) fn wake_watch(&self) -> WakeWatch {
        match &self.inner {
            #[cfg(windows)]
            Inner::Windows { event, .. } => WakeWatch { event: *event },
            #[cfg(unix)]
            Inner::Unix { wake, .. } => WakeWatch { wake: wake.clone() },
            #[cfg(not(any(windows, unix)))]
            Inner::Unsupported => WakeWatch { _unused: () },
        }
    }
}

impl WakeWatch {
    /// Consume one activation signal, if any.
    pub(super) fn poll(&self) -> bool {
        match self {
            #[cfg(windows)]
            WakeWatch { event } => poll_windows_event(*event),
            #[cfg(unix)]
            WakeWatch { wake } => match std::fs::remove_file(wake) {
                Ok(()) => true,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
                Err(error) => {
                    log::warn!("instance wake peek failed: {error}");
                    false
                }
            },
            #[cfg(not(any(windows, unix)))]
            WakeWatch { _unused: () } => false,
        }
    }
}

impl Drop for InstanceGuard {
    fn drop(&mut self) {
        match &self.inner {
            #[cfg(windows)]
            Inner::Windows { mutex, event } => {
                close_handle(*mutex);
                close_handle(*event);
            }
            #[cfg(unix)]
            Inner::Unix { .. } => {}
            #[cfg(not(any(windows, unix)))]
            Inner::Unsupported => {}
        }
    }
}

#[cfg(windows)]
fn acquire_named(token: &str) -> anyhow::Result<Named> {
    use windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError, SetLastError};
    use windows::Win32::System::Threading::{CreateEventW, CreateMutexW};
    use windows::core::PCWSTR;

    let mutex_wide = wide(&mutex_name(token));
    let event_wide = wide(&event_name(token));
    unsafe { SetLastError(windows::Win32::Foundation::WIN32_ERROR(0)) };
    let mutex = unsafe { CreateMutexW(None, false, PCWSTR(mutex_wide.as_ptr())) }
        .map_err(|error| anyhow::anyhow!("instance mutex: {error}"))?;
    let last_error = unsafe { GetLastError() };
    if last_error == ERROR_ALREADY_EXISTS {
        close_handle(mutex);
        return Ok(if signal_windows_event(&event_wide) {
            Named::Signaled
        } else {
            Named::Unconfirmed
        });
    }
    let event = match unsafe { CreateEventW(None, false, false, PCWSTR(event_wide.as_ptr())) } {
        Ok(event) => event,
        Err(error) => {
            close_handle(mutex);
            return Err(anyhow::anyhow!("instance wake event: {error}"));
        }
    };
    Ok(Named::Primary(InstanceGuard {
        inner: Inner::Windows { mutex, event },
    }))
}

#[cfg(windows)]
fn signal_windows_event(event_wide: &[u16]) -> bool {
    use std::time::Duration;

    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{EVENT_MODIFY_STATE, OpenEventW, SetEvent};
    use windows::core::PCWSTR;

    for _ in 0..20 {
        match unsafe { OpenEventW(EVENT_MODIFY_STATE, false, PCWSTR(event_wide.as_ptr())) } {
            Ok(event) => {
                let signaled = unsafe { SetEvent(event) };
                let _ = unsafe { CloseHandle(event) };
                if signaled.is_err() {
                    log::warn!("instance wake SetEvent failed");
                    return false;
                }
                return true;
            }
            Err(_) => std::thread::sleep(Duration::from_millis(50)),
        }
    }
    log::info!("instance wake event was not yet created; will retry ownership");
    false
}

#[cfg(windows)]
fn poll_windows_event(event: windows::Win32::Foundation::HANDLE) -> bool {
    use windows::Win32::Foundation::WAIT_OBJECT_0;
    use windows::Win32::System::Threading::WaitForSingleObject;

    let result = unsafe { WaitForSingleObject(event, 0) };
    result == WAIT_OBJECT_0
}

#[cfg(windows)]
fn close_handle(handle: windows::Win32::Foundation::HANDLE) {
    if !handle.is_invalid() {
        let _ = unsafe { windows::Win32::Foundation::CloseHandle(handle) };
    }
}

#[cfg(windows)]
fn wide(name: &str) -> Vec<u16> {
    name.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(unix)]
fn acquire_named(_token: &str) -> anyhow::Result<Named> {
    use std::fs::OpenOptions;
    use std::os::unix::io::AsRawFd;

    let lock_path = paths::instance_lock_path();
    let wake_path = paths::instance_wake_path();
    match try_unix_lock(&lock_path)? {
        Some(file) => Ok(Named::Primary(InstanceGuard {
            inner: Inner::Unix {
                _lock: file,
                wake: wake_path,
            },
        })),
        None => {
            if let Err(wake_error) = std::fs::File::create(&wake_path) {
                log::warn!("instance wake {} failed: {wake_error}", wake_path.display());
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
            match try_unix_lock(&lock_path)? {
                Some(file) => Ok(Named::Primary(InstanceGuard {
                    inner: Inner::Unix {
                        _lock: file,
                        wake: wake_path,
                    },
                })),
                None => Ok(Named::Signaled),
            }
        }
    }
}

#[cfg(unix)]
fn try_unix_lock(lock_path: &std::path::Path) -> anyhow::Result<Option<std::fs::File>> {
    use std::fs::OpenOptions;
    use std::os::unix::io::AsRawFd;

    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(lock_path)
        .map_err(|error| anyhow::anyhow!("instance lock {}: {error}", lock_path.display()))?;
    let rc = unsafe { flock(file.as_raw_fd(), LOCK_EX | LOCK_NB) };
    if rc == 0 {
        return Ok(Some(file));
    }
    let error = std::io::Error::last_os_error();
    if error.kind() == std::io::ErrorKind::WouldBlock {
        return Ok(None);
    }
    Err(anyhow::anyhow!(
        "instance flock {}: {error}",
        lock_path.display()
    ))
}

#[cfg(unix)]
const LOCK_EX: i32 = 2;
#[cfg(unix)]
const LOCK_NB: i32 = 4;

#[cfg(unix)]
unsafe extern "C" {
    fn flock(fd: i32, operation: i32) -> i32;
}

#[cfg(not(any(windows, unix)))]
fn acquire_named(_token: &str) -> anyhow::Result<Named> {
    Ok(Named::Primary(InstanceGuard {
        inner: Inner::Unsupported,
    }))
}
