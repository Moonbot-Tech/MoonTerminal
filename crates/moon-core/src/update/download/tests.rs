//! Regression tests for bounded verified download staging.

use std::fs;
#[cfg(windows)]
use std::fs::OpenOptions;
use std::io::Write as _;
#[cfg(windows)]
use std::os::windows::fs::OpenOptionsExt as _;
use std::path::PathBuf;

use super::*;
use crate::update::release::parse_sha256;

/// Removing streamed digest enforcement would allow corrupted bytes to reach the executable
/// staging path. This oracle uses the published SHA-256 test vector for `abc`.
#[test]
fn streamed_download_rejects_a_digest_mismatch() {
    let expected =
        parse_sha256("sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad")
            .unwrap();
    let mut written = Vec::new();
    copy_verified(&b"abc"[..], &mut written, 3, &expected).unwrap();
    assert_eq!(written, b"abc");

    let mut corrupted = expected;
    corrupted[0] ^= 0xff;
    let error = copy_verified(&b"abc"[..], Vec::new(), 3, &corrupted).unwrap_err();
    assert!(error.to_string().contains("SHA-256"));
    assert!(copy_verified(&b"abc"[..], Vec::new(), 4, &expected).is_err());
    assert!(copy_verified(&b"abc"[..], Vec::new(), 2, &expected).is_err());
}

/// A failed post-sync verification must remove its part file, while success promotes only the
/// independently rehashed bytes from the still-exclusive open handle.
#[test]
fn part_guard_cleans_failures_and_promotion_keeps_verified_bytes() {
    let root = test_root("part-promotion");
    if root.exists() {
        fs::remove_dir_all(&root).unwrap();
    }
    fs::create_dir_all(&root).unwrap();
    let staged = root.join("MoonTerminal.exe");
    let expected =
        parse_sha256("sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad")
            .unwrap();
    let (path, mut file) = create_unique_part(&staged).unwrap();
    file.write_all(b"bad").unwrap();
    let mut part = PartFile::new(path.clone(), file);
    let part_path = part.path().to_path_buf();
    assert!(promote_verified_part(&part_path, part.file_mut(), &staged, 3, &expected).is_err());
    drop(part);
    assert!(!path.exists());
    assert!(!staged.exists());

    let (path, mut file) = create_unique_part(&staged).unwrap();
    file.write_all(b"abc").unwrap();
    let mut part = PartFile::new(path, file);
    let part_path = part.path().to_path_buf();
    promote_verified_part(&part_path, part.file_mut(), &staged, 3, &expected).unwrap();
    part.disarm();
    assert_eq!(fs::read(&staged).unwrap(), b"abc");
    assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
    fs::remove_dir_all(root).unwrap();
}

/// Reopening the part path for the post-sync hash would fail while this handle deliberately
/// denies shared reads, but hashing the supplied handle can still promote the verified bytes.
#[cfg(windows)]
#[test]
fn post_sync_hash_uses_the_supplied_handle_without_reopening_the_path() {
    const FILE_SHARE_WRITE: u32 = 0x0000_0002;
    const FILE_SHARE_DELETE: u32 = 0x0000_0004;

    let root = test_root("same-handle");
    if root.exists() {
        fs::remove_dir_all(&root).unwrap();
    }
    fs::create_dir_all(&root).unwrap();
    let part_path = root.join("MoonTerminal.exe.part");
    let staged = root.join("MoonTerminal.exe");
    let expected =
        parse_sha256("sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad")
            .unwrap();
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .share_mode(FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .open(&part_path)
        .unwrap();
    file.write_all(b"abc").unwrap();

    assert!(fs::File::open(&part_path).is_err());
    promote_verified_part(&part_path, &mut file, &staged, 3, &expected).unwrap();

    drop(file);
    assert_eq!(fs::read(&staged).unwrap(), b"abc");
    fs::remove_dir_all(root).unwrap();
}

/// Allocate a process-local temporary directory outside every production path constructor.
fn test_root(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "moonterminal-update-{label}-{}",
        std::process::id()
    ))
}
