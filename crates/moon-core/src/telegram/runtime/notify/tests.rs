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

/// Supersession must preserve mandatory cards, unrelated targets and the last valid edit.
#[test]
fn pending_edits_are_bounded_by_the_messages_they_replace() {
    let mut file = NotifyFile::default();
    push_outbox(&mut file, 7, "mandatory".into(), None, 1);
    super::push_edit(&mut file, 8, 10, "other chat".into(), None, 1);
    super::push_edit(&mut file, 7, 11, "other message".into(), None, 1);
    for id in 0..100 {
        super::push_edit(&mut file, 7, 10, format!("state {id}"), None, id);
    }
    assert_eq!(
        file.outbox
            .iter()
            .map(|row| row.html.as_str())
            .collect::<Vec<_>>(),
        vec!["mandatory", "other chat", "other message", "state 99"]
    );
    assert!(!super::push_edit(
        &mut file,
        7,
        10,
        "x".repeat(4097),
        None,
        101
    ));
    assert_eq!(file.outbox.last().unwrap().html, "state 99");
}

/// A blocked chat keeps every distinct card but at most one pending edit per target message.
#[test]
#[ignore = "manual blocked-chat queue scale measurement"]
fn probe_blocked_chat_supersession() {
    let mut before = Vec::new();
    let mut after = Vec::new();
    for _ in 0..5 {
        let mut original = NotifyFile::default();
        for id in 0..2000 {
            push_outbox(&mut original, 7, format!("card {id}"), None, id);
        }
        let mut current = original.clone();
        let start = std::time::Instant::now();
        for id in 0..5000 {
            super::push_row(&mut original, 7, format!("state {id}"), None, None, id);
            original.outbox.last_mut().unwrap().edit = Some(42);
        }
        before.push(start.elapsed().as_secs_f64() * 1000.0 / 5000.0);
        let start = std::time::Instant::now();
        for id in 0..5000 {
            super::push_edit(&mut current, 7, 42, format!("state {id}"), None, id);
        }
        after.push(start.elapsed().as_secs_f64() * 1000.0 / 5000.0);
        assert_eq!(original.outbox.len(), 7000);
        assert_eq!(current.outbox.len(), 2001);
        assert_eq!(
            current
                .outbox
                .iter()
                .filter(|row| row.edit.is_none())
                .count(),
            2000
        );
    }
    before.sort_by(f64::total_cmp);
    after.sort_by(f64::total_cmp);
    println!(
        "[scale] blocked chat: 7000 -> 2001 rows; enqueue {:.6} -> {:.6} ms/edit",
        before[2], after[2]
    );
}

/// Mixed owner transactions and delivery receipts must replay in order, including after a
/// checkpoint whose old journal has not yet been cleared.
#[test]
fn notification_transactions_replay_with_receipts_and_checkpoint() {
    let root = TempRoot::new("transactions");
    let path = root.path("notifications.json");
    let mut store = NotifyStore::open(path.clone()).unwrap();
    store
        .update(|file| {
            file.chats.insert(7, ChatNotify::default());
        })
        .unwrap();
    store
        .edit_notifications(|file| {
            file.chats
                .get_mut(&7)
                .unwrap()
                .ledger
                .seen
                .insert(1, BTreeMap::from([(4, 40)]));
            push_outbox(file, 7, "first".into(), None, 1);
            push_outbox(file, 7, "second".into(), None, 1);
        })
        .unwrap();
    store.ack(0, 7, None, None).unwrap();
    store
        .edit_notifications(|file| {
            file.chats
                .get_mut(&7)
                .unwrap()
                .ledger
                .down_announced
                .insert(1);
            push_outbox(file, 7, "third".into(), None, 2);
        })
        .unwrap();
    assert_eq!(NotifyFile::load(&path).unwrap(), store.file);
    store.file.save(&path).unwrap();
    assert_eq!(NotifyFile::load(&path).unwrap(), store.file);
    let mut restarted = NotifyStore::open(path.clone()).unwrap();
    restarted.ack(1, 7, None, None).unwrap();
    assert_eq!(NotifyFile::load(&path).unwrap(), restarted.file);
    restarted.checkpoint().unwrap();
    assert!(
        std::fs::read(crate::config::paths::telegram_notify_receipts(&path))
            .unwrap()
            .is_empty()
    );
}

/// A failed incremental append must not publish either its ledger or its queued notification.
#[test]
fn notification_transaction_failure_leaves_the_document_unchanged() {
    let root = TempRoot::new("transaction-fail");
    let path = root.path("notifications.json");
    let mut store = NotifyStore::open(path.clone()).unwrap();
    store
        .update(|file| {
            file.chats.insert(7, ChatNotify::default());
        })
        .unwrap();
    let before = store.file.clone();
    std::fs::create_dir(crate::config::paths::telegram_notify_receipts(&path)).unwrap();
    assert!(
        store
            .edit_notifications(|file| {
                file.chats
                    .get_mut(&7)
                    .unwrap()
                    .ledger
                    .down_announced
                    .insert(1);
                push_outbox(file, 7, "unsaved".into(), None, 1);
            })
            .is_err()
    );
    assert_eq!(store.file, before);
    assert_eq!(NotifyFile::load_snapshot(&path).unwrap(), before);
}

