//! Fixture-only restart and deletion-age coverage; no Bot API requests.
use super::{Answer, History};

/// Exact temporary file ownership keeps fixture cleanup independent of the application data root.
struct Fixture(std::path::PathBuf);
impl Drop for Fixture {
    /// Remove only this test's file, including on assertion failure.
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Reload from disk as a new process would, retaining menu ownership and original send time.
#[test]
fn restart_restores_answers_and_navigation_separately() {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let file = Fixture(std::env::temp_dir().join(format!(
        "telegram-history-{}-{nonce}.json",
        std::process::id()
    )));
    let mut history = History::default();
    history.navigation.insert(7, 10);
    history.answers.insert(
        7,
        Answer {
            id: 11,
            sent_at: 1000,
        },
    );
    history.save(&file.0).unwrap();
    let mut restored = History::load(&file.0);
    assert_eq!(restored.navigation.get(&7), Some(&10));
    let previous = restored
        .answers
        .insert(
            7,
            Answer {
                id: 12,
                sent_at: 1100,
            },
        )
        .unwrap();
    assert_eq!(previous.id, 11);
    assert_eq!(previous.sent_at, 1000);
    assert!(previous.deletable(1100));
    restored.save(&file.0).unwrap();
    assert_eq!(History::load(&file.0).answers[&7].id, 12);
    std::fs::write(&file.0, b"broken json").unwrap();
    assert!(History::load(&file.0).answers.is_empty());
}

/// Unknown timestamps, future timestamps and the deadline margin must never trigger deletion.
#[test]
fn expired_and_unknown_answers_are_not_deleted() {
    let answer = Answer {
        id: 11,
        sent_at: 1000,
    };
    assert!(answer.deletable(1000 + 47 * 3600));
    assert!(!answer.deletable(1000 + 48 * 3600 - 60));
    assert!(!answer.deletable(1000 + 48 * 3600));
    assert!(!answer.deletable(999));
    assert!(!Answer { id: 11, sent_at: 0 }.deletable(1000));
}

/// Switching to a different bot cannot load outgoing IDs owned by the previous bot.
#[test]
fn bot_identity_scopes_persistence() {
    assert_ne!(
        crate::config::paths::telegram_chat_history(1),
        crate::config::paths::telegram_chat_history(2)
    );
}
