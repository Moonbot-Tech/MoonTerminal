//! Independent clone-edit parity for durable incremental notification transactions.

use super::*;

/// Losing removals, empty cores, unchanged trades or FIFO order corrupts announce-once replay.
#[test]
fn patch_matches_frozen_full_document_replacement() {
    for duplicate in [false, true] {
        let mut old = NotifyFile::default();
        let mut chat = ChatNotify::default();
        chat.ledger
            .seen
            .insert(1, BTreeMap::from([(1, 10), (2, 20)]));
        chat.ledger.held.insert(2, BTreeMap::from([(3, 30)]));
        chat.ledger
            .cards
            .insert(1, BTreeMap::from([(4, CardWait::default())]));
        chat.ledger.charts.seen = chat.ledger.seen.clone();
        chat.ledger.charts.held = chat.ledger.held.clone();
        old.chats.insert(7, chat);
        old.chats.insert(8, ChatNotify::default());
        for id in [1, 2, if duplicate { 2 } else { 3 }] {
            old.outbox.push(Pending {
                id,
                chat: 7,
                html: format!("card-{id}"),
                ..Default::default()
            });
        }
        let mut expected = old.clone();
        expected.chats.remove(&8);
        expected.chats.insert(9, ChatNotify::default());
        let chat = expected.chats.get_mut(&7).unwrap();
        chat.settings.down.on = true;
        chat.revision = 2;
        chat.ledger.down_announced.insert(1);
        chat.ledger.seen.get_mut(&1).unwrap().remove(&1);
        chat.ledger.seen.get_mut(&1).unwrap().insert(3, 31);
        chat.ledger.held.remove(&2);
        chat.ledger.held.insert(5, BTreeMap::new());
        chat.ledger
            .cards
            .get_mut(&1)
            .unwrap()
            .get_mut(&4)
            .unwrap()
            .message = Some(42);
        chat.ledger.charts.seen.clear();
        chat.ledger.charts.held.get_mut(&2).unwrap().insert(6, 60);
        expected.outbox.remove_id(1);
        expected.outbox.last_mut().unwrap().html = "edited".into();
        expected.outbox.push(Pending {
            id: 5,
            chat: 8,
            ..Default::default()
        });
        expected.next_id = 6;
        let encoded = serde_json::to_vec(&Update::between(&old, &expected)).unwrap();
        serde_json::from_slice::<Update>(&encoded)
            .unwrap()
            .apply(&mut old)
            .unwrap();
        assert_eq!(old, expected);
    }
}