/// Measure owner updates and down snapshots with a 72-hour ledger and a slow-chat backlog.
#[test]
#[ignore = "manual synthetic notification persistence scale measurement"]
fn probe_owner_notify_updates() {
    let root = TempRoot::new("owner-scale");
    let mut store = NotifyStore::open(root.path("notifications.json")).unwrap();
    store
        .update(|file| {
            let mut chat = ChatNotify::default();
            for core in 1..=10 {
                chat.ledger
                    .seen
                    .insert(core, (0..5000).map(|id| (id, 100)).collect());
            }
            file.chats.insert(7, chat);
            for id in 0..2000 {
                file.outbox.push(crate::telegram::notify::Pending {
                    id,
                    chat: 7,
                    html: "XALPHA ".repeat(64),
                    ..Default::default()
                });
            }
            file.next_id = 2000;
        })
        .unwrap();
    let mut control = NotifyStore::open(root.path("control.json")).unwrap();
    control.file = store.file.clone();
    control.file.save(&control.path).unwrap();
    let mut before = Vec::new();
    let mut after = Vec::new();
    for round in 0..8 {
        let edit = |file: &mut NotifyFile| {
            file.chats
                .get_mut(&7)
                .unwrap()
                .ledger
                .seen
                .get_mut(&1)
                .unwrap()
                .insert(0, 100 + round);
            push_outbox(file, 7, "new close".into(), None, round);
        };
        let mut old = || {
            let start = std::time::Instant::now();
            control.frozen_notify_update(edit).unwrap();
            before.push(start.elapsed().as_secs_f64() * 1000.0);
        };
        let mut new = || {
            let start = std::time::Instant::now();
            let mut batch = crate::telegram::notify::NotificationBatch::new(
                &store.file,
                &[7],
                &[crate::telegram::notify::CardKey { core: 1, rec_id: 0 }],
                None,
            );
            edit(&mut batch.file);
            store.update_notifications(batch.finish()).unwrap();
            after.push(start.elapsed().as_secs_f64() * 1000.0);
        };
        if round % 2 == 0 {
            old();
            new();
        } else {
            new();
            old();
        }
        let mut expected = control.file.clone();
        expected.ack_sequence = store.file.ack_sequence;
        assert_eq!(store.file, expected);
        assert_eq!(NotifyFile::load(&store.path).unwrap(), store.file);
    }
    before.sort_by(f64::total_cmp);
    after.sort_by(f64::total_cmp);
    println!(
        "[scale] dirty owner update median: {:.3} -> {:.3} ms",
        before[4], after[4]
    );
    let mut next = store.file.clone();
    next.chats
        .get_mut(&7)
        .unwrap()
        .ledger
        .seen
        .get_mut(&1)
        .unwrap()
        .insert(0, 999);
    push_outbox(&mut next, 7, "another close".into(), None, 999);
    let delta = crate::telegram::notify::updates::Update::between(&store.file, &next);
    component_median("document clone", || {
        std::hint::black_box(store.file.clone());
    });
    component_median("transaction diff", || {
        std::hint::black_box(crate::telegram::notify::updates::Update::between(
            &store.file,
            &next,
        ));
    });
    component_median("transaction encode", || {
        std::hint::black_box(serde_json::to_vec(&delta).unwrap());
    });
    let mut bytes = 0;
    let mut sequence = 0;
    component_median("transaction append and sync", || {
        sequence += 1;
        bytes =
            super::receipts::append_update(&root.path("component.json"), bytes, sequence, &delta)
                .unwrap();
    });
    let ledger = &store.file.chats[&7].ledger;
    let start = std::time::Instant::now();
    for _ in 0..100 {
        std::hint::black_box(ledger.clone());
    }
    println!(
        "[scale] whole down ledger clone: {:.3} ms",
        start.elapsed().as_secs_f64() * 10.0
    );
    let start = std::time::Instant::now();
    for _ in 0..100 {
        std::hint::black_box(ledger.down_announced.clone());
    }
    println!(
        "[scale] down bits clone: {:.6} ms",
        start.elapsed().as_secs_f64() * 10.0
    );
}

