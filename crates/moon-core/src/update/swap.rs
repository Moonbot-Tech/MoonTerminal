//! The Linux halves of the terminal's self-update transaction: binding to the process that
//! started the helper, waiting for it to exit, and swapping the installed executable while a
//! backup is kept.
//!
//! The transaction itself — helper, manifest, markers, the order of rollback — lives in the
//! terminal's `update.rs` and is the same on every platform. Windows implements these steps
//! there with `OpenProcess` and `ReplaceFileW`. The Linux ones live here instead, in the crate
//! the Linux CI job can test without building the GUI: a mistake in them leaves a user with no
//! working executable, and a Windows-only test suite would never run them.
//!
//! Why each step is shaped the way it is:
//!   - the parent is held through a process descriptor (`pidfd_open`), not a pid, so the process
//!     waited on is the one that was checked, whatever happens to its pid afterwards;
//!   - the parent's image is compared with the installed file by device and inode, read through
//!     `/proc/<pid>/exe`, so neither a renamed install nor a path that merely looks the same can
//!     stand in for it;
//!   - the backup is a second NAME for the installed file (a hard link), and the replacement is
//!     one `rename`: at every instant the install path names a complete executable, the old one
//!     or the new one.

use std::fs;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};

#[cfg(test)]
mod tests;

/// Permission bits a mode carries, without the file-type bits `st_mode` holds above them.
const PERMISSION_BITS: u32 = 0o7777;

/// The exact process a helper was started by, held open so its exit can be awaited.
pub struct ParentProcess {
    pidfd: OwnedFd,
}

impl ParentProcess {
    /// Bind to the running process `pid` after proving its image is the installed executable.
    ///
    /// Args:
    ///     pid: Process id the old application recorded in the transaction manifest.
    ///     target: Installed executable the transaction is about to replace.
    ///
    /// Returns:
    ///     A handle whose [`Self::wait_for_exit`] waits for that process and no other.
    ///
    /// Errors:
    ///     The process cannot be opened, its image is not `target`, or it exited before the
    ///     check completed — in which case the pid may already name someone else.
    pub fn open(pid: u32, target: &Path) -> anyhow::Result<Self> {
        let pid_arg = libc::pid_t::try_from(pid).context("update parent pid is out of range")?;
        // SAFETY: `pidfd_open` takes a pid and a flags word and returns a new descriptor or -1;
        // it reads no memory of ours.
        let raw = unsafe { libc::syscall(libc::SYS_pidfd_open, pid_arg, 0) };
        if raw < 0 {
            return Err(io::Error::last_os_error())
                .context("open update parent process (pidfd_open, Linux 5.3 or newer)");
        }
        let raw = RawFd::try_from(raw).context("update parent descriptor is out of range")?;
        // SAFETY: the kernel just returned this descriptor to us and nothing else owns it.
        let parent = Self {
            pidfd: unsafe { OwnedFd::from_raw_fd(raw) },
        };

        let image =
            fs::metadata(format!("/proc/{pid}/exe")).context("query update parent image")?;
        let installed = fs::metadata(target).context("query installed executable")?;
        if image.dev() != installed.dev() || image.ino() != installed.ino() {
            bail!("update parent is not the installed executable");
        }
        // `/proc/<pid>` was read by number. It described the process this descriptor names only
        // if that process was still alive afterwards: a pid is reused after its owner is gone.
        if parent.has_exited().context("poll update parent process")? {
            bail!("update parent exited before it could be bound");
        }
        Ok(parent)
    }

    /// Block until the bound process has exited.
    ///
    /// Waits without a deadline on purpose: after the helper published readiness it owns
    /// availability, and a slow shutdown of the old application must not make it give up while
    /// that application still holds the install.
    ///
    /// Errors:
    ///     Polling the process descriptor failed.
    pub fn wait_for_exit(self) -> anyhow::Result<()> {
        let exited = self
            .poll_exit(-1)
            .context("failed while waiting for update parent exit")?;
        if !exited {
            bail!("the wait for update parent exit returned while it was still running");
        }
        Ok(())
    }

    /// Whether the bound process has already exited, without waiting.
    fn has_exited(&self) -> io::Result<bool> {
        self.poll_exit(0)
    }

