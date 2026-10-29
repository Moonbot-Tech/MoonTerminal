//! Chunk-boundary order and bounded copy-on-write regressions.

use super::{CHUNK_ROWS, SnapshotRows};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

/// Count row copies independently of collection metadata.
#[derive(Debug)]
struct Tracked {
    value: usize,
    copies: Arc<AtomicUsize>,
}
impl Clone for Tracked {
    /// Detect a regression to copying every row of a shared publication.
    fn clone(&self) -> Self {
        self.copies.fetch_add(1, Ordering::Relaxed);
        Self {
            value: self.value,
            copies: self.copies.clone(),
        }
    }
}

/// Editing a snapshot must preserve the original and copy exactly one affected chunk.
#[test]
fn edits_copy_one_chunk_and_keep_the_snapshot_unchanged() {
    let copies = Arc::new(AtomicUsize::new(0));
    let original: SnapshotRows<_> = (0..CHUNK_ROWS * 2 + 1)
        .map(|value| Tracked {
            value,
            copies: copies.clone(),
        })
        .collect();
    let mut edited = original.clone();
    assert_eq!(copies.load(Ordering::Relaxed), 0);
    edited[CHUNK_ROWS].value = 900;
    assert_eq!(copies.load(Ordering::Relaxed), CHUNK_ROWS);
    assert_eq!(original[CHUNK_ROWS].value, CHUNK_ROWS);
    assert_eq!(edited[CHUNK_ROWS].value, 900);
    assert!(Arc::ptr_eq(&original.chunks[0], &edited.chunks[0]));
    assert!(Arc::ptr_eq(&original.chunks[2], &edited.chunks[2]));
    edited[CHUNK_ROWS + 1].value = 901;
    assert_eq!(copies.load(Ordering::Relaxed), CHUNK_ROWS);
    assert!(edited.get_mut(CHUNK_ROWS * 2 + 1).is_none());
}

/// Read cursors must keep order and size across full and partial chunk boundaries.
#[test]
fn iterators_preserve_indices_size_and_partial_chunks() {
    let rows: SnapshotRows<_> = (0..CHUNK_ROWS * 2 + 1).collect();
    assert_eq!(
        rows.iter().copied().collect::<Vec<_>>(),
        (0..CHUNK_ROWS * 2 + 1).collect::<Vec<_>>()
    );
    assert_eq!(
        rows.iter().rev().copied().collect::<Vec<_>>(),
        (0..CHUNK_ROWS * 2 + 1).rev().collect::<Vec<_>>()
    );
    let mut read = rows.iter();
    assert_eq!(read.next(), Some(&0));
    assert_eq!(read.next_back(), Some(&(CHUNK_ROWS * 2)));
    assert_eq!(read.len(), CHUNK_ROWS * 2 - 1);
    assert!(rows.get(rows.len()).is_none());
    assert!(SnapshotRows::<usize>::default().is_empty());
}