/// Separate CPU preparation from durable I/O instead of attributing a whole save to cloning.
fn component_median(label: &str, mut run: impl FnMut()) {
    let mut samples = Vec::new();
    for _ in 0..7 {
        let start = std::time::Instant::now();
        run();
        samples.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    samples.sort_by(f64::total_cmp);
    println!("[scale] {label}: {:.3} ms", samples[3]);
}

/// Keeping a short already-checkpointed prefix would make the next durable ack look like a
/// sequence gap on restart and resend that card, including when corruption shortened the prefix.
#[test]
fn checkpointed_short_receipt_prefix_cannot_poison_the_next_append() {
    use std::io::Write;
    for tail in [b"".as_slice(), b"corrupt\n".as_slice()] {
        let root = TempRoot::new("receipt-short-prefix");
        let path = root.path("notifications.json");
        let mut store = NotifyStore::open(path.clone()).unwrap();
        store.enqueue(7, "XALPHA".into(), None, 1).unwrap();
        store.file.ack_sequence = 5;
        store.file.save(&path).unwrap();
        super::receipts::append(
            &path,
            0,
            &super::receipts::Receipt {
                sequence: 1,
                id: 99,
                chat: 7,
                replaced: None,
                card: None,
            },
        )
        .unwrap();
        std::fs::OpenOptions::new()
            .append(true)
            .open(crate::config::paths::telegram_notify_receipts(&path))
            .unwrap()
            .write_all(tail)
            .unwrap();
        let mut loaded = NotifyStore::open(path.clone()).unwrap();
        loaded.ack(0, 7, None, None).unwrap();
        let restarted = NotifyStore::open(path).unwrap();
        assert!(restarted.file.outbox.is_empty());
        assert_eq!(restarted.file.ack_sequence, 6);
    }
}

/// Forgetting a still-queued accepted id resends it; retaining a removed id leaks sender state.
#[test]
fn delivered_retention_keeps_only_queued_accepted_ids() {
    let root = TempRoot::new("delivered-retain");
    let mut store = NotifyStore::open(root.path("notifications.json")).unwrap();
    store.enqueue(7, "XALPHA".into(), None, 1).unwrap();
    store.enqueue(7, "UNIT_01".into(), None, 1).unwrap();
    store.ack(0, 7, None, None).unwrap();
    let mut delivered = std::collections::HashMap::from([
        (0, super::Accepted::default()),
        (1, super::Accepted::default()),
        (99, super::Accepted::default()),
    ]);
    super::retain_delivered(&Mutex::new(store), &mut delivered);
    assert_eq!(delivered.keys().copied().collect::<Vec<_>>(), vec![1]);
}

/// Paired lock-scope measurement at 10000 remembered ids, where per-id locking scales badly.
#[test]
#[ignore = "scale probe"]
fn probe_delivered_retention() {
    let root = TempRoot::new("delivered-probe");
    let mut store = NotifyStore::open(root.path("notifications.json")).unwrap();
    for id in 0..10_000 {
        store.file.outbox.push(crate::telegram::notify::Pending {
            id,
            chat: 7,
            ..Default::default()
        });
    }
    let store = Mutex::new(store);
    let initial: std::collections::HashMap<_, _> = (0..10_000)
        .map(|id| (id, super::Accepted::default()))
        .collect();
    let mut current = initial.clone();
    let mut per_id = initial;
    let mut current_time = std::time::Duration::ZERO;
    let mut per_id_time = std::time::Duration::ZERO;
    for round in 0..200 {
        let mut measure_current = || {
            let start = std::time::Instant::now();
            super::retain_delivered(&store, &mut current);
            current_time += start.elapsed();
        };
        let mut measure_per_id = || {
            let start = std::time::Instant::now();
            // Frozen pre-fix control: reacquire the document mutex for every remembered id.
            per_id.retain(|id, _| super::lock_store(&store).file.outbox.by_id(*id).is_some());
            per_id_time += start.elapsed();
        };
        if round % 2 == 0 {
            measure_current();
            measure_per_id();
        } else {
            measure_per_id();
            measure_current();
        }
    }
    assert_eq!(current.len(), 10_000);
    assert_eq!(per_id.len(), 10_000);
    eprintln!(
        "delivered retain ms/pass: current={:.3}, per-id-lock={:.3}",
        current_time.as_secs_f64() * 5.0,
        per_id_time.as_secs_f64() * 5.0
    );
}

/// A replayed photo receipt must clean up fresh files immediately, while a shared photo stays
/// until its final queued row is acknowledged. The age-based startup sweep cannot prove this.
#[test]
fn receipt_replay_deletes_only_photos_without_a_queued_owner() {
    let root = TempRoot::new("receipt-photos");
    let path = root.path("notifications.json");
    let mut store = NotifyStore::open(path.clone()).unwrap();
    let dir = super::chart_spool_dir(&path);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("single.png"), b"fixture").unwrap();
    std::fs::write(dir.join("shared.png"), b"fixture").unwrap();
    store
        .update(|file| {
            for photo in ["single.png", "shared.png", "shared.png"] {
                super::push_photo(file, 7, "XALPHA".into(), None, photo.into(), 1);
            }
        })
        .unwrap();
    let mut length = 0;
    for id in 0..2 {
        length = super::receipts::append(
            &path,
            length,
            &super::receipts::Receipt {
                sequence: id + 1,
                id,
                chat: 7,
                replaced: None,
                card: None,
            },
        )
        .unwrap();
    }
    let loaded = NotifyFile::load(&path).unwrap();
    assert_eq!(
        loaded.outbox.iter().map(|row| row.id).collect::<Vec<_>>(),
        vec![2]
    );
    assert!(!dir.join("single.png").exists());
    assert!(dir.join("shared.png").exists());
    super::receipts::append(
        &path,
        length,
        &super::receipts::Receipt {
            sequence: 3,
            id: 2,
            chat: 7,
            replaced: None,
            card: None,
        },
    )
    .unwrap();
    assert!(NotifyStore::open(path).unwrap().file.outbox.is_empty());
    assert!(!dir.join("shared.png").exists());
}

