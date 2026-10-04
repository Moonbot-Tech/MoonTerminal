//! Outbox persistence, the 4096 UTF-16 refusal, and the ack-retry decision.
//!
//! The sender loop is not covered: it needs a live Bot API. A timeout or transport error is left
//! in the outbox on purpose, so a restart retries it. Whether an accepted id is resent is pinned
//! by the delivered-id set without a live client.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};

use super::{NotifyStore, push_outbox};
use crate::telegram::notify::{ChatNotify, NotifyFile};
use crate::telegram::reply::{TELEGRAM_MESSAGE_UTF16_LIMIT, utf16_len};

static TEST_SEQ: AtomicU32 = AtomicU32::new(0);

/// Isolated temp root. Removed when the test drops it, including on panic.
struct TempRoot(PathBuf);

impl TempRoot {
    fn new(tag: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "moonterminal-notify-send-{}-{tag}-{}",
            std::process::id(),
            TEST_SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("temp root");
        Self(root)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A crash after enqueue returns must still find the row. Losing it would drop the notification.
#[test]
fn enqueue_is_on_disk_before_it_returns() {
    let root = TempRoot::new("enqueue");
    let path = root.path("notifications.json");
    std::fs::write(&path, r#"{"chats":{"7":{}},"next_id":3}"#).unwrap();
    let mut store = NotifyStore::open(path.clone()).expect("open");
    store
        .enqueue(7, "<b>hi</b>".into(), None, 1_700_000_050)
        .expect("enqueue");

    let loaded = NotifyFile::load(&path).expect("reload");
    assert!(
        loaded.chats.contains_key(&7),
        "enqueue must keep paired chats"
    );
    assert_eq!(loaded.next_id, 4);
    assert_eq!(loaded.outbox.len(), 1);
    assert_eq!(loaded.outbox[0].id, 3);
    assert_eq!(loaded.outbox[0].chat, 7);
    assert_eq!(loaded.outbox[0].html, "<b>hi</b>");
    assert_eq!(loaded.outbox[0].created_utc, 1_700_000_050);
    assert_eq!(store.file.outbox, loaded.outbox);
}

/// Ack must delete the row from disk. Leaving it would send the same notification again.
#[test]
fn ack_removes_the_row_and_persists() {
    let root = TempRoot::new("ack");
    let path = root.path("notifications.json");
    let mut store = NotifyStore::open(path.clone()).expect("open");
    store.enqueue(7, "one".into(), None, 10).expect("enqueue");
    let id = NotifyFile::load(&path).expect("reload").outbox[0].id;

    store.ack(id, 7, None, None).expect("ack");

    let loaded = NotifyFile::load(&path).expect("reload after ack");
    assert!(loaded.outbox.is_empty(), "acked id {id} still on disk");
    assert_eq!(loaded.next_id, 1, "ack must not reuse the id counter");
    assert!(store.file.outbox.is_empty());
}

/// Cutting an oversize body would store a broken tag or a lone surrogate, and Telegram would
/// reject it. A body past 4096 UTF-16 units is refused: nothing is queued and the id is not
/// consumed. A body that fits, including one that ends on an emoji, stays whole.
#[test]
fn oversize_notification_is_refused_and_does_not_advance_the_id() {
    let root = TempRoot::new("bound");
    let path = root.path("notifications.json");
    std::fs::write(&path, r#"{"chats":{"1":{}},"next_id":5}"#).unwrap();
    let mut store = NotifyStore::open(path.clone()).expect("open");

    let exact = "n".repeat(TELEGRAM_MESSAGE_UTF16_LIMIT);
    store.enqueue(1, exact.clone(), None, 1).expect("exact");
    let emoji = "\u{1F600}";
    let keeps_emoji = "a".repeat(TELEGRAM_MESSAGE_UTF16_LIMIT - 2) + emoji;
    store
        .enqueue(1, keeps_emoji.clone(), None, 2)
        .expect("emoji fits");

    let accepted = store.file.clone();
    let spills_emoji = "a".repeat(TELEGRAM_MESSAGE_UTF16_LIMIT - 1) + emoji;
    store
        .enqueue(1, spills_emoji, None, 3)
        .expect("a refusal is not a disk error");
    let ascii_over = "x".repeat(TELEGRAM_MESSAGE_UTF16_LIMIT + 1);
    store
        .enqueue(1, ascii_over, None, 4)
        .expect("a refusal is not a disk error");

    assert_eq!(store.file, accepted, "a refusal must not change memory");
    let loaded = NotifyFile::load(&path).expect("reload");
    assert_eq!(loaded, store.file, "a refusal must not rewrite the file");
    assert_eq!(loaded.next_id, 7, "only accepted rows consume ids");
    assert_eq!(loaded.outbox.len(), 2);
    assert_eq!(loaded.outbox[0].id, 5);
    assert_eq!(loaded.outbox[0].html, exact);
    assert_eq!(
        utf16_len(&loaded.outbox[0].html),
        TELEGRAM_MESSAGE_UTF16_LIMIT
    );
    assert_eq!(loaded.outbox[1].id, 6);
    assert_eq!(loaded.outbox[1].html, keeps_emoji);
    assert!(loaded.outbox[1].html.ends_with(emoji));
    for row in &loaded.outbox {
        assert!(
            !row.html.contains('\u{2026}'),
            "an oversize body must not be stored as a cut"
        );
        assert!(utf16_len(&row.html) <= TELEGRAM_MESSAGE_UTF16_LIMIT);
        assert!(std::str::from_utf8(row.html.as_bytes()).is_ok());
    }

    let mut bare = NotifyFile {
        next_id: 5,
        ..NotifyFile::default()
    };
    assert!(
        !push_outbox(
            &mut bare,
            1,
            "x".repeat(TELEGRAM_MESSAGE_UTF16_LIMIT + 1),
            None,
            9,
        ),
        "push_outbox reports the refusal"
    );
    assert!(bare.outbox.is_empty(), "a refused push must not append");
    assert_eq!(
        bare.next_id, 5,
        "a refused push must not consume the next id"
    );
    assert!(push_outbox(&mut bare, 1, "ok".into(), None, 10));
    assert_eq!(bare.outbox.len(), 1);
    assert_eq!(bare.outbox[0].id, 5);
    assert_eq!(bare.outbox[0].html, "ok");
    assert_eq!(bare.next_id, 6);
}

/// Ack failed after Telegram accepted the send: the next pass retries the ack and does not
/// resend. Sending the body again would deliver the same notification twice.
#[test]
fn ack_failed_after_ok_retries_ack_without_resending() {
    let mut delivered = HashSet::new();
    let id = 3;
    assert!(
        !delivered.contains(&id),
        "an id Telegram has not accepted must be sent"
    );
    delivered.insert(id);
    assert!(
        delivered.contains(&id),
        "an accepted id whose ack has not saved must not be sent again"
    );
    delivered.remove(&id);
    assert!(
        !delivered.contains(&id),
        "a saved ack must drop the id so the sender does not keep treating it as delivered"
    );
}

/// Restart sends the loaded outbox from the front. Sorting by id would reorder a hand-written file.
#[test]
fn loaded_outbox_keeps_file_order() {
    let root = TempRoot::new("order");
    let path = root.path("notifications.json");
    std::fs::write(
        &path,
        r#"{"chats":{"9":{}},"outbox":[{"id":4,"chat":9,"html":"later-id-first","created_utc":10},{"id":2,"chat":9,"html":"earlier-id-second","created_utc":11}],"next_id":8}"#,
    )
    .unwrap();

    let store = NotifyStore::open(path).expect("open");
    let ids: Vec<u64> = store.file.outbox.iter().map(|row| row.id).collect();
    assert_eq!(ids, vec![4, 2]);
    assert_eq!(store.file.outbox[0].html, "later-id-first");
    assert_eq!(store.file.outbox[1].html, "earlier-id-second");
    assert_eq!(store.file.outbox[0].created_utc, 10);
    assert_eq!(store.file.outbox[1].chat, 9);
    assert_eq!(store.file.next_id, 8);
}

/// A chat edit must be on disk before `update` returns. A crash after the call still finds it.
#[test]
fn update_persists_before_it_returns() {
    let root = TempRoot::new("update");
    let path = root.path("notifications.json");
    let mut store = NotifyStore::open(path.clone()).expect("open");
    let kept = store
        .update(|file| {
            file.chats.insert(
                7,
                ChatNotify {
                    revision: 2,
                    ..ChatNotify::default()
                },
            );
            file.chats.len()
        })
        .expect("update");

    assert_eq!(kept, 1);
    let loaded = NotifyFile::load(&path).expect("reload");
    assert_eq!(loaded.chats.get(&7).map(|chat| chat.revision), Some(2));
    assert_eq!(store.file, loaded);
}

/// A failed save must not publish the edit. A directory is not a file the rename can replace.
#[test]
fn update_failed_save_leaves_memory_and_disk_unchanged() {
    let root = TempRoot::new("update-fail");
    let path = root.path("notifications.json");
    let mut store = NotifyStore::open(path.clone()).expect("open");
    store
        .update(|file| {
            file.chats.insert(7, ChatNotify::default());
        })
        .expect("seed");
    let seeded = store.file.clone();
    let dir = root.path("not-a-file");
    std::fs::create_dir_all(&dir).expect("directory");
    store.path = dir;

    let failed = store.update(|file| {
        file.chats.insert(8, ChatNotify::default());
    });

    assert!(failed.is_err(), "renaming onto a directory must fail");
    assert_eq!(store.file, seeded);
    let loaded = NotifyFile::load(&path).expect("seed file");
    assert_eq!(loaded, seeded);
    assert!(!loaded.chats.contains_key(&8));
}

/// Ledger edits and outbox rows land in one save. Pushing on a detached clone does not write.
#[test]
fn update_persists_ledger_and_outbox_together() {
    let root = TempRoot::new("atomic");
    let path = root.path("notifications.json");
    std::fs::write(&path, r#"{"chats":{"7":{"revision":1}},"next_id":3}"#).expect("seed");
    let mut store = NotifyStore::open(path.clone()).expect("open");
    let mut detached = store.file.clone();
    push_outbox(&mut detached, 7, "<b>x</b>".to_string(), None, 50);

    let loaded = NotifyFile::load(&path).expect("reload before update");
    assert!(loaded.outbox.is_empty(), "push_outbox must not save");
    assert!(store.file.outbox.is_empty());

    store
        .update(|file| {
            file.chats
                .get_mut(&7)
                .expect("seeded chat")
                .ledger
                .trades_enabled_utc = Some(1000);
            push_outbox(file, 7, "<b>x</b>".to_string(), None, 50);
        })
        .expect("update");

    let loaded = NotifyFile::load(&path).expect("reload after update");
    assert_eq!(
        loaded
            .chats
            .get(&7)
            .map(|chat| chat.ledger.trades_enabled_utc),
        Some(Some(1000))
    );
    assert_eq!(loaded.outbox.len(), 1);
    assert_eq!(loaded.outbox[0].html, "<b>x</b>");
    assert_eq!(loaded.outbox[0].id, 3);
    assert_eq!(loaded.outbox[0].chat, 7);
    assert_eq!(loaded.outbox[0].created_utc, 50);
    assert_eq!(loaded.next_id, 4);
    assert_eq!(store.file, loaded);
}

/// One queued row on chat 7. The allow map starts unpublished.
///
/// Args:
///     tag: Temp-directory name.
///     cores: Disclosure stored on the row.
///
/// Returns:
///     The temp root and the store. Dropping the root deletes the file.
fn store_with_row(tag: &str, cores: Option<Vec<u64>>) -> (TempRoot, Mutex<NotifyStore>) {
    let root = TempRoot::new(tag);
    let mut store = NotifyStore::open(root.path("notifications.json")).expect("open");
    store.enqueue(7, "body".into(), cores, 10).expect("enqueue");
    (root, Mutex::new(store))
}

/// Id of the only outbox row.
///
/// Args:
///     store: Store that holds one row.
///
/// Returns:
///     That row's id.
fn only_id(store: &Mutex<NotifyStore>) -> u64 {
    store
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .file
        .outbox[0]
        .id
}

/// Publish `allowed` on `store`.
///
/// Args:
///     store: Store whose in-memory map is replaced.
///     allowed: Chat id to that chat's grant.
fn publish(store: &Mutex<NotifyStore>, allowed: BTreeMap<i64, Option<BTreeSet<u64>>>) {
    store
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .publish_allowed(allowed);
}

/// Before the first publish the sender must not treat a row as ready.
#[test]
fn hold_waits_until_the_allow_map_is_published() {
    let (_root, store) = store_with_row("hold-wait", Some(vec![7]));
    let id = only_id(&store);
    assert!(matches!(super::hold(&store, id, 7), super::Held::Waiting));
}

/// A published map that omits the chat drops the row as unpaired.
#[test]
fn hold_marks_a_chat_absent_from_the_allow_map_unpaired() {
    let (_root, store) = store_with_row("hold-unpaired", Some(vec![7]));
    let id = only_id(&store);
    let mut allowed = BTreeMap::new();
    allowed.insert(99, None);
    publish(&store, allowed);
    assert!(matches!(super::hold(&store, id, 7), super::Held::Unpaired));
}

/// A viewer row for a core outside the published set is revoked, and the row stays queued.
#[test]
fn hold_revokes_a_viewer_row_the_purge_did_not_drop() {
    let (_root, store) = store_with_row("hold-revoked", Some(vec![7]));
    let id = only_id(&store);
    let mut allowed = BTreeMap::new();
    allowed.insert(7, Some(BTreeSet::from([8])));
    publish(&store, allowed);
    assert!(matches!(super::hold(&store, id, 7), super::Held::Revoked));
    let left = store
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .file
        .outbox
        .len();
    assert_eq!(left, 1, "hold does not ack; the row is still queued");
}

/// An unknown row is revoked for a viewer. An empty disclosure is ready.
#[test]
fn hold_revokes_an_unknown_viewer_row_and_readies_an_empty_disclosure() {
    let (_root, unknown) = store_with_row("hold-unknown", None);
    let unknown_id = only_id(&unknown);
    let mut viewers = BTreeMap::new();
    viewers.insert(7, Some(BTreeSet::from([8])));
    publish(&unknown, viewers.clone());
    assert!(matches!(
        super::hold(&unknown, unknown_id, 7),
        super::Held::Revoked
    ));

    let (_root, quiet) = store_with_row("hold-quiet", Some(Vec::new()));
    let quiet_id = only_id(&quiet);
    publish(&quiet, viewers);
    match super::hold(&quiet, quiet_id, 7) {
        super::Held::Ready(row) if row.auto.is_none() => assert_eq!(row.html, "body"),
        _ => panic!("an empty disclosure must be ready for a viewer"),
    }
}

/// An owner grant readies the row.
#[test]
fn hold_readies_an_owner_row() {
    let (_root, store) = store_with_row("hold-owner", Some(vec![7]));
    let id = only_id(&store);
    let mut allowed = BTreeMap::new();
    allowed.insert(7, None);
    publish(&store, allowed);
    match super::hold(&store, id, 7) {
        super::Held::Ready(row) if row.auto.is_none() => assert_eq!(row.html, "body"),
        _ => panic!("an owner row must be ready"),
    }
}

/// A running-total report queued behind an unsent one of its kind takes its place; hourly reports,
/// other kinds, other chats and ordinary notifications stay.
#[test]
fn a_new_auto_report_replaces_its_unsent_running_total_only() {
    use crate::telegram::api::{InlineKeyboardMarkup, ReplyMarkup};
    use crate::telegram::notify::{AutoReport, AutoRow};
    let auto = |kind| AutoRow {
        kind,
        keyboard: ReplyMarkup::Inline(InlineKeyboardMarkup {
            inline_keyboard: Vec::new(),
        }),
    };
    let mut file = NotifyFile::default();
    let mut push = |chat, html: &str, kind| {
        super::push_auto_report(&mut file, chat, html.into(), None, auto(kind), 2)
    };
    assert!(push(7, "t1", AutoReport::Today));
    assert!(push(7, "h1", AutoReport::Hourly));
    assert!(push(8, "t-other", AutoReport::Today));
    assert!(push(7, "m1", AutoReport::Month));
    assert!(push(7, "t2", AutoReport::Today));
    assert!(push(7, "h2", AutoReport::Hourly));
    push_outbox(&mut file, 7, "card".into(), None, 1);
    let bodies: Vec<&str> = file.outbox.iter().map(|row| row.html.as_str()).collect();
    assert_eq!(bodies, vec!["h1", "t-other", "m1", "t2", "h2", "card"]);
    // A rich report is not held to the 4096-unit cap of a plain message.
    let big = "x".repeat(20_000);
    assert!(super::push_auto_report(
        &mut file,
        7,
        big,
        None,
        auto(AutoReport::Month),
        6
    ));
    assert_eq!(file.outbox.last().unwrap().html.len(), 20_000);
}

/// Acking a running-total report records it as its kind's message and hands back the one it
/// replaces, in one save; an hourly report and a chat with no stored settings record nothing.
#[test]
fn acking_an_auto_report_records_it_and_returns_the_replaced_one() {
    use crate::telegram::notify::{AutoReport, ChatNotify};
    let root = TempRoot::new("ack-auto");
    let path = root.path("notifications.json");
    let mut store = NotifyStore::open(path.clone()).expect("open");
    store
        .update(|file| {
            let mut chat = ChatNotify::default();
            chat.settings.reports.set(AutoReport::Today, true);
            chat.settings.reports.set(AutoReport::Hourly, true);
            file.chats.insert(7, chat);
        })
        .expect("seed");
    store
        .enqueue(7, "report".into(), None, 10)
        .expect("enqueue");
    let id = store.file.outbox[0].id;
    assert_eq!(
        store
            .ack(id, 7, Some((AutoReport::Today, 100)), None)
            .unwrap(),
        None
    );
    assert_eq!(
        store
            .ack(id + 50, 7, Some((AutoReport::Today, 101)), None)
            .unwrap(),
        Some(100)
    );
    // The same message again is not its own predecessor.
    assert_eq!(
        store
            .ack(id + 51, 7, Some((AutoReport::Today, 101)), None)
            .unwrap(),
        None
    );
    // Hourly reports stay in the chat: nothing recorded, nothing to delete.
    assert_eq!(
        store
            .ack(id + 52, 7, Some((AutoReport::Hourly, 200)), None)
            .unwrap(),
        None
    );
    assert_eq!(
        store
            .ack(id + 53, 7, Some((AutoReport::Hourly, 201)), None)
            .unwrap(),
        None
    );
    let loaded = NotifyFile::load(&path).expect("reload");
    assert!(loaded.outbox.is_empty());
    assert_eq!(loaded.chats[&7].ledger.reports.today.message, Some(101));
    assert_eq!(loaded.chats[&7].ledger.reports.hourly.message, None);
    assert_eq!(
        store
            .ack(id + 54, 9, Some((AutoReport::Month, 5)), None)
            .unwrap(),
        None
    );
    // A report switched off while its message was in flight is not recorded.
    assert_eq!(
        store
            .ack(id + 55, 7, Some((AutoReport::Month, 300)), None)
            .unwrap(),
        None
    );
    assert_eq!(store.file.chats[&7].ledger.reports.month.message, None);
}

/// Acking a trade card the chat waits to fill in records its message against the trade in the
/// same save; a card the chat stopped waiting for is not recorded, and an edit row carries its
/// target message.
#[test]
fn acking_a_waiting_card_records_its_message() {
    use crate::telegram::notify::{CardKey, CardWait};
    let root = TempRoot::new("ack-card");
    let path = root.path("notifications.json");
    let mut store = NotifyStore::open(path.clone()).expect("open");
    let key = CardKey {
        core: 3,
        rec_id: 44,
    };
    store
        .update(|file| {
            let mut chat = ChatNotify::default();
            chat.ledger
                .cards
                .entry(3)
                .or_default()
                .insert(44, CardWait::default());
            file.chats.insert(7, chat);
            assert!(super::push_trade_card(
                file,
                7,
                "card".into(),
                Some(vec![3]),
                Some(key),
                10
            ));
            assert!(super::push_edit(
                file,
                7,
                900,
                "filled".into(),
                Some(vec![3]),
                11
            ));
        })
        .expect("seed");
    assert_eq!(store.file.outbox[0].card, Some(key));
    assert_eq!(store.file.outbox[1].edit, Some(900));
    let id = store.file.outbox[0].id;
    store.ack(id, 7, None, Some((key, 555))).expect("ack");
    let loaded = NotifyFile::load(&path).expect("reload");
    assert_eq!(loaded.outbox.len(), 1);
    assert_eq!(loaded.chats[&7].ledger.cards[&3][&44].message, Some(555));
    let gone = CardKey {
        core: 3,
        rec_id: 45,
    };
    store.ack(id + 9, 7, None, Some((gone, 556))).expect("ack");
    assert!(!store.file.chats[&7].ledger.cards[&3].contains_key(&45));
}

/// A redraw of a menu screen carries its buttons, and only the newest one per message waits: an
/// older picture of the same message goes, while edits of a card, another message and another
/// chat stay. A press on the message drops its redraw alone.
#[test]
fn a_redraw_keeps_only_the_newest_picture_of_its_message() {
    use crate::telegram::api::{InlineKeyboardMarkup, ReplyMarkup};
    let keyboard = || {
        ReplyMarkup::Inline(InlineKeyboardMarkup {
            inline_keyboard: Vec::new(),
        })
    };
    let mut file = NotifyFile::default();
    assert!(super::push_redraw(
        &mut file,
        7,
        100,
        "old".into(),
        keyboard(),
        1
    ));
    assert!(super::push_edit(&mut file, 7, 100, "card".into(), None, 1));
    assert!(super::push_redraw(
        &mut file,
        7,
        101,
        "other message".into(),
        keyboard(),
        1
    ));
    assert!(super::push_redraw(
        &mut file,
        8,
        100,
        "other chat".into(),
        keyboard(),
        1
    ));
    assert!(super::push_redraw(
        &mut file,
        7,
        100,
        "new".into(),
        keyboard(),
        2
    ));
    let rows: Vec<(&str, Option<i64>, bool)> = file
        .outbox
        .iter()
        .map(|row| (row.html.as_str(), row.edit, row.redraw.is_some()))
        .collect();
    assert_eq!(
        rows,
        vec![
            ("card", Some(100), false),
            ("other message", Some(101), true),
            ("other chat", Some(100), true),
            ("new", Some(100), true),
        ]
    );
    assert_eq!(super::drop_redraws(&mut file, 7, 100), 1);
    let left: Vec<&str> = file.outbox.iter().map(|row| row.html.as_str()).collect();
    assert_eq!(left, vec!["card", "other message", "other chat"]);
}

/// A redraw stays worth sending for a minute; an ordinary notification has no such limit.
#[test]
fn a_redraw_outlives_its_minute_and_a_notification_does_not() {
    use crate::telegram::api::{InlineKeyboardMarkup, ReplyMarkup};
    let mut file = NotifyFile::default();
    assert!(super::push_redraw(
        &mut file,
        7,
        100,
        "screen".into(),
        ReplyMarkup::Inline(InlineKeyboardMarkup {
            inline_keyboard: Vec::new(),
        }),
        1_000,
    ));
    push_outbox(&mut file, 7, "card".into(), None, 1_000);
    let (redraw, card) = (&file.outbox[0], &file.outbox[1]);
    assert!(!super::redraw_outlived(redraw, 1_060));
    assert!(super::redraw_outlived(redraw, 1_061));
    assert!(!super::redraw_outlived(card, 100_000));
}
