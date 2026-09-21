//! Classified load-state presentation regression tests.

use super::{
    DbReadFailedNotice, LoadState, Note, db_read_failed_hint, db_read_failed_hint_key,
    db_read_failed_retryable,
};
use moon_core::db::{FailKind, ReadFail};
use std::sync::Arc;

/// Construct a classified read failure without coupling tests to a rendered error message.
fn failure(kind: FailKind) -> ReadFail {
    ReadFail::Failed {
        kind,
        msg: Arc::from("test failure"),
    }
}

/// `load_state.rs:LoadState::apply_or_keep` must retain only a settled stale snapshot when a
/// report catch-up gets Busy. Keeping a NotReady result would show numbers from an unavailable
/// replica, while dropping a valid stale snapshot makes the Analytics surface flash to an error.
#[test]
fn catch_up_failure_keeps_only_a_real_stale_snapshot() {
    let mut stale = LoadState::Ready(Arc::new(vec![42]));
    stale.begin();
    stale.apply_or_keep(Err(failure(FailKind::Busy)), true);
    assert!(
        matches!(&stale, LoadState::Ready(_)),
        "a Busy revalidation restores the settled stale snapshot"
    );

    let mut no_stale = LoadState::<Vec<i32>>::default();
    no_stale.apply_or_keep(Err(failure(FailKind::Busy)), true);
    assert!(
        matches!(&no_stale, LoadState::Failed(_)),
        "an initial read failure has no snapshot to preserve"
    );

    let mut unavailable = LoadState::Ready(Arc::new(vec![7]));
    unavailable.begin();
    unavailable.apply_or_keep(Err(ReadFail::NotReady), true);
    assert!(
        matches!(&unavailable, LoadState::NotReady),
        "NotReady is a completed unavailable-replica answer, never stale data"
    );
}

/// `load_state.rs:LoadState::apply_or_keep` must publish a failure when preservation is disabled.
/// Keeping the old snapshot after a user scope change would put the prior scope's numbers under
/// the new label, which is worse than a visible classified read failure.
#[test]
fn scope_changes_do_not_keep_the_previous_snapshot_after_a_failure() {
    let mut state = LoadState::Ready(Arc::new(vec![42]));
    state.begin();
    state.apply_or_keep(Err(failure(FailKind::Busy)), false);
    assert!(
        matches!(&state, LoadState::Failed(_)),
        "a non-preserving scope change must surface the completed failure"
    );

    state.apply_or_keep(Ok(vec![9]), true);
    assert!(
        matches!(&state, LoadState::Ready(_)),
        "a successful replacement still settles normally"
    );
}

/// Incomparable quote scope is guidance, not a reports-database failure.
///
/// Mapping `ReadFail::IncomparableQuote` through the generic failure branch in
/// `load_state.rs:LoadState::view` makes a healthy mixed-currency tuner scope tell the user that
/// the database could not be read.
#[test]
fn incomparable_quote_has_a_non_database_note() {
    let mut state = LoadState::<Vec<()>>::default();
    state.apply(Err(ReadFail::IncomparableQuote));

    assert!(matches!(
        state.view(Vec::is_empty),
        Err(Note::IncomparableQuote)
    ));
}

/// `load_state.rs:db_read_failed_hint_key` must keep each FailKind on the locale key Analytics
/// and Report already ship. Mapping Busy onto the corrupt key would tell the user retrying cannot
/// help while the lock is still the one thing a retry clears.
#[test]
fn db_read_failed_hint_key_matches_the_published_common_keys() {
    assert_eq!(
        db_read_failed_hint_key(FailKind::Corrupt),
        Some("common.db_read_failed_corrupt")
    );
    assert_eq!(
        db_read_failed_hint_key(FailKind::Busy),
        Some("common.db_read_failed_retry")
    );
    assert_eq!(
        db_read_failed_hint_key(FailKind::Other),
        Some("common.db_read_failed_other")
    );
    assert_eq!(db_read_failed_hint_key(FailKind::ReplicaAccessDenied), None);
    assert_eq!(
        DbReadFailedNotice::of(FailKind::ReplicaAccessDenied),
        DbReadFailedNotice::Recovery
    );
}

/// `load_state.rs:db_read_failed_retryable` must hide Retry for the two kinds that cannot recover.
/// Offering the button on Corrupt or ReplicaAccessDenied teaches the user that buttons do nothing.
#[test]
fn db_read_failed_retryable_only_for_busy_and_other() {
    assert!(db_read_failed_retryable(FailKind::Busy));
    assert!(db_read_failed_retryable(FailKind::Other));
    assert!(!db_read_failed_retryable(FailKind::Corrupt));
    assert!(!db_read_failed_retryable(FailKind::ReplicaAccessDenied));
}

/// `load_state.rs:db_read_failed_hint` must emit the published common.yml copy in each locale.
/// Collapsing every kind onto `chart.trade_history.failed` is the badge the user reported.
#[test]
fn db_read_failed_hint_names_the_cause_in_english_and_russian() {
    for (locale, busy, corrupt, other, denial_title) in [
        (
            "en",
            "The reports database is busy right now. Retry — the period will recompute.",
            "The database file is damaged — retrying will not help. See the Log tab.",
            "This is a read error, not an absence of trades. See the Log tab for details.",
            "Access to the reports replica is unavailable.",
        ),
        (
            "ru",
            "База отчётов сейчас занята. Повторите — период пересчитается.",
            "Файл базы повреждён — повтор не поможет. Подробности во вкладке «Лог».",
            "Это ошибка чтения, а не отсутствие сделок. Подробности — во вкладке «Лог».",
            "Доступ к реплике отчётов закрыт.",
        ),
    ] {
        let _locale = crate::test_locale::force(locale);
        assert_eq!(db_read_failed_hint(FailKind::Busy), busy);
        assert_eq!(db_read_failed_hint(FailKind::Corrupt), corrupt);
        assert_eq!(db_read_failed_hint(FailKind::Other), other);
        let denial = db_read_failed_hint(FailKind::ReplicaAccessDenied);
        assert!(
            denial.starts_with(denial_title),
            "{locale}: lease denial must open with the recovery title, got {denial}"
        );
        assert_ne!(busy, corrupt);
        assert_ne!(busy, other);
        assert_ne!(corrupt, other);
    }
}