/// A downgrade drops the cursor but keeps newer report slots; stale receipts must not roll
/// those slots back or remove a newly queued row that reused an old id.
#[test]
fn downgrade_snapshot_discards_a_journal_ahead_of_its_cursor() {
    use crate::telegram::notify::AutoReport;
    let root = TempRoot::new("receipt-downgrade");
    let path = root.path("notifications.json");
    let mut store = NotifyStore::open(path.clone()).unwrap();
    store
        .update(|file| {
            let mut chat = ChatNotify::default();
            chat.settings.reports.today = true;
            chat.ledger.reports.today.message = Some(200);
            file.chats.insert(7, chat);
            push_outbox(file, 7, "XALPHA".into(), None, 1);
        })
        .unwrap();
    let mut snapshot = serde_json::to_value(&store.file).unwrap();
    snapshot.as_object_mut().unwrap().remove("ack_sequence");
    std::fs::write(&path, serde_json::to_vec(&snapshot).unwrap()).unwrap();
    super::receipts::append(
        &path,
        0,
        &super::receipts::Receipt {
            sequence: 4,
            id: 0,
            chat: 7,
            replaced: Some((AutoReport::Today, 100)),
            card: None,
        },
    )
    .unwrap();
    let loaded = NotifyStore::open(path.clone()).unwrap();
    assert_eq!(loaded.file, store.file);
    assert!(
        std::fs::read(crate::config::paths::telegram_notify_receipts(&path))
            .unwrap()
            .is_empty()
    );
}

/// A later sequence gap proves the whole journal stale, including its seemingly valid prefix.
#[test]
fn discontinuous_receipts_do_not_partially_mutate_the_snapshot() {
    let root = TempRoot::new("receipt-gap");
    let path = root.path("notifications.json");
    let mut store = NotifyStore::open(path.clone()).unwrap();
    store.enqueue(7, "XALPHA".into(), None, 1).unwrap();
    let receipt = |sequence| super::receipts::Receipt {
        sequence,
        id: 0,
        chat: 7,
        replaced: None,
        card: None,
    };
    let bytes = super::receipts::append(&path, 0, &receipt(1)).unwrap();
    super::receipts::append(&path, bytes, &receipt(3)).unwrap();
    assert_eq!(NotifyStore::open(path).unwrap().file, store.file);
}

/// A corrupt complete middle line must leave notifications available and preserve only the
/// accepted prefix; the next append must replace the corrupt line and everything after it.
#[test]
fn corrupt_receipt_middle_replays_prefix_and_is_overwritten() {
    use std::io::Write;
    let root = TempRoot::new("receipt-corrupt");
    let path = root.path("notifications.json");
    let mut store = NotifyStore::open(path.clone()).unwrap();
    store
        .update(|file| {
            for body in ["XALPHA", "UNIT_01", "CORE_A"] {
                push_outbox(file, 7, body.into(), None, 1);
            }
        })
        .unwrap();
    store.ack(0, 7, None, None).unwrap();
    let journal_path = crate::config::paths::telegram_notify_receipts(&path);
    std::fs::OpenOptions::new()
        .append(true)
        .open(&journal_path)
        .unwrap()
        .write_all(
            b"corrupt\n{\"sequence\":3,\"id\":2,\"chat\":7,\"replaced\":null,\"card\":null}\n",
        )
        .unwrap();
    let mut loaded = NotifyStore::open(path.clone()).unwrap();
    assert_eq!(
        loaded
            .file
            .outbox
            .iter()
            .map(|row| row.id)
            .collect::<Vec<_>>(),
        vec![1, 2]
    );
    loaded.ack(1, 7, None, None).unwrap();
    let restarted = NotifyStore::open(path).unwrap();
    assert_eq!(
        restarted
            .file
            .outbox
            .iter()
            .map(|row| row.id)
            .collect::<Vec<_>>(),
        vec![2]
    );
    assert_eq!(restarted.file.ack_sequence, 2);
    assert!(
        !String::from_utf8(std::fs::read(journal_path).unwrap())
            .unwrap()
            .contains("corrupt")
    );
}