    /// Poll the process descriptor, which becomes readable when the process exits.
    ///
    /// Args:
    ///     timeout_ms: Milliseconds to wait; zero returns at once, negative waits forever.
    ///
    /// Returns:
    ///     Whether the process has exited.
    fn poll_exit(&self, timeout_ms: libc::c_int) -> io::Result<bool> {
        let mut descriptor = libc::pollfd {
            fd: self.pidfd.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        loop {
            // SAFETY: `descriptor` is one valid, exclusively borrowed `pollfd`.
            let ready = unsafe { libc::poll(&mut descriptor, 1, timeout_ms) };
            if ready >= 0 {
                return Ok(ready > 0);
            }
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::Interrupted {
                return Err(error);
            }
        }
    }
}

/// Give a downloaded file the execute bits its read bits allow.
///
/// A download is created without them, and the staged update is the helper the transaction
/// runs. Execute follows read — `0644` becomes `0755`, `0600` becomes `0700` — so the file is
/// never made runnable by anyone who could not already read it.
///
/// Args:
///     path: The verified staged executable.
///
/// Errors:
///     The file's mode cannot be read or changed.
pub fn mark_executable(path: &Path) -> anyhow::Result<()> {
    let mode = fs::metadata(path)
        .context("query staged executable mode")?
        .mode()
        & PERMISSION_BITS;
    let executable = mode | ((mode & 0o444) >> 2);
    fs::set_permissions(path, fs::Permissions::from_mode(executable))
        .context("mark staged executable runnable")
}

/// Put `incoming` in place of the installed executable, keeping the old one at `backup`.
///
/// Args:
///     target: Installed executable; must exist.
///     incoming: Verified new executable on the same filesystem; consumed by the call.
///     backup: Path that must not exist yet; names the previous executable afterwards.
///
/// Errors:
///     The backup cannot be made or the rename fails. In both cases `target` still names the
///     previous executable, and a `backup` that was already made is left for the caller's
///     recovery to restore or remove.
pub fn replace_keeping_backup(target: &Path, incoming: &Path, backup: &Path) -> anyhow::Result<()> {
    let installed = fs::metadata(target).context("query installed executable")?;
    let mode = installed.mode() & PERMISSION_BITS;
    // A hard link first: the previous executable gets its second name before the first one is
    // taken away. A backup path that is already taken is refused outright — it is not ours to
    // overwrite. Any other failure means a filesystem without hard links, which gets a full
    // copy into a file this call creates itself.
    match fs::hard_link(target, backup) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            return Err(error).context("update backup path is already taken");
        }
        Err(_) => copy_into_new_file(target, backup, mode)
            .context("keep the previous executable as a backup")?,
    }
    // The new file takes over the old one's permissions, so an install that was private stays
    // private and one that was runnable stays runnable.
    fs::set_permissions(incoming, fs::Permissions::from_mode(mode))
        .context("carry the installed mode onto the new executable")?;
    fs::rename(incoming, target).context("replace installed executable")?;
    sync_parent_directory(target);
    Ok(())
}

/// Put the backup back over a failed replacement.
///
/// One `rename`, which replaces `target` when it exists and recreates it when it does not.
///
/// Args:
///     target: Installed executable path to restore.
///     backup: The previous executable kept by [`replace_keeping_backup`]; consumed by the call.
///
/// Errors:
///     The rename fails; `target` is then whatever it was before the call.
pub fn restore_backup(target: &Path, backup: &Path) -> anyhow::Result<()> {
    // A replacement that failed after the backup link was made leaves both names on one file.
    // `rename` between two names of the same file does nothing and reports success, which would
    // leave the backup behind. The install is already the previous executable then, so only the
    // extra name has to go.
    if is_same_file(target, backup) {
        return fs::remove_file(backup).context("remove the redundant update backup");
    }
    fs::rename(backup, target).context("restore previous executable")?;
    sync_parent_directory(target);
    Ok(())
}

/// Whether two paths name one file, by device and inode. `false` when either cannot be read.
fn is_same_file(left: &Path, right: &Path) -> bool {
    match (fs::metadata(left), fs::metadata(right)) {
        (Ok(left), Ok(right)) => left.dev() == right.dev() && left.ino() == right.ino(),
        _ => false,
    }
}

/// Copy `source` to `destination` so that `destination` is the complete copy or does not exist.
///
/// The bytes go into a sibling `.part` file and are renamed into place only once they are
/// flushed. A copy that fails halfway — a full disk, a read error — therefore leaves nothing
/// under the destination's name. That matters because the caller's recovery treats an existing
/// backup as the previous executable and puts it back over the install: a truncated one would
/// destroy an install that was still intact.
///
/// A process killed in the middle of the copy leaves the `.part` behind. It is never read as a
/// backup; it only keeps that transaction directory from being removed until it is deleted.
///
/// Args:
///     source: File to copy.
///     destination: Path the copy is published at; the caller has established it is free.
///     mode: Permission bits the copy gets.
fn copy_into_new_file(source: &Path, destination: &Path, mode: u32) -> io::Result<()> {
    let mut part = destination.as_os_str().to_owned();
    part.push(".part");
    let part = PathBuf::from(part);
    // `create_new`, not `fs::copy`: a copy would follow a link planted at the working name and
    // overwrite whatever it points at. A working name that is already taken is refused and left
    // alone — only a file this call created is ever removed below.
    let mut output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&part)?;
    let published =
        fill_and_flush(source, &mut output, mode).and_then(|()| fs::rename(&part, destination));
    if published.is_err() {
        let _ = fs::remove_file(&part);
    }
    published
}

/// Fill an open file with a copy of `source`, give it an exact mode, and flush it to disk.
fn fill_and_flush(source: &Path, output: &mut fs::File, mode: u32) -> io::Result<()> {
    let mut input = fs::File::open(source)?;
    io::copy(&mut input, output)?;
    output.set_permissions(fs::Permissions::from_mode(mode))?;
    output.sync_all()
}

/// Flush the directory entry change to disk, best effort.
///
/// The rename is already visible to every process; this only narrows what a power loss could
/// undo, so a filesystem that cannot sync a directory is not a reason to fail the update.
fn sync_parent_directory(path: &Path) {
    let Some(parent) = path.parent() else {
        return;
    };
    if let Ok(directory) = fs::File::open(parent) {
        let _ = directory.sync_all();
    }
}
