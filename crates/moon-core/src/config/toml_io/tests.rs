use super::super::schema::{SettingsFile, default_ui_scale};
use super::{
    ConfigLoad, Memo, atomic_tmp_path, load_or_default, load_or_default_status, memo_hit,
    write_atomic, write_atomic_memo,
};
use std::path::{Path, PathBuf};

/// Holding the process memo during a slow write must not stall an unrelated path's memo check.
#[test]
fn a_slow_writer_does_not_block_another_paths_memo_check() {
    let slow = Scratch::new("slow-writer");
    let other = Scratch::new("other-writer");
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let (checked_tx, checked_rx) = std::sync::mpsc::channel();
    std::thread::scope(|scope| {
        let writer = scope.spawn(move || {
            super::write_atomic_memo_with(
                &slow.file(),
                b"key",
                b"bytes",
                true,
                "test",
                |_, _, _| {
                    started_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                    Ok(())
                },
            )
            .unwrap();
        });
        started_rx.recv().unwrap();
        let checker = scope.spawn(move || {
            super::write_atomic_memo(&other.file(), b"other", b"bytes", true, "test").unwrap();
            checked_tx.send(()).unwrap();
        });
        let checked = checked_rx.recv_timeout(std::time::Duration::from_secs(5));
        release_tx.send(()).unwrap();
        writer.join().unwrap();
        checker.join().unwrap();
        assert!(checked.is_ok(), "another path waited for the slow writer");
    });
}

/// Bind the absent-file branch in [`load_or_default`] to `defaults_for_absent_file`.
///
/// The plausible breakage is treating `defaults_for_absent_file` as a redundant wrapper and
/// replacing it with `T::default()`. This compiles, but `#[serde(default = "...")]` applies
/// only during deserialization, so derived `Default` zeroes fields that have real serde
/// defaults; `ui_scale = 0.0` leaves visible UI without hit targets.
///
/// The oracle is independent of the loader: `schema::default_*` are the same functions used by
/// the fresh-config branch in `config::mod`, so the two paths cannot drift apart.
#[test]
fn an_absent_file_yields_the_schema_defaults_not_zeroes() {
    let missing = PathBuf::from("no-such-dir-4f21c8/no-such-settings.toml");
    assert!(
        !missing.exists(),
        "the fixture path must genuinely not exist"
    );

    let cfg: SettingsFile = load_or_default(&missing, "settings.toml", |_| {
        panic!("on_corrupt must not fire for a file that is merely absent")
    });

    assert_eq!(
        cfg.ui_scale,
        default_ui_scale(),
        "an absent settings.toml must load the schema default scale, not f32::default()"
    );
}

/// An existing but UNREADABLE file must not be treated as absent.
///
/// A construction such as `let Ok(text) = read_to_string(..) else { default }` maps every
/// error, including access denial, a sharing violation, or an unhydrated cloud placeholder, to
/// "first launch." The resulting `version = 0` marks the config dirty and triggers write-back
/// in `AppConfig::load`, which would replace a valid `settings.toml` with defaults.
///
/// The plausible breakage is collapsing the `match` into a shorter `let Ok(..) else`. On a
/// machine where the file reads normally, both versions behave identically and hide the risk.
///
/// A directory is a portable way to obtain a read error other than `NotFound`: no platform
/// reports `NotFound` for an existing path that is not a file.
#[test]
fn an_unreadable_file_is_not_reported_as_absent() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    assert!(dir.is_dir(), "the fixture must be a directory that exists");

    let (_cfg, status) = load_or_default_status::<SettingsFile>(&dir, "settings.toml", |_| {
        panic!("on_corrupt must not fire for a file that could not be read at all")
    });

    assert_eq!(
        status,
        ConfigLoad::Unreadable,
        "a path that exists but does not read must report Unreadable, never Absent"
    );
}

/// A genuinely missing file reports `Absent` so first launch can save defaults.
#[test]
fn a_missing_file_reports_absent() {
    let missing = Path::new("no-such-dir-4f21c8/no-such-settings.toml");
    assert!(
        !missing.exists(),
        "the fixture path must genuinely not exist"
    );

    let (_cfg, status) = load_or_default_status::<SettingsFile>(missing, "settings.toml", |_| {
        panic!("on_corrupt must not fire for a file that is merely absent")
    });

    assert_eq!(status, ConfigLoad::Absent);
}