/// Both hosts join this service on clean exit. Even a nonempty queue must checkpoint after
/// the last sender joins, so a normal binary downgrade cannot resend journal-only deliveries.
#[test]
fn service_stop_checkpoints_receipts_with_pending_rows() {
    let root = TempRoot::new("receipt-stop");
    let path = root.path("notifications.json");
    let mut store = NotifyStore::open(path.clone()).unwrap();
    store
        .update(|file| {
            push_outbox(file, 7, "XALPHA".into(), None, 1);
            push_outbox(file, 7, "UNIT_01".into(), None, 1);
        })
        .unwrap();
    let shared = std::sync::Arc::new(Mutex::new(store));
    let sender = shared.clone();
    let join = std::thread::spawn(move || {
        super::durable_ack(&sender, 0, 7, None, None).unwrap();
    });
    let (_, events) = std::sync::mpsc::sync_channel(1);
    let mut service = super::super::TelegramService {
        alive: None,
        joins: vec![join],
        events,
        authorization: std::sync::Arc::new(Mutex::new(
            crate::telegram::auth::Authorization::from_authorized_chats(vec![]),
        )),
        mini_config: std::sync::Arc::default(),
        labels: std::sync::Arc::default(),
        notifications: Some(shared),
        notify_wake: None,
        work_waker: std::sync::Arc::default(),
    };
    service.stop();
    assert!(
        std::fs::read(crate::config::paths::telegram_notify_receipts(&path))
            .unwrap()
            .is_empty()
    );
    let snapshot = NotifyFile::load_snapshot(&path).unwrap();
    assert_eq!(snapshot.ack_sequence, 1);
    assert_eq!(
        snapshot.outbox.iter().map(|row| row.id).collect::<Vec<_>>(),
        vec![1]
    );
}

impl NotifyStore {
    /// Frozen round-one clone/diff/append path for the paired residual-budget measurement.
    fn frozen_notify_update(&mut self, edit: impl FnOnce(&mut NotifyFile)) -> anyhow::Result<()> {
        let mut next = self.file.clone();
        edit(&mut next);
        if next == self.file {
            return Ok(());
        }
        let mut journal = self.journal.try_lock().unwrap();
        let sequence = journal.sequence + 1;
        let update = crate::telegram::notify::updates::Update::between(&self.file, &next);
        journal.bytes =
            super::receipts::append_update(&self.path, journal.bytes, sequence, &update)?;
        journal.sequence = sequence;
        next.ack_sequence = sequence;
        self.file = next;
        Ok(())
    }

    /// Build explicit candidate patches for legacy fixture edits, leaving the queue out of drafts.
    fn edit_notifications<R>(
        &mut self,
        edit: impl FnOnce(&mut NotifyFile) -> R,
    ) -> anyhow::Result<R> {
        let chats: Vec<_> = self.file.chats.keys().copied().collect();
        let candidates: Vec<_> = self
            .file
            .chats
            .values()
            .flat_map(|chat| {
                chat.ledger.seen.iter().flat_map(|(core, rows)| {
                    rows.keys().map(|rec_id| crate::telegram::notify::CardKey {
                        core: *core,
                        rec_id: *rec_id,
                    })
                })
            })
            .collect();
        let mut batch =
            crate::telegram::notify::NotificationBatch::new(&self.file, &chats, &candidates, None);
        let result = edit(&mut batch.file);
        self.update_notifications(batch.finish())?;
        Ok(result)
    }

    /// Adapt existing single-owner fixtures to the production two-phase receipt transaction,
    /// moving rather than copying the document into its shared-store mutex.
    fn ack(
        &mut self,
        id: u64,
        chat: i64,
        replaced: Option<(crate::telegram::notify::AutoReport, i64)>,
        card: Option<(crate::telegram::notify::CardKey, i64)>,
    ) -> anyhow::Result<Option<i64>> {
        let placeholder = NotifyStore {
            path: self.path.clone(),
            file: Default::default(),
            allowed: None,
            wake: self.wake.clone(),
            journal: self.journal.clone(),
            purged: None,
            checkpoint_at: self.checkpoint_at,
        };
        let shared = Mutex::new(std::mem::replace(self, placeholder));
        let result = super::durable_ack(&shared, id, chat, replaced, card);
        *self = shared
            .into_inner()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        result
    }
}

