//! A minidump of the faulting process, written beside the logs as the crash filter's last act.
//!
//! The text report names the faulting instruction and the raw stack; the dump carries the stack
//! of every thread with its registers — and deliberately no data: neither the heap nor the values
//! on the stacks, only the pointers that let a debugger walk them, so a forwarded dump cannot
//! carry a key. WinDbg reads it without symbols, and with the PDB kept from the same release build
//! it reads as source lines.
//! The Windows Error Reporting way of getting one — `LocalDumps` in the registry — needs an
//! administrator on the user's machine; this needs nothing.
//!
//! Written LAST, after every text line: `MiniDumpWriteDump` lives in `dbghelp`, which is the
//! library that has faulted inside this handler before. Failing here costs the dump, nothing else.

#[cfg(test)]
mod tests;

use std::os::windows::io::AsRawHandle;
use std::path::{Path, PathBuf};

use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::Diagnostics::Debug::{
    EXCEPTION_POINTERS, MINIDUMP_EXCEPTION_INFORMATION, MiniDumpFilterMemory, MiniDumpNormal,
    MiniDumpWithThreadInfo, MiniDumpWithUnloadedModules, MiniDumpWriteDump,
};
use windows::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentProcessId, GetCurrentThreadId,
};

/// Dumps kept in `logs/`: the newest crash and the two before it. Pruned at startup, so a run
/// that crashes still leaves at most this many behind.
pub(crate) const KEEP: usize = 3;

/// File name prefix; the suffix is the local wall-clock time of the crash and the process id — a
/// launch that crashes at once, restarted within the same second, must not overwrite the dump
/// of the launch before it.
const PREFIX: &str = "crash-";

/// Writes `logs/crash-<local time>-<pid>.dmp` for the exception in `info`.
///
/// Args:
///     info: The filter's exception pointers; null is accepted and yields a dump without an
///         exception stream.
///
/// Returns:
///     The dump's path and size in KiB, or the reason nothing was written.
pub(super) fn write(info: *const EXCEPTION_POINTERS) -> Result<(PathBuf, u64), String> {
    let dir = moon_core::config::paths::logs_dir_no_create();
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let path = dir.join(format!("{PREFIX}{stamp}-{}.dmp", std::process::id()));
    let file = std::fs::File::create(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let exception = MINIDUMP_EXCEPTION_INFORMATION {
        ThreadId: unsafe { GetCurrentThreadId() },
        ExceptionPointers: info.cast_mut(),
        // The pointers are ours, in this process.
        ClientPointers: false.into(),
    };
    // Stacks, thread names/times and the modules that were unloaded before the fault — and
    // nothing of the heap. `MiniDumpFilterMemory` strips the stacks down to the pointers a
    // debugger needs to walk them: the user forwards `logs/` to support with this file in it,
    // and a stack frame can hold an exchange key or a password as easily as a counter. The
    // stack trace, which is what the dump is for, survives the filter intact.
    let kind = MiniDumpNormal
        | MiniDumpFilterMemory
        | MiniDumpWithThreadInfo
        | MiniDumpWithUnloadedModules;
    unsafe {
        MiniDumpWriteDump(
            GetCurrentProcess(),
            GetCurrentProcessId(),
            HANDLE(file.as_raw_handle()),
            kind,
            (!info.is_null()).then_some(&exception as *const _),
            None,
            None,
        )
    }
    .map_err(|e| format!("MiniDumpWriteDump: {e}"))?;
    let size = file.metadata().map(|m| m.len() / 1024).unwrap_or(0);
    Ok((path, size))
}

/// Deletes the oldest `crash-*.dmp` files in `dir` until `keep - 1` remain, so the next crash
/// brings the folder to `keep`. Names sort by their timestamp (two dumps from the same second
/// sort by their pid text, which is as good as any order for files that close together); nothing
/// else in the folder is touched. Called once at startup, never from the handler.
pub(crate) fn prune(dir: &Path, keep: usize) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut dumps: Vec<PathBuf> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.extension().is_some_and(|x| x == "dmp")
                && p.file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with(PREFIX))
        })
        .collect();
    dumps.sort();
    let excess = dumps.len().saturating_sub(keep.saturating_sub(1));
    for old in dumps.iter().take(excess) {
        if let Err(e) = std::fs::remove_file(old) {
            log::warn!("crash dump {} not removed: {e}", old.display());
        }
    }
}