/// A fresh scratch file path per test; the directory is removed by [`Scratch`]'s drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("moon-memo-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn file(&self) -> PathBuf {
        self.0.join("cfg.json")
    }

    /// A directory squatting on the temp path makes every REAL write fail, so an `Ok` while it
    /// stands proves the write was skipped.
    fn block_writes(&self) {
        std::fs::create_dir_all(atomic_tmp_path(&self.file())).unwrap();
    }

    fn unblock_writes(&self) {
        std::fs::remove_dir_all(atomic_tmp_path(&self.file())).unwrap();
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn an_identical_save_is_skipped() {
    let s = Scratch::new("same");
    write_atomic(&s.file(), b"alpha", "t").unwrap();
    s.block_writes();
    assert!(
        write_atomic(&s.file(), b"alpha", "t").is_ok(),
        "equal bytes must not touch disk"
    );
}

#[test]
fn changed_content_is_written() {
    let s = Scratch::new("changed");
    write_atomic(&s.file(), b"alpha", "t").unwrap();
    s.block_writes();
    assert!(
        write_atomic(&s.file(), b"beta!", "t").is_err(),
        "changed bytes must be written"
    );
    s.unblock_writes();
    write_atomic(&s.file(), b"beta!", "t").unwrap();
    assert_eq!(std::fs::read(s.file()).unwrap(), b"beta!");
}

#[test]
fn an_external_modify_or_delete_forces_the_rewrite() {
    let s = Scratch::new("external");
    write_atomic(&s.file(), b"alpha", "t").unwrap();
    std::fs::write(s.file(), b"edited by hand").unwrap();
    write_atomic(&s.file(), b"alpha", "t").unwrap();
    assert_eq!(std::fs::read(s.file()).unwrap(), b"alpha");

    std::fs::remove_file(s.file()).unwrap();
    write_atomic(&s.file(), b"alpha", "t").unwrap();
    assert_eq!(std::fs::read(s.file()).unwrap(), b"alpha");
}

/// Comparing stamps instead of bytes would silently keep a foreign same-length rewrite.
#[test]
fn a_foreign_rewrite_with_the_same_length_and_mtime_is_overwritten() {
    let s = Scratch::new("same-stamp");
    write_atomic(&s.file(), b"alpha", "test").unwrap();
    let modified = std::fs::metadata(s.file()).unwrap().modified().unwrap();
    std::fs::write(s.file(), b"beta!").unwrap();
    std::fs::OpenOptions::new()
        .write(true)
        .open(s.file())
        .unwrap()
        .set_modified(modified)
        .unwrap();
    assert_eq!(
        std::fs::metadata(s.file()).unwrap().modified().unwrap(),
        modified
    );
    write_atomic(&s.file(), b"alpha", "test").unwrap();
    assert_eq!(std::fs::read(s.file()).unwrap(), b"alpha");
}

/// Skipping must be based on disk bytes even when this process has never saved the file.
#[test]
fn identical_bytes_skip_without_a_memo() {
    let s = Scratch::new("no-memo");
    std::fs::write(s.file(), b"alpha").unwrap();
    s.block_writes();
    write_atomic(&s.file(), b"alpha", "test").unwrap();
}

/// A failed encrypted save must invalidate the prior plaintext memo and retry on the next call.
#[test]
fn a_failed_write_clears_the_memo() {
    let s = Scratch::new("failed");
    write_atomic_memo(&s.file(), b"alpha", b"cipher-a", true, "t").unwrap();
    s.block_writes();
    assert!(write_atomic_memo(&s.file(), b"beta!", b"cipher-b", true, "t").is_err());
    assert!(write_atomic_memo(&s.file(), b"alpha", b"cipher-c", true, "t").is_err());
    s.unblock_writes();
    write_atomic_memo(&s.file(), b"beta!", b"cipher-d", true, "t").unwrap();
    assert_eq!(std::fs::read(s.file()).unwrap(), b"cipher-d");
}

/// Fresh encryption nonces may skip unchanged plaintext, but pending key slots must reach disk.
#[test]
fn encrypted_memo_skips_nonce_changes_but_not_forced_or_deleted_writes() {
    let s = Scratch::new("encrypted");
    write_atomic_memo(&s.file(), b"alpha", b"cipher-a", true, "t").unwrap();
    s.block_writes();
    write_atomic_memo(&s.file(), b"alpha", b"cipher-b", true, "t").unwrap();
    assert!(write_atomic_memo(&s.file(), b"alpha", b"cipher-c", false, "t").is_err());
    s.unblock_writes();
    write_atomic_memo(&s.file(), b"alpha", b"cipher-d", false, "t").unwrap();
    assert_eq!(std::fs::read(s.file()).unwrap(), b"cipher-d");
    std::fs::remove_file(s.file()).unwrap();
    write_atomic_memo(&s.file(), b"alpha", b"cipher-e", true, "t").unwrap();
    assert_eq!(std::fs::read(s.file()).unwrap(), b"cipher-e");
}

#[test]
fn memo_hit_needs_equal_content_and_an_untouched_file() {
    let t = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(7);
    let memo = Memo {
        hash: 1,
        key_len: 5,
        file_len: 9,
        mtime: Some(t),
    };
    assert!(memo_hit(Some(&memo), 1, 5, Some((9, Some(t)))));
    assert!(!memo_hit(None, 1, 5, Some((9, Some(t)))));
    assert!(!memo_hit(Some(&memo), 2, 5, Some((9, Some(t)))));
    assert!(!memo_hit(Some(&memo), 1, 5, Some((10, Some(t)))));
    assert!(!memo_hit(
        Some(&memo),
        1,
        5,
        Some((9, Some(t + std::time::Duration::from_secs(1))))
    ));
    assert!(!memo_hit(Some(&memo), 1, 5, None));
    assert!(!memo_hit(Some(&memo), 1, 5, Some((9, None))));
}
