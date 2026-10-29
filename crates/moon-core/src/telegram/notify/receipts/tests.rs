//! Directory-entry durability at the first receipt and its failed-write retry boundary.

use super::*;

/// Skipping directory sync, or advancing after it fails, can lose an acknowledged journal
/// entry on power loss. A retry at the unchanged zero cursor must repeat the sync.
#[test]
fn first_append_requires_directory_sync_and_retries_it_on_failure() {
    let root = std::env::temp_dir().join(format!("moon-receipt-directory-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("notifications.json");
    let receipt = Receipt {
        sequence: 1,
        id: 0,
        chat: 7,
        replaced: None,
        card: None,
    };
    let failed = append_with(&path, 0, &receipt, |journal| {
        assert_eq!(
            journal,
            crate::config::paths::telegram_notify_receipts(&path)
        );
        assert!(std::fs::read(journal)?.ends_with(b"\n"));
        Err(std::io::Error::other("directory sync fixture failure"))
    });
    assert!(failed.is_err());
    let mut synced = false;
    let length = append_with(&path, 0, &receipt, |_| {
        synced = true;
        Ok(())
    })
    .unwrap();
    assert!(synced);
    let receipt = Receipt {
        sequence: 2,
        ..receipt
    };
    append_with(&path, length, &receipt, |_| {
        panic!("existing nonempty journal needs no directory sync")
    })
    .unwrap();
    let mut file = NotifyFile::default();
    replay(&path, &mut file).unwrap();
    assert_eq!(file.ack_sequence, 2);
    std::fs::remove_dir_all(root).unwrap();
}
