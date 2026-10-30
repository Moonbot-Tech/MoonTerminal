//! Full-document decision parity and legacy journal compatibility for candidate-only batches.

use super::*;
use crate::telegram::notify::trades::{ClosedTrade, SEEN_WINDOW_SECS, decide, decide_charts};
use crate::telegram::runtime::{push_edit, push_outbox};

/// Omitting history outside the candidate set must preserve it, while expiry and rule-off
/// clearing still remove all eligible leaves; existing queued cards must keep their FIFO order.
#[test]
fn candidate_patch_matches_full_document_decisions_and_queue_supersession() {
    for on in [false, true] {
        let now = 1_000_000;
        let mut source = NotifyFile::default();
        let mut entry = ChatNotify::default();
        entry.settings.trades.on = on;
        entry.settings.charts.on = on;
        entry.ledger.trades_enabled_utc = Some(0);
        entry.ledger.charts.enabled_utc = Some(0);
        entry
            .ledger
            .seen
            .insert(1, BTreeMap::from([(1, now), (2, 1), (3, now)]));
        entry.ledger.seen.insert(2, BTreeMap::new());
        entry.ledger.charts.seen = entry.ledger.seen.clone();
        entry
            .ledger
            .held
            .insert(1, BTreeMap::from([(8, now), (9, now)]));
        source.chats.insert(7, entry);
        push_outbox(&mut source, 7, "mandatory".into(), None, now);
        push_edit(&mut source, 7, 42, "old state".into(), None, now);
        let trades = vec![
            ClosedTrade {
                core: 1,
                rec_id: 1,
                close_utc: now,
                ..Default::default()
            },
            ClosedTrade {
                core: 1,
                rec_id: 8,
                close_utc: now,
                profit_usd: Some(5.0),
                ..Default::default()
            },
        ];
        let edit = |file: &mut NotifyFile| {
            let entry = file.chats.get_mut(&7).unwrap();
            let announced = decide(
                &entry.settings.trades,
                &mut entry.ledger,
                &[1],
                &trades,
                now,
            );
            decide_charts(
                &entry.settings.charts,
                &mut entry.ledger.charts,
                &[1],
                &trades,
                now,
            );
            for row in announced {
                push_outbox(file, 7, format!("card {}", row.trade.rec_id), None, now);
            }
            push_edit(file, 7, 42, "new state".into(), None, now);
        };
        let mut expected = source.clone();
        edit(&mut expected);
        let keys: Vec<_> = trades.iter().map(ClosedTrade::key).collect();
        let mut batch = NotificationBatch::new(&source, &[7], &keys, Some(now - SEEN_WINDOW_SECS));
        assert_eq!(
            batch.file.chats[&7]
                .ledger
                .seen
                .values()
                .map(BTreeMap::len)
                .sum::<usize>(),
            1
        );
        assert!(batch.file.outbox.is_empty());
        edit(&mut batch.file);
        let encoded = serde_json::to_vec(&batch.finish()).unwrap();
        let mut actual = source;
        serde_json::from_slice::<Update>(&encoded)
            .unwrap()
            .apply(&mut actual)
            .unwrap();
        assert_eq!(actual, expected, "rule on={on}");
    }
}

/// A pre-upgrade transaction has neither pruning fields nor append-only queue variants.
#[test]
fn legacy_owner_transaction_still_deserializes_and_applies() {
    let encoded = br#"{"chats":[],"queue":{"Ordered":{"order":[3],"changed":[{"id":3,"chat":7,"html":"legacy"}]}},"next_id":4}"#;
    let mut file = NotifyFile::default();
    serde_json::from_slice::<Update>(encoded)
        .unwrap()
        .apply(&mut file)
        .unwrap();
    assert_eq!(file.outbox[0].html, "legacy");
    assert_eq!(file.next_id, 4);
}
