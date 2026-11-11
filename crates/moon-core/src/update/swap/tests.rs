//! Regression tests for the Linux executable swap and parent-process binding.

use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use super::{
    ParentProcess, copy_into_new_file, mark_executable, replace_keeping_backup, restore_backup,
};

/// A fresh, empty directory for one test.
fn test_root(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "moon-core-update-swap-{name}-{}",
        std::process::id()
    ));
    if root.exists() {
        fs::remove_dir_all(&root).unwrap();
    }
    fs::create_dir_all(&root).unwrap();
    root
}

/// Write a file with an exact mode, independent of the process umask.
fn write_with_mode(path: &Path, bytes: &[u8], mode: u32) {
    fs::write(path, bytes).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}

/// A long-lived child whose image is a file this test controls.
///
/// A private copy of `sleep`, so the image's inode belongs to the test and a second copy is a
/// different file with the same bytes.
fn sleeper(root: &Path, name: &str) -> (PathBuf, Child) {
    let image = root.join(name);
    fs::copy("/bin/sleep", &image).unwrap();
    // Tests run on parallel threads, and several of them fork. A child forked by another test
    // while `fs::copy` above still had the image open for writing keeps that descriptor until
    // its own exec, and the kernel will not execute a file somebody holds open for writing
    // ("Text file busy"). The holder is gone within moments, so that one error is retried; any
    // other one, or a holder that never goes away, still fails the test.
    let mut attempts = 0;
    let mut child = loop {
        let spawned = Command::new(&image)
            .arg("300")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        match spawned {
            Ok(child) => break child,
            Err(error)
                if error.kind() == std::io::ErrorKind::ExecutableFileBusy && attempts < 500 =>
            {
                attempts += 1;
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(error) => panic!("spawn {}: {error}", image.display()),
        }
    };
    // `spawn` returns once the child has begun its exec, and for a moment after that
    // `/proc/<pid>/exe` still names the image the child was forked from — this test binary.
    // The tests below are about a process that RUNS the image, so wait until it does. The real
    // helper never meets this: the parent it binds has been running since the user started it.
    let wanted = fs::metadata(&image).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let running = fs::metadata(format!("/proc/{}/exe", child.id()));
        if running.is_ok_and(|exe| exe.dev() == wanted.dev() && exe.ino() == wanted.ino()) {
            break;
        }
        if Instant::now() >= deadline {
            // The caller never receives this child, so nothing else would stop it.
            let _ = child.kill();
            let _ = child.wait();
            panic!("the child never started running {}", image.display());
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    (image, child)
}

/// Dropping the mode carry-over leaves the installed path naming a file nobody can execute: the
/// download is created `0644`, and the next start of the terminal fails with "permission
/// denied". Swapping the link and the rename would leave no backup to roll back to.
#[test]
fn replacement_installs_the_new_bytes_with_the_old_mode_and_keeps_the_old_file() {
    let root = test_root("replace");
    let target = root.join("moonterminal");
    let incoming = root.join("incoming");
    let backup = root.join("backup");
    write_with_mode(&target, b"old executable", 0o750);
    write_with_mode(&incoming, b"new executable", 0o644);
    let old_inode = fs::metadata(&target).unwrap().ino();

    replace_keeping_backup(&target, &incoming, &backup).unwrap();

    assert_eq!(fs::read(&target).unwrap(), b"new executable");
    assert_eq!(fs::metadata(&target).unwrap().mode() & 0o7777, 0o750);
    assert_eq!(fs::read(&backup).unwrap(), b"old executable");
    assert_eq!(
        fs::metadata(&backup).unwrap().ino(),
        old_inode,
        "the backup must be the previous file itself, not a rewritten copy"
    );
    assert!(
        !incoming.exists(),
        "the new file must have moved, not been copied"
    );
    fs::remove_dir_all(&root).unwrap();
}

/// A backup path that already exists is somebody else's file. Overwriting it would destroy the
/// only copy of a previous executable, or follow a planted link; the swap must refuse and leave
/// the install exactly as it was.
#[test]
fn replacement_refuses_an_existing_backup_and_leaves_the_install_untouched() {
    let root = test_root("occupied-backup");
    let target = root.join("moonterminal");
    let incoming = root.join("incoming");
    let backup = root.join("backup");
    write_with_mode(&target, b"old executable", 0o755);
    write_with_mode(&incoming, b"new executable", 0o644);
    write_with_mode(&backup, b"somebody else's file", 0o600);

    assert!(replace_keeping_backup(&target, &incoming, &backup).is_err());

    assert_eq!(fs::read(&target).unwrap(), b"old executable");
    assert_eq!(fs::read(&backup).unwrap(), b"somebody else's file");
    assert_eq!(fs::read(&incoming).unwrap(), b"new executable");
    fs::remove_dir_all(&root).unwrap();
}

/// Rollback must work in both states a failed update can leave: the new file in place, and no
/// file in place at all. A restore that only handled the first would strand the second with no
/// executable.
#[test]
fn restore_puts_the_previous_executable_back_over_a_present_or_missing_target() {
    let root = test_root("restore");
    let target = root.join("moonterminal");
    let incoming = root.join("incoming");
    let backup = root.join("backup");
    write_with_mode(&target, b"old executable", 0o755);
    write_with_mode(&incoming, b"new executable", 0o644);
    replace_keeping_backup(&target, &incoming, &backup).unwrap();

    restore_backup(&target, &backup).unwrap();
    assert_eq!(fs::read(&target).unwrap(), b"old executable");
    assert_eq!(fs::metadata(&target).unwrap().mode() & 0o7777, 0o755);
    assert!(!backup.exists());

    fs::rename(&target, &backup).unwrap();
    restore_backup(&target, &backup).unwrap();
    assert_eq!(fs::read(&target).unwrap(), b"old executable");
    fs::remove_dir_all(&root).unwrap();
}

/// A replacement that fails after the backup link exists leaves the backup and the install as two
/// names for one file. A rename between them reports success and removes nothing, so without the
/// identity check the "restored" backup would stay behind in the transaction directory.
#[test]
fn restore_drops_a_backup_that_is_the_installed_file_itself() {
    let root = test_root("restore-same-file");
    let target = root.join("moonterminal");
    let backup = root.join("backup");
    write_with_mode(&target, b"old executable", 0o755);
    fs::hard_link(&target, &backup).unwrap();

    restore_backup(&target, &backup).unwrap();

    assert_eq!(fs::read(&target).unwrap(), b"old executable");
    assert!(!backup.exists(), "the redundant second name must be gone");
    fs::remove_dir_all(&root).unwrap();
}

/// On a filesystem without hard links the backup is a copy, and recovery puts any file found
/// under the backup's name back over the install. Writing that copy in place would let a copy
/// that failed halfway leave a truncated file there, and recovery would destroy an install that
/// was still whole.
#[test]
fn a_copied_backup_appears_whole_or_not_at_all() {
    let root = test_root("copy-backup");
    let source = root.join("moonterminal");
    let backup = root.join("backup");
    let part = root.join("backup.part");
    write_with_mode(&source, b"old executable", 0o644);

    copy_into_new_file(&source, &backup, 0o750).unwrap();
    assert_eq!(fs::read(&backup).unwrap(), b"old executable");
    assert_eq!(fs::metadata(&backup).unwrap().mode() & 0o7777, 0o750);
    assert!(!part.exists(), "the working file must not outlive the copy");

    // A copy that fails after its working file exists: the source cannot be read.
    fs::remove_file(&backup).unwrap();
    assert!(copy_into_new_file(&root.join("missing"), &backup, 0o750).is_err());
    assert!(!backup.exists(), "a failed copy must publish nothing");
    assert!(
        !part.exists(),
        "a failed copy must remove the working file it created"
    );

    // A working name that is already taken belongs to someone else: refused, and left as it was.
    write_with_mode(&part, b"somebody else's file", 0o600);
    assert!(copy_into_new_file(&source, &backup, 0o750).is_err());
    assert!(!backup.exists());
    assert_eq!(fs::read(&part).unwrap(), b"somebody else's file");
    fs::remove_dir_all(&root).unwrap();
}

/// The staged download is created without execute bits and is then run as the helper. Without
/// this step the update fails at its first spawn on every Linux machine; granting execute to
/// classes that cannot read would widen a private download.
#[test]
fn a_download_becomes_runnable_only_for_those_who_can_read_it() {
    let root = test_root("mode");
    let shared = root.join("shared");
    let private = root.join("private");
    write_with_mode(&shared, b"#!/bin/sh\n", 0o644);
    write_with_mode(&private, b"#!/bin/sh\n", 0o600);

    mark_executable(&shared).unwrap();
    mark_executable(&private).unwrap();

    assert_eq!(fs::metadata(&shared).unwrap().mode() & 0o7777, 0o755);
    assert_eq!(fs::metadata(&private).unwrap().mode() & 0o7777, 0o700);
    fs::remove_dir_all(&root).unwrap();
}

/// The binding is what stops a forged manifest from making the helper wait on, and then replace
/// the executable of, a process that is not the terminal. Comparing paths or bytes instead of
/// the file identity would accept the look-alike here: same name pattern, same contents.
#[test]
fn a_parent_is_bound_only_when_its_image_is_the_installed_file() {
    let root = test_root("bind");
    let (image, mut child) = sleeper(&root, "installed");
    let look_alike = root.join("look-alike");
    fs::copy(&image, &look_alike).unwrap();

    let wrong = ParentProcess::open(child.id(), &look_alike);
    let right = ParentProcess::open(child.id(), &image);

    child.kill().unwrap();
    child.wait().unwrap();
    assert!(
        wrong.is_err(),
        "a process running another file must not be bound"
    );
    assert!(
        right.is_ok(),
        "the process running the installed file binds"
    );
    fs::remove_dir_all(&root).unwrap();
}

/// The helper replaces the executable only after the old application is gone. A wait that
/// returned early would swap the file under a running terminal; one that never returned would
/// leave the update hanging with the old application already closed.
#[test]
fn waiting_returns_once_the_parent_has_exited_and_not_before() {
    let root = test_root("wait");
    let (image, mut child) = sleeper(&root, "installed");
    let parent = ParentProcess::open(child.id(), &image).unwrap();
    assert!(
        !parent.has_exited().unwrap(),
        "a running parent must not read as exited"
    );

    child.kill().unwrap();
    let started = Instant::now();
    parent.wait_for_exit().unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "the wait must end when the process does"
    );
    child.wait().unwrap();
    fs::remove_dir_all(&root).unwrap();
}

/// A pid that names nobody must fail to bind rather than be waited on: the manifest's pid is
/// untrusted input, and a helper that "waited" on a dead pid would proceed straight to replacing
/// the executable.
#[test]
fn a_process_that_already_exited_cannot_be_bound() {
    let root = test_root("gone");
    let (image, mut child) = sleeper(&root, "installed");
    let pid = child.id();
    child.kill().unwrap();
    child.wait().unwrap();

    assert!(ParentProcess::open(pid, &image).is_err());
    fs::remove_dir_all(&root).unwrap();
}
