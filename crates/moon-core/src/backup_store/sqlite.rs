//! Name grammar and completion check shared by the single-SQLite-file backup domains.
//!
//! The strategy database and the order-trace archive both snapshot one SQLite file plus a
//! versioned completion marker, under the same scheduled and manual directory names. Each domain
//! keeps its own file name and marker contents and passes them in.

use std::path::Path;
use std::{fs::File, io::Read as _};

use super::timestamp_prefix;

/// Return the fixed timestamp portion of an application-owned snapshot directory name.
pub(crate) fn snapshot_stamp(name: &str) -> Option<&str> {
    let stamp = timestamp_prefix(name)?;
    let digits = |s: &str| s.bytes().all(|byte| byte.is_ascii_digit());
    let valid_suffix = match name.len() {
        15 => true,
        22 => name.get(15..) == Some("-manual"),
        25 => name.get(15..23) == Some("-manual-") && name.get(23..).is_some_and(digits),
        _ => false,
    };
    valid_suffix.then_some(stamp)
}

/// Return whether a regular file begins with SQLite's fixed 16-byte format signature.
fn has_sqlite_header(path: &Path) -> bool {
    let mut header = [0u8; 16];
    File::open(path)
        .and_then(|mut file| file.read_exact(&mut header))
        .is_ok()
        && &header == b"SQLite format 3\0"
}

/// Return whether `dir` is a completed snapshot with a nonempty regular database file.
///
/// Args:
///     dir: Candidate snapshot directory.
///     database_name: Name of the SQLite file inside the snapshot.
///     marker_name: Name of the completion marker inside the snapshot.
///     marker_content: Exact bytes the marker must hold.
pub(crate) fn is_completed_snapshot(
    dir: &Path,
    database_name: &str,
    marker_name: &str,
    marker_content: &[u8],
) -> bool {
    let Ok(dir_meta) = std::fs::symlink_metadata(dir) else {
        return false;
    };
    if !dir_meta.is_dir() || dir_meta.file_type().is_symlink() {
        return false;
    }
    let database = dir.join(database_name);
    let valid_database = std::fs::symlink_metadata(&database)
        .map(|meta| meta.is_file() && !meta.file_type().is_symlink() && meta.len() >= 16)
        .unwrap_or(false)
        && has_sqlite_header(&database);
    let marker = dir.join(marker_name);
    let valid_marker = std::fs::symlink_metadata(&marker)
        .map(|meta| meta.is_file() && !meta.file_type().is_symlink())
        .unwrap_or(false)
        && std::fs::read(marker)
            .map(|bytes| bytes == marker_content)
            .unwrap_or(false);
    valid_database && valid_marker
}
