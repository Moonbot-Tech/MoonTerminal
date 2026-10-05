//! Synthetic disk state pins retirement across restarts and upgrade seeding.

use super::raise_at;

/// Replacing the watermark with the current config maximum would recycle removed uid 7.
#[test]
fn retirement_and_upgrade_floor_survive_reopening_state() {
    let dir = std::env::temp_dir().join(format!("station-uid-history-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("counter.txt");
    assert_eq!(raise_at(&path, 3).unwrap(), 3);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "3\n");
    assert_eq!(raise_at(&path, 7).unwrap(), 7);
    assert_eq!(raise_at(&path, 3).unwrap(), 7);
    // An upgrade sees reports from uid 12 even though surviving config ends at uid 9.
    assert_eq!(raise_at(&path, 12).unwrap(), 12);
    assert_eq!(raise_at(&path, 9).unwrap(), 12);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "12\n");
    assert_eq!(raise_at(&path, u64::MAX).unwrap(), u64::MAX);
    assert_eq!(raise_at(&path, 9).unwrap(), u64::MAX);
    std::fs::write(&path, "broken\n").unwrap();
    assert!(raise_at(&path, 9).is_err());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "broken\n");
    std::fs::remove_file(&path).unwrap();
    std::fs::remove_dir(&dir).unwrap();
}