/// A slow receipt flush must not block owner ticks, and an overlapping settings save must not
/// truncate an as-yet-uncommitted receipt. Channels pause at the exact I/O boundary without sleeps.
#[test]
fn receipt_io_releases_the_store_and_survives_an_overlapping_save() {
    let root = TempRoot::new("receipt-overlap");
    let path = root.path("notifications.json");
    let mut original = NotifyStore::open(path.clone()).unwrap();
    original.enqueue(7, "XALPHA".into(), None, 1).unwrap();
    let shared = std::sync::Arc::new(Mutex::new(original));
    let writer = shared.clone();
    let (entered, entered_rx) = std::sync::mpsc::channel();
    let (resume, resume_rx) = std::sync::mpsc::channel();
    let task = std::thread::spawn(move || {
        super::ack_with(&writer, 0, 7, None, None, |path, bytes, receipt| {
            let length = super::receipts::append(path, bytes, receipt)?;
            entered.send(()).unwrap();
            resume_rx
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap();
            Ok(length)
        })
    });
    entered_rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .unwrap();
    {
        let mut guard = shared.try_lock().expect("owner blocked by receipt I/O");
        assert_eq!(
            guard.file.outbox.len(),
            1,
            "publish only after receipt completion"
        );
        guard
            .update(|file| {
                file.chats.insert(8, ChatNotify::default());
            })
            .unwrap();
        guard
            .edit_notifications(|file| {
                file.chats
                    .get_mut(&8)
                    .unwrap()
                    .ledger
                    .down_announced
                    .insert(1);
            })
            .unwrap();
    }
    resume.send(()).unwrap();
    task.join().unwrap().unwrap();
    let loaded = NotifyFile::load(&path).unwrap();
    assert!(
        loaded.outbox.is_empty(),
        "overlapping save discarded the durable receipt"
    );
    assert!(loaded.chats.contains_key(&8));
    assert!(loaded.chats[&8].ledger.down_announced.contains(&1));
}

/// A replaced queue or changed disclosure must be scanned, but removals and ledger-only saves
/// must not invalidate a previously completed revocation scan.
#[test]
fn revocation_cache_tracks_appends_replacements_and_grants() {
    let root = TempRoot::new("purge-cache");
    let mut store = NotifyStore::open(root.path("notifications.json")).unwrap();
    store.enqueue(7, "XALPHA".into(), Some(vec![1]), 1).unwrap();
    let plans = BTreeMap::from([(7, (BTreeSet::from([1]), false))]);
    assert_eq!(store.purge_disallowed(plans.clone()).unwrap(), 0);
    let stamp = store.purged.as_ref().unwrap().revision;
    store
        .update(|file| {
            file.chats.insert(7, ChatNotify::default());
        })
        .unwrap();
    assert_eq!(store.file.outbox.revision(), stamp);
    assert_eq!(store.purge_disallowed(plans.clone()).unwrap(), 0);
    store
        .enqueue(7, "UNIT_01".into(), Some(vec![2]), 1)
        .unwrap();
    assert_eq!(store.purge_disallowed(plans.clone()).unwrap(), 1);
    store
        .update(|file| {
            file.outbox[0].cores = Some(vec![2]);
        })
        .unwrap();
    assert_eq!(store.purge_disallowed(plans.clone()).unwrap(), 1);
    store
        .update(|file| {
            file.outbox = vec![crate::telegram::notify::Pending {
                id: 99,
                chat: 7,
                cores: Some(vec![2]),
                ..Default::default()
            }]
            .into();
        })
        .unwrap();
    assert_eq!(store.purge_disallowed(plans).unwrap(), 1);
    store.enqueue(7, "XALPHA".into(), Some(vec![1]), 1).unwrap();
    assert_eq!(
        store
            .purge_disallowed(BTreeMap::from([(7, (BTreeSet::new(), false))]))
            .unwrap(),
        1
    );
}

/// Frozen clone/retain/ledger portion of the original acknowledgement; persistence is tested
/// separately by restart tests, while this oracle pins every surviving value and row order.
fn frozen_ack(
    file: &mut NotifyFile,
    id: u64,
    chat: i64,
    replaced: Option<(crate::telegram::notify::AutoReport, i64)>,
    card: Option<(crate::telegram::notify::CardKey, i64)>,
) -> Option<i64> {
    let replaced = replaced.filter(|(kind, _)| kind.replaces_previous());
    let mut next = file.clone();
    next.outbox.retain(|row| row.id != id);
    if let Some((key, message)) = card {
        let wait = next
            .chats
            .get_mut(&chat)
            .and_then(|entry| entry.ledger.cards.get_mut(&key.core))
            .and_then(|rows| rows.get_mut(&key.rec_id));
        if let Some(wait) = wait {
            wait.message = Some(message);
        }
    }
    let previous = replaced.and_then(|(kind, message)| {
        let entry = next.chats.get_mut(&chat)?;
        if !entry.settings.reports.on(kind) {
            return None;
        }
        let slot = entry.ledger.reports.slot_mut(kind);
        slot.message.replace(message).filter(|old| *old != message)
    });
    *file = next;
    previous
}

