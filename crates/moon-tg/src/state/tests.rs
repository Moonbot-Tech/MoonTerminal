//! Cleanup identity regressions use empty credentials, so no transport is launched.

use super::TelegramState;
use moon_core::config::{Secret, TelegramConfig};

/// A detached read must retain exclusive admission even when its bot service is replaced.
#[test]
fn pending_report_survives_service_replacement() {
    let config = TelegramConfig::default();
    let mut state = TelegramState::new(&config, crate::HostKind::Terminal);
    state.report_pending = true;
    state.start_saved(&config);
    assert!(state.report_pending);
}

/// The same restart helper used after pairing reset must retain retired menu recipients.
#[test]
fn revoked_menu_identities_survive_service_replacement() {
    let before = TelegramConfig {
        authorized_chat_ids: vec![7, 8],
        ..TelegramConfig::default()
    };
    let saved = TelegramConfig {
        authorized_chat_ids: vec![8],
        ..TelegramConfig::default()
    };
    let mut state = TelegramState::new(&before, crate::HostKind::Terminal);
    state.remember_menu_cleanup(&before, &saved);
    state.restart();
    state.start_saved(&saved);
    assert_eq!(state.retired_menu_chats, vec![7]);
    assert!(
        state.service.is_none(),
        "empty test credentials must not start transport"
    );
    state.start_saved(&saved);
    assert_eq!(
        state.retired_menu_chats,
        vec![7],
        "a second retirement must not lose pending cleanup"
    );
}

/// A new bot must never receive cleanup requests for the previous bot's users.
#[test]
fn token_change_discards_old_bot_menu_identities() {
    let before = TelegramConfig {
        authorized_chat_ids: vec![7],
        ..TelegramConfig::default()
    };
    let saved = TelegramConfig::default();
    let mut state = TelegramState::new(&before, crate::HostKind::Terminal);
    state.remember_menu_cleanup(&before, &saved);
    let new_bot = TelegramConfig {
        token: Secret::new("fixture-only"),
        ..saved.clone()
    };
    // Only compare credential identities; never start the non-empty fixture credential.
    state.remember_menu_cleanup(&saved, &new_bot);
    assert!(state.retired_menu_chats.is_empty());
}

/// Isolated temp root. Removed when the test drops it, including on panic.
struct TempRoot(std::path::PathBuf);

impl TempRoot {
    fn new(tag: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "moon-tg-notify-state-{}-{tag}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("temp root");
        Self(root)
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// An empty token must not create the notifications directory. The path stays for a later start.
#[test]
fn empty_token_does_not_create_the_notifications_directory() {
    let root = TempRoot::new("empty");
    let path = root.0.join("nested").join("notifications.json");
    let config = TelegramConfig::default();
    let state = TelegramState::new_with_notifications(
        &config,
        crate::HostKind::Terminal,
        Some(path.clone()),
    );
    assert!(state.service.is_none());
    assert!(
        !root.0.join("nested").exists(),
        "an empty token must not create the notifications directory"
    );
    assert_eq!(state.notifications_path.as_deref(), Some(path.as_path()));
    let mut state = state;
    state.start_saved(&config);
    assert_eq!(state.notifications_path.as_deref(), Some(path.as_path()));
    assert!(!root.0.join("nested").exists());
}

/// A service restart keeps the in-flight notification read and drops the in-memory down gate.
#[test]
fn start_saved_keeps_notify_busy_and_clears_the_down_gate() {
    use std::time::Instant;

    use crate::ReportRevision;
    use crate::notify::down::DownTracker;

    let config = TelegramConfig::default();
    let mut state = TelegramState::new(&config, crate::HostKind::Station);
    state.notify_busy = true;
    state.down_trackers.insert(7, DownTracker::default());
    state.last_notify_run = Some(Instant::now());
    state.last_report_revision = Some(ReportRevision::from_parts(1, 2, 3, 4));
    state.start_saved(&config);
    assert!(state.notify_busy);
    assert!(state.down_trackers.is_empty());
    assert!(state.last_notify_run.is_none());
    assert!(state.last_report_revision.is_none());
    assert!(
        state.service.is_none(),
        "empty test credentials must not start transport"
    );
}

/// Unpairing removes that chat's settings. A missing chat reads back all-off, and its outbox
/// row stays so the sender can drop it. A second pass with the same chats does not rewrite.
#[test]
fn forget_unpaired_removes_the_chat_and_keeps_its_outbox_row() {
    use moon_core::telegram::notify::{ChatNotify, NotifyFile, NotifySettings, Pending};
    use moon_core::telegram::runtime::NotifyStore;

    let root = TempRoot::new("unpair");
    let path = root.0.join("notifications.json");
    let mut store = NotifyStore {
        path: path.clone(),
        file: NotifyFile::default(),
        allowed: None,
    };
    store
        .update(|file| {
            file.chats.insert(
                7,
                ChatNotify {
                    revision: 4,
                    ..ChatNotify::default()
                },
            );
            file.chats.insert(
                8,
                ChatNotify {
                    revision: 9,
                    ..ChatNotify::default()
                },
            );
            file.outbox.push(Pending {
                id: 1,
                chat: 7,
                html: "keep".into(),
                created_utc: 10,
                cores: None,
                auto: None,
                ..Pending::default()
            });
            file.next_id = 2;
        })
        .expect("seed");

    let removed = store.forget_unpaired(&[8]).expect("forget");
    assert_eq!(removed, 1);
    assert!(!store.file.chats.contains_key(&7));
    assert_eq!(store.file.chats[&8].revision, 9);
    assert_eq!(store.file.outbox.len(), 1);
    assert_eq!(store.file.outbox[0].html, "keep");
    assert_eq!(store.file.outbox[0].chat, 7);
    let loaded = NotifyFile::load(&path).expect("reload");
    assert_eq!(loaded, store.file);
    let reread = store
        .file
        .chats
        .get(&7)
        .map(|chat| chat.settings.clone())
        .unwrap_or_default();
    assert_eq!(reread, NotifySettings::default());
    assert!(!reread.trades.on);

    let bytes = std::fs::read(&path).expect("bytes");
    assert_eq!(store.forget_unpaired(&[8]).expect("noop"), 0);
    assert_eq!(std::fs::read(&path).expect("bytes after noop"), bytes);
}
