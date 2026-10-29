//! Causal-wake regression tests without a bot token or network.
use super::*;

/// Waking before publication can lose a command until the next periodic tick; a full channel
/// must not emit a wake for work it refused.
#[test]
fn accepted_work_is_visible_before_wake_and_full_work_does_not_wake() {
    let (tx, rx) = mpsc::sync_channel(1);
    let seen = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count = seen.clone();
    let receiver = Arc::new(Mutex::new(rx));
    let check_receiver = receiver.clone();
    let waker: SharedWaker = Arc::new(Mutex::new(Some(Arc::new(move || {
        assert!(matches!(
            check_receiver.lock().unwrap().try_recv(),
            Ok(Work::Status(crate::telegram::TelegramStatus::Starting))
        ));
        count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }))));
    let sender = WorkSender { tx, waker };
    assert!(sender.try_send(Work::Status(crate::telegram::TelegramStatus::Starting)));
    assert_eq!(seen.load(std::sync::atomic::Ordering::SeqCst), 1);
    let count = seen.clone();
    *sender.waker.lock().unwrap() = Some(Arc::new(move || {
        count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }));
    assert!(sender.try_send(Work::Status(crate::telegram::TelegramStatus::Starting)));
    assert!(!sender.try_send(Work::Status(crate::telegram::TelegramStatus::Starting)));
    assert_eq!(seen.load(std::sync::atomic::Ordering::SeqCst), 2);
}

/// The callback must run without holding its registration lock: hosts may rebind a wake during
/// service replacement, and a callback holding the mutex would deadlock that path.
#[test]
fn callback_runs_outside_registration_lock() {
    let (tx, _rx) = mpsc::sync_channel(1);
    let shared: SharedWaker = Arc::default();
    let callback_shared = shared.clone();
    let (called, call_rx) = mpsc::channel();
    *shared.lock().unwrap() = Some(Arc::new(move || {
        assert!(callback_shared.try_lock().is_ok());
        called.send(()).unwrap();
    }));
    assert!(
        WorkSender {
            tx,
            waker: shared.clone(),
        }
        .try_send(Work::Status(crate::telegram::TelegramStatus::Starting))
    );
    call_rx.try_recv().expect("the callback must actually run");
    *shared.lock().unwrap() = None;
}