/// Receipts must preserve missing-row acks, disabled reports, repeated message ids and card fills.
#[test]
fn receipt_ledger_and_fifo_match_the_frozen_ack() {
    use crate::telegram::notify::{AutoReport, CardKey, CardWait};
    let root = TempRoot::new("ack-parity");
    let path = root.path("notifications.json");
    let mut store = NotifyStore::open(path.clone()).unwrap();
    let key = CardKey { core: 1, rec_id: 4 };
    store
        .update(|file| {
            let mut entry = ChatNotify::default();
            entry.settings.reports.today = true;
            entry
                .ledger
                .cards
                .entry(1)
                .or_default()
                .insert(4, CardWait::default());
            file.chats.insert(7, entry);
            for id in 0..4 {
                push_outbox(file, 7, format!("XALPHA {id}"), None, 1);
            }
        })
        .unwrap();
    let mut old = store.file.clone();
    for (id, kind, message, card) in [
        (2, AutoReport::Today, 100, Some((key, 200))),
        (0, AutoReport::Today, 101, None),
        (99, AutoReport::Today, 101, None),
        (1, AutoReport::Hourly, 300, None),
        (3, AutoReport::Month, 400, None),
    ] {
        let expected = frozen_ack(&mut old, id, 7, Some((kind, message)), card);
        assert_eq!(
            store.ack(id, 7, Some((kind, message)), card).unwrap(),
            expected
        );
        assert_eq!(store.file.chats, old.chats);
        assert_eq!(store.file.outbox, old.outbox);
        let loaded = NotifyFile::load(&path).unwrap();
        assert_eq!(loaded.chats, old.chats);
        assert_eq!(loaded.outbox, old.outbox);
        assert_eq!(loaded.next_id, old.next_id);
    }
}

/// Losing an accepted row before the full checkpoint must not replay it after a crash.
#[test]
fn receipts_recover_acks_before_the_coalesced_checkpoint() {
    let root = TempRoot::new("receipt-recover");
    let path = root.path("notifications.json");
    let mut store = NotifyStore::open(path.clone()).unwrap();
    store
        .update(|file| {
            push_outbox(file, 7, "XALPHA".into(), None, 1);
            push_outbox(file, 8, "UNIT_01".into(), None, 1);
        })
        .unwrap();
    let snapshot = std::fs::read(&path).unwrap();
    store.ack(0, 7, None, None).unwrap();
    assert_eq!(
        std::fs::read(&path).unwrap(),
        snapshot,
        "acks must not rewrite the full document"
    );
    let recovered = NotifyStore::open(path.clone()).unwrap();
    assert_eq!(
        recovered
            .file
            .outbox
            .iter()
            .map(|row| row.id)
            .collect::<Vec<_>>(),
        vec![1]
    );
    assert_eq!(recovered.file, store.file);
    store.ack(1, 8, None, None).unwrap();
    store.checkpoint().unwrap();
    assert!(NotifyFile::load(&path).unwrap().outbox.is_empty());
    assert!(
        std::fs::read(crate::config::paths::telegram_notify_receipts(&path))
            .unwrap()
            .is_empty()
    );
}

/// A torn unconfirmed receipt is overwritten on retry, rather than poisoning later valid acks.
#[test]
fn a_torn_receipt_tail_is_replaced_without_losing_complete_receipts() {
    use std::io::Write;
    let root = TempRoot::new("receipt-tail");
    let path = root.path("notifications.json");
    let mut store = NotifyStore::open(path.clone()).unwrap();
    store
        .update(|file| {
            push_outbox(file, 7, "XALPHA".into(), None, 1);
            push_outbox(file, 7, "UNIT_01".into(), None, 1);
        })
        .unwrap();
    store.ack(0, 7, None, None).unwrap();
    std::fs::OpenOptions::new()
        .append(true)
        .open(crate::config::paths::telegram_notify_receipts(&path))
        .unwrap()
        .write_all(b"{\"sequence\":")
        .unwrap();
    let mut restarted = NotifyStore::open(path.clone()).unwrap();
    assert_eq!(restarted.file.outbox.len(), 1);
    restarted.ack(1, 7, None, None).unwrap();
    assert!(NotifyFile::load(&path).unwrap().outbox.is_empty());
}

/// A snapshot published just before a crash must not apply its old receipts twice.
#[test]
fn checkpoint_cursor_makes_old_receipts_idempotent() {
    let root = TempRoot::new("receipt-cursor");
    let path = root.path("notifications.json");
    let mut store = NotifyStore::open(path.clone()).unwrap();
    store.enqueue(7, "XALPHA".into(), None, 1).unwrap();
    store.ack(0, 7, None, None).unwrap();
    store.file.save(&path).unwrap();
    let mut restarted = NotifyStore::open(path.clone()).unwrap();
    assert_eq!(restarted.file, store.file);
    restarted.enqueue(7, "UNIT_01".into(), None, 2).unwrap();
    restarted.ack(1, 7, None, None).unwrap();
    assert!(NotifyFile::load(&path).unwrap().outbox.is_empty());
    assert_eq!(restarted.file.ack_sequence, 2);
}

