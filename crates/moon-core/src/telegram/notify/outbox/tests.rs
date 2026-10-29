//! FIFO parity and complete-drain scaling, using neutral synthetic cards.

use super::*;
use std::mem::size_of;

/// Alternating chats and non-monotonic ids distinguish FIFO order from sorted ids.
fn rows(count: usize) -> Vec<Pending> {
    (0..count)
        .map(|index| Pending {
            id: (count - index) as u64,
            chat: index as i64 % 10 + 1,
            html: "XALPHA ".repeat(64),
            cores: Some(vec![1]),
            ..Default::default()
        })
        .collect()
}

/// Removing arbitrary ids must preserve the old Vec row set, order and JSON array contract.
#[test]
fn indexed_fifo_matches_the_frozen_vec_operations() {
    let mut old = rows(17);
    let mut new = Outbox::from(old.clone());
    for id in [3, 17, 9, 2, 99] {
        old.retain(|row| row.id != id);
        new.remove_id(id);
        assert_eq!(new.iter().cloned().collect::<Vec<_>>(), old);
        assert_eq!(
            serde_json::to_value(&new).unwrap(),
            serde_json::to_value(&old).unwrap()
        );
        let mut chats = std::collections::HashSet::new();
        let expected: Vec<_> = old.iter().filter(|row| chats.insert(row.chat)).collect();
        assert_eq!(new.heads(), expected);
    }
    let added = Pending {
        id: 100,
        chat: 1,
        html: "UNIT_01".into(),
        ..Default::default()
    };
    old.push(added.clone());
    new.push(added);
    old.retain(|row| row.chat != 2);
    new.retain(|row| row.chat != 2);
    assert_eq!(new.into_iter().collect::<Vec<_>>(), old);
}

/// A legacy file containing duplicate ids must keep its rows until the old ack's retain would
/// remove all matches; indexing by id alone must never silently overwrite a queued card.
#[test]
fn duplicate_ids_and_shared_photos_keep_the_vec_contract() {
    let old = vec![
        Pending {
            id: 3,
            chat: 1,
            photo: Some("shared.png".into()),
            ..Default::default()
        },
        Pending {
            id: 4,
            chat: 2,
            photo: Some("shared.png".into()),
            ..Default::default()
        },
        Pending {
            id: 3,
            chat: 1,
            ..Default::default()
        },
    ];
    let json = serde_json::to_string(&old).unwrap();
    let mut new: Outbox = serde_json::from_str(&json).unwrap();
    assert_eq!(new.len(), 3);
    new.remove_id(3);
    assert_eq!(new.iter().map(|row| row.id).collect::<Vec<_>>(), vec![4]);
    assert!(new.names_photo("shared.png"));
    new.remove_id(4);
    assert!(!new.names_photo("shared.png"));
    assert!(new.is_empty());
}

/// Measure a whole drain at 10000 cards, not just one cheap lookup. Allocation estimates include
/// the live payloads, hash entries and id vectors; allocator bookkeeping is not included.
#[test]
#[ignore = "scale probe"]
fn probe_notify_complete_drain() {
    for count in [1_000, 10_000] {
        let mut old = rows(count);
        let mut new = Outbox::from(old.clone());
        let payload: usize = new
            .rows
            .values()
            .map(|row| {
                row.value.html.capacity()
                    + row
                        .value
                        .cores
                        .as_ref()
                        .map_or(0, |ids| ids.capacity() * size_of::<u64>())
            })
            .sum();
        let containers = new.rows.capacity() * (size_of::<(u64, Row)>() + 1)
            + new.ids.capacity() * (size_of::<(u64, Vec<u64>)>() + 1)
            + new
                .ids
                .values()
                .map(|ids| ids.capacity() * size_of::<u64>())
                .sum::<usize>()
            + (new.heads.capacity() + new.tails.capacity()) * (size_of::<(i64, u64)>() + 1);
        eprintln!(
            "probe queued memory {count}: payload={payload}, indexed entries={containers}, total={} bytes (allocator overhead excluded)",
            payload + containers
        );
        let start = std::time::Instant::now();
        for id in (1..=count as u64).rev() {
            old.retain(|row| row.id != id);
        }
        let before = start.elapsed().as_secs_f64() * 1_000.0;
        let start = std::time::Instant::now();
        while !new.is_empty() {
            let heads: Vec<_> = new.heads().into_iter().map(|row| row.id).collect();
            for id in heads {
                new.remove_id(id);
            }
        }
        let after = start.elapsed().as_secs_f64() * 1_000.0;
        eprintln!(
            "probe complete drain {count}: Vec retain {before:.3} -> indexed heads {after:.3} ms"
        );
        assert!(old.is_empty());
        assert!(new.is_empty());
    }
}
