//! Shared filesystem reserve and packed-tape growth accounting.

use std::path::Path;

/// Minimum space kept free on every known-size filesystem.
pub const MIN_FREE_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// Divisor retaining ten percent of the filesystem.
pub const MIN_FREE_SHARE: u64 = 10;

/// Return the larger of the absolute reserve and the filesystem's reserved share.
pub fn reserve(total: u64) -> u64 {
    MIN_FREE_BYTES.max(total / MIN_FREE_SHARE)
}

/// Return spendable bytes, failing closed below reserve and allowing unknown-size mounts.
pub fn spendable(free: u64, total: u64) -> Option<u64> {
    if total == 0 {
        Some(u64::MAX)
    } else {
        free.checked_sub(reserve(total))
    }
}

/// Budget VACUUM's temporary copy and WAL write-back while preserving the disk reserve.
pub fn vacuum_has_room(db_path: &Path) -> bool {
    vacuum_has_room_with(db_path, disk_space)
}

/// Probe the database mount; unavailable measurements allow maintenance with a warning.
pub(crate) fn vacuum_has_room_with(
    db_path: &Path,
    probe: impl FnOnce(&Path) -> Option<(u64, u64)>,
) -> bool {
    let Some((free, total)) = db_path.parent().and_then(probe) else {
        log::warn!(
            "VACUUM disk probe failed; allowing maintenance: {}",
            db_path.display()
        );
        return true;
    };
    let (main, wal) = crate::db::maint::db_sizes(db_path);
    vacuum_room(free, total, main, wal)
}

/// Compare in wide arithmetic so overflowing file sizes cannot admit an undersized volume.
fn vacuum_room(free: u64, total: u64, main: u64, wal: u64) -> bool {
    u128::from(free) >= 2 * u128::from(main) + u128::from(wal) + u128::from(reserve(total))
}

/// Convert physical headroom to packed bytes without evicting for an unreclaimable shortfall.
pub fn tape_ceiling(held_packed: u64, file_bytes: u64, free: u64, total: u64, cap: u64) -> u64 {
    if total == 0 {
        return cap;
    }
    let headroom = free.saturating_sub(reserve(total));
    let growth = if file_bytes == 0 || held_packed == 0 {
        u128::from(headroom)
    } else {
        u128::from(headroom) * u128::from(held_packed) / u128::from(file_bytes)
    };
    u128::from(cap).min(u128::from(held_packed) + growth) as u64
}

/// Query caller-available bytes and total volume bytes for a directory.
#[cfg(windows)]
pub fn disk_space(path: &Path) -> Option<(u64, u64)> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        /// Return caller-available and total byte counts for the directory's volume.
        fn GetDiskFreeSpaceExW(
            path: *const u16,
            free: *mut u64,
            total: *mut u64,
            all_free: *mut u64,
        ) -> i32;
    }
    let path: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let (mut free, mut total) = (0, 0);
    // SAFETY: the path is NUL terminated and both output pointers refer to live u64 values.
    let ok =
        unsafe { GetDiskFreeSpaceExW(path.as_ptr(), &mut free, &mut total, std::ptr::null_mut()) };
    (ok != 0).then_some((free, total))
}

/// Use caller-available Unix filesystem counters, including unprivileged space restrictions.
#[cfg(unix)]
pub fn disk_space(path: &Path) -> Option<(u64, u64)> {
    use std::os::unix::ffi::OsStrExt;
    let path = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut stat = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: the path is NUL terminated and stat points to writable storage for one statvfs.
    let stat = unsafe {
        if libc::statvfs(path.as_ptr(), stat.as_mut_ptr()) != 0 {
            return None;
        }
        stat.assume_init()
    };
    let block = match stat.f_frsize as u64 {
        0 => stat.f_bsize as u64,
        size => size,
    };
    Some((
        (stat.f_bavail as u64).saturating_mul(block),
        (stat.f_blocks as u64).saturating_mul(block),
    ))
}

/// Unsupported hosts cannot prove filesystem headroom.
#[cfg(not(any(windows, unix)))]
pub fn disk_space(_path: &Path) -> Option<(u64, u64)> {
    None
}

#[cfg(test)]
mod tests;