/// A failed receipt write leaves the row and its ledger untouched for the existing ack retry.
#[test]
fn a_failed_receipt_does_not_advance_memory() {
    let root = TempRoot::new("receipt-fail");
    let path = root.path("notifications.json");
    let mut store = NotifyStore::open(path.clone()).unwrap();
    store.enqueue(7, "XALPHA".into(), None, 1).unwrap();
    let before = store.file.clone();
    std::fs::create_dir(crate::config::paths::telegram_notify_receipts(&path)).unwrap();
    assert!(store.ack(0, 7, None, None).is_err());
    assert_eq!(store.file, before);
}

/// Time sender bookkeeping and durable acknowledgements at 10000 unbounded synthetic cards.
#[test]
#[ignore = "scale probe"]
fn probe_notify_outbox() {
    let root = TempRoot::new("scale");
    let path = root.path("notifications.json");
    let mut store = NotifyStore::open(path).unwrap();
    store
        .update(|file| {
            for id in 0..10_000 {
                file.outbox.push(crate::telegram::notify::Pending {
                    id,
                    chat: id as i64 % 10 + 1,
                    html: "XALPHA ".repeat(64),
                    cores: Some(vec![1]),
                    ..Default::default()
                });
            }
            file.next_id = 10_000;
        })
        .unwrap();
    store.publish_allowed((1..=10).map(|chat| (chat, None)).collect());
    let store = Mutex::new(store);
    let start = std::time::Instant::now();
    for _ in 0..10 {
        std::hint::black_box(super::snapshot(&store));
    }
    eprintln!(
        "probe snapshot: {:.3} ms",
        start.elapsed().as_secs_f64() * 100.0
    );
    let start = std::time::Instant::now();
    for id in 9_000..10_000 {
        std::hint::black_box(super::hold(&store, id, id as i64 % 10 + 1));
    }
    eprintln!("probe hold: {:.6} ms", start.elapsed().as_secs_f64());
    let start = std::time::Instant::now();
    for id in 0..10 {
        store
            .lock()
            .unwrap()
            .ack(id, id as i64 % 10 + 1, None, None)
            .unwrap();
    }
    eprintln!("probe ack: {:.3} ms", start.elapsed().as_secs_f64() * 100.0);
    let guard = store.lock().unwrap();
    let bytes = std::mem::size_of_val(&guard.file.outbox)
        + guard
            .file
            .outbox
            .iter()
            .map(|row| {
                std::mem::size_of_val(row)
                    + row.html.capacity()
                    + row.cores.as_ref().map_or(0, |ids| ids.capacity() * 8)
            })
            .sum::<usize>();
    eprintln!("probe queued card payload memory (10000): {bytes} bytes");
    assert_eq!(guard.file.outbox.len(), 9_990);
}

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

/// An enqueue from another thread wakes a waiting sender well before its slice runs out.
#[test]
fn a_cross_thread_enqueue_wakes_the_waiter() {
    let root = TempRoot::new("wake-enqueue");
    let store = std::sync::Arc::new(Mutex::new(
        NotifyStore::open(root.path("notifications.json")).expect("open"),
    ));
    let wake = std::sync::Arc::clone(&store.lock().unwrap().wake);
    let seen = wake.generation();
    let writer = std::sync::Arc::clone(&store);
    let join = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(50));
        writer
            .lock()
            .unwrap()
            .enqueue(1, "<b>hi</b>".into(), Some(vec![]), 0)
            .expect("enqueue");
        std::time::Instant::now()
    });
    assert!(wake.wait_past(seen, std::time::Duration::from_secs(10)));
    let woke = std::time::Instant::now();
    // Measured from the save's return: the fsync itself is the disk's time, not the wake's.
    let saved = join.join().unwrap();
    assert!(woke.saturating_duration_since(saved) < std::time::Duration::from_millis(500));
}

/// With nothing changing, the wait returns `false` after its timeout.
#[test]
fn an_unchanged_generation_times_out() {
    let wake = super::NotifyWake::default();
    let started = std::time::Instant::now();
    assert!(!wake.wait_past(wake.generation(), std::time::Duration::from_millis(60)));
    assert!(started.elapsed() >= std::time::Duration::from_millis(60));
}

/// Stopping rings the wake, so a sender mid-wait leaves at once rather than after the slice.
#[test]
fn stop_wakes_the_waiter_at_once() {
    let wake = std::sync::Arc::new(super::NotifyWake::default());
    let seen = wake.generation();
    let stopper = std::sync::Arc::clone(&wake);
    let started = std::time::Instant::now();
    let join = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(20));
        stopper.bump();
    });
    assert!(wake.wait_past(seen, std::time::Duration::from_secs(5)));
    assert!(started.elapsed() < std::time::Duration::from_millis(500));
    join.join().unwrap();
}
