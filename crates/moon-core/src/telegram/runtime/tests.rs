//! Runtime-only regression tests for the Telegram transport owner.

use super::*;

const RUNTIME_SOURCE: &str = include_str!("../runtime.rs");
const EMPTY_TOKEN_GUARD: &str =
    "        if config.token.is_empty() {\n            return None;\n        }\n";
const FIRST_SERVICE_CONSTRUCTION: &str = "        let alive = Arc::new(());";

/// `telegram/runtime.rs:TelegramService::start_localized` must keep the empty-token guard before
/// its first service construction; removing that guard would let a default installation create
/// Telegram background transport and reach network code although the feature is disabled.
#[test]
fn empty_token_guard_dominates_telegram_service_construction() {
    let guard_offset = RUNTIME_SOURCE
        .find(EMPTY_TOKEN_GUARD)
        .expect("the empty-token off switch must remain as a complete guard block");
    let construction_offset = RUNTIME_SOURCE
        .find(FIRST_SERVICE_CONSTRUCTION)
        .expect("start_localized must still identify its first service construction");

    assert!(
        guard_offset < construction_offset,
        "the empty-token guard must dominate service construction before any transport can start"
    );

    let config = TelegramConfig::default();
    assert!(
        TelegramService::start(&config, None).is_none(),
        "an empty credential must be the hard off switch rather than an inactive service handle"
    );
}

/// Notification consumers must poll rather than block the UI during owner-side loading.
///
/// A blocking getter would put document I/O back onto the 100 ms coordination loop; an empty
/// default would instead let the first host edit overwrite the persisted ledger and outbox.
#[test]
fn notification_slot_is_nonblocking_until_the_terminal_load_result() {
    let slot = NotificationSlot::new();
    assert!(
        slot.get().is_none(),
        "a pending load must not block or fabricate a store"
    );
    slot.complete(None);
    assert!(slot.get().is_none(), "a failed load stays unavailable");
}

/// Measure loading a synthetic 72-hour notification document before the first group window.
#[test]
#[ignore = "synthetic startup Telegram scale probe"]
fn probe_startup_telegram_load() {
    let root = std::env::temp_dir().join(format!(
        "moon-terminal-startup-notify-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("notifications.json");
    let mut file = crate::telegram::notify::NotifyFile::default();
    for chat in 1..=10 {
        for core in 1..=10u64 {
            let seen = file
                .chats
                .entry(chat)
                .or_default()
                .ledger
                .seen
                .entry(core)
                .or_default();
            for rec_id in 1..=500 {
                seen.insert(rec_id, 1_700_000_000 + rec_id);
            }
        }
    }
    for id in 0..2_000i64 {
        push_outbox(&mut file, 7, format!("COIN_A close {id}"), None, id);
    }
    file.save(&path).unwrap();

    let mut load_ms = Vec::new();
    for _ in 0..3 {
        let started = std::time::Instant::now();
        let store = notify::NotifyStore::open(path.clone()).unwrap();
        load_ms.push(started.elapsed().as_secs_f64() * 1000.0);
        assert_eq!(store.file.outbox.len(), 2_000);
        assert_eq!(store.file.chats.len(), 10);
    }
    load_ms.sort_by(f64::total_cmp);
    println!(
        "probe_startup_telegram chats=10 closes=50000 queued=2000 load_ms={load_ms:?} median_ms={:.3}",
        load_ms[1]
    );
    std::fs::remove_dir_all(root).unwrap();
}
