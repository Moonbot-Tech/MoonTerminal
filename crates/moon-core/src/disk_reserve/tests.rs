//! Regression checks for reserve boundaries and physical-to-packed headroom conversion.

use super::*;

const GIB: u64 = 1024 * 1024 * 1024;
const CAP: u64 = 128 * GIB;

/// Omitting either main-file copy, WAL or reserve would admit a VACUUM one byte too early.
#[test]
fn vacuum_room_requires_two_main_files_wal_and_reserve() {
    for (total, floor) in [(20 * GIB, 2 * GIB), (100 * GIB, 10 * GIB)] {
        let needed = floor + 230;
        assert!(!vacuum_room(needed - 1, total, 100, 30));
        assert!(vacuum_room(needed, total, 100, 30));
    }
    assert!(!vacuum_room(u64::MAX, 1, u64::MAX, u64::MAX));
}

/// The wrapper must measure real main/WAL files and allow a failed mount probe.
#[test]
fn vacuum_wrapper_measures_files_and_allows_failed_probe() {
    let dir = VacuumFixture::new();
    let db = dir.0.join("synthetic.sqlite");
    std::fs::write(&db, [0; 100]).unwrap();
    std::fs::write(dir.0.join("synthetic.sqlite-wal"), [0; 30]).unwrap();
    let needed = 2 * GIB + 230;
    assert!(!vacuum_has_room_with(&db, |parent| {
        assert_eq!(parent, dir.0);
        Some((needed - 1, 20 * GIB))
    }));
    assert!(vacuum_has_room_with(&db, |_| Some((needed, 20 * GIB))));
    assert!(vacuum_has_room_with(&db, |_| None));
    assert!(vacuum_has_room(&dir.0.join("missing-parent").join("db")));
}

/// Own a unique temporary directory so wrapper tests never inspect production databases.
struct VacuumFixture(std::path::PathBuf);

impl VacuumFixture {
    /// Create an isolated main/WAL fixture root.
    fn new() -> Self {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("vacuum-room-{}-{stamp}", std::process::id()));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for VacuumFixture {
    /// Remove only this fixture's owned files.
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

/// Changing the share or absolute floor must not reduce the common reserve.
#[test]
fn reserve_covers_small_and_large_filesystems() {
    assert_eq!(reserve(10 * GIB), 2 * GIB);
    assert_eq!(reserve(23 * GIB), 23 * GIB / 10);
    assert_eq!(reserve(200 * GIB), 20 * GIB);
    assert_eq!(spendable(2 * GIB - 1, 10 * GIB), None);
    assert_eq!(spendable(2 * GIB, 10 * GIB), Some(0));
    assert_eq!(spendable(0, 0), Some(u64::MAX));
}

/// Counting physical bytes as packed bytes would over-admit writes on small disks.
#[test]
fn small_disk_uses_packing_density_and_large_disk_hits_cap() {
    let held = 4 * GIB / 5;
    assert_eq!(tape_ceiling(held, GIB, 3 * GIB, 20 * GIB, CAP), 2 * held);
    assert_eq!(tape_ceiling(GIB, GIB, 1000 * GIB, 2000 * GIB, CAP), CAP);
    assert_eq!(tape_ceiling(1, 9, 0, 0, CAP), CAP);
}

/// Subtracting a disk shortfall from held bytes would cascade to empty on repeated probes.
#[test]
fn low_disk_stops_growth_without_cascade() {
    for free in [0, GIB, 2 * GIB] {
        assert_eq!(tape_ceiling(GIB, 2 * GIB, free, 20 * GIB, CAP), GIB);
        assert_eq!(tape_ceiling(GIB / 2, 2 * GIB, free, 20 * GIB, CAP), GIB / 2);
    }
}

/// Empty stores need unit density; very large measurements must not wrap the product or sum.
#[test]
fn empty_and_overflowing_measurements_keep_a_bounded_limit() {
    assert_eq!(tape_ceiling(0, GIB, 3 * GIB, 20 * GIB, CAP), GIB);
    assert_eq!(tape_ceiling(GIB, 0, 3 * GIB, 20 * GIB, CAP), 2 * GIB);
    assert_eq!(tape_ceiling(u64::MAX, 1, u64::MAX, 1, CAP), CAP);
}
