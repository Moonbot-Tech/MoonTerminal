//! Localized reports-access guidance and recovery precedence regressions.

use super::recovery_notice_text;
use moon_core::db::report_recovery::RecoveryNotice;

/// Removing the shared recording warning or any lease check leaves a user unaware of the gap.
/// Mapping the lease to failed recovery also changes the independently specified title below.
#[test]
fn report_lease_notice_explains_checks_and_catch_up_in_each_locale() {
    for (locale, title, required) in [
        (
            "en",
            "Could not lock the reports replica.",
            vec![
                "second terminal",
                "network drive",
                "cloud-synced",
                "permissions",
                "free space",
                "antivirus",
                "not being recorded",
                "restarting",
                "checkpoint",
                "no longer retains",
            ],
        ),
        (
            "ru",
            "Не удалось получить исключительный доступ к реплике отчётов.",
            vec![
                "вторая копия",
                "сетевого диска",
                "облачной",
                "права доступа",
                "свободное место",
                "антивирус",
                "не сохраняются",
                "перезапуска",
                "метка синхронизации",
                "уже не хранит",
            ],
        ),
        (
            "es",
            "No se pudo bloquear la réplica de informes.",
            vec![
                "otro terminal",
                "unidad de red",
                "nube",
                "permisos",
                "espacio libre",
                "antivirus",
                "no se están guardando",
                "reiniciar",
                "punto de sincronización",
                "ya no conserva",
            ],
        ),
    ] {
        let _locale = crate::test_locale::force(locale);
        let (actual_title, detail) =
            recovery_notice_text(Some(&RecoveryNotice::LeaseUnavailable {
                detail: "internal lease diagnostic must stay out of the UI".into(),
            }));
        assert_eq!(actual_title, title);
        for phrase in required {
            assert!(
                detail.contains(phrase),
                "missing lease guidance for {locale}"
            );
        }
        assert!(!detail.contains("internal lease diagnostic"));
    }
}

/// Replacing recovery facts with the generic lease fallback loses the saved-copy location.
/// Appending the stopped-writer warning to successful recovery falsely reports recording as off.
#[test]
fn report_recovery_preserves_captions_paths_and_writer_state() {
    let _locale = crate::test_locale::force("en");
    let snapshot = std::path::PathBuf::from("saved-copy");
    let (title, detail) = recovery_notice_text(Some(&RecoveryNotice::Blocked {
        detail: "technical diagnostic".into(),
        snapshot_dir: Some(snapshot.clone()),
    }));
    assert_eq!(title, "Automatic reports recovery was stopped.");
    assert!(detail.contains("Previous copy: saved-copy."));
    assert!(detail.contains("not being recorded"));
    let (title, detail) = recovery_notice_text(Some(&RecoveryNotice::Failed {
        detail: "technical diagnostic".into(),
    }));
    assert_eq!(title, "Safe automatic reports recovery did not complete.");
    assert!(detail.contains("published recovery folder"));
    assert!(detail.contains("not being recorded"));
    let (title, detail) = recovery_notice_text(Some(&RecoveryNotice::Recovered {
        snapshot_dir: snapshot,
    }));
    assert_eq!(title, "The damaged reports replica was safely replaced.");
    assert!(detail.contains("preserved in saved-copy."));
    assert!(!detail.contains("not being recorded"));
}

/// A denial before notice publication must still avoid raw diagnostics and explain recording.
#[test]
fn report_access_denial_without_notice_still_warns_about_recording() {
    let _locale = crate::test_locale::force("en");
    let (title, detail) = recovery_notice_text(None);
    assert_eq!(title, "Access to the reports replica is unavailable.");
    assert!(detail.contains("not being recorded"));
    assert!(detail.contains("restarting the terminal"));
}

/// A check still in flight is not a fault: the notice must say that history is readable and that
/// recording starts by itself. Routing it through the stopped-recording paragraph tells the user
/// to fix a cause and restart, and a restart only starts the same check over.
#[test]
fn running_check_notice_promises_recording_without_asking_for_a_restart() {
    let _locale = crate::test_locale::force("en");
    let (title, detail) = recovery_notice_text(Some(&RecoveryNotice::CheckRunning));
    assert_eq!(
        title,
        "The reports replica integrity check is still running."
    );
    assert!(detail.contains("history remains available for reading"));
    assert!(detail.contains("once the check finishes"));
    assert!(!detail.contains("restarting"));
    assert!(!detail.contains("not being recorded in this reports replica"));
}

/// An inconclusive scan must explain readable history and stopped recording without falsely
/// blaming a second terminal, asserting corruption, or displaying internal diagnostics.
#[test]
fn incomplete_check_notice_describes_the_actual_access_mode() {
    let _locale = crate::test_locale::force("en");
    let (title, detail) = recovery_notice_text(Some(&RecoveryNotice::CheckIncomplete {
        detail: "fixture internal watchdog diagnostic".into(),
    }));
    assert_eq!(title, "The replica integrity check did not complete.");
    assert!(detail.contains("history remains available for reading"));
    assert!(detail.contains("not being recorded"));
    assert!(detail.contains("diagnostic log"));
    assert!(!detail.contains("second terminal"));
    assert!(!detail.contains("fixture internal"));
}

/// The three states of the user's set-aside must each say what is true of recording.
///
/// Routing the running set-aside through the stopped-recording paragraph tells the user to fix a
/// cause and restart while recording is seconds from starting by itself; dropping that paragraph
/// from a failed one hides that nothing is recorded; appending it to a started replica claims
/// the opposite of what happened, and losing the path loses where the old files went.
#[test]
fn fresh_replica_notices_state_where_the_files_are_and_whether_recording_runs() {
    let _locale = crate::test_locale::force("en");
    let (title, detail) = recovery_notice_text(Some(&RecoveryNotice::FreshReplicaRunning));
    assert_eq!(title, "Setting the reports replica aside.");
    assert!(detail.contains("recording starts right after it"));
    assert!(!detail.contains("not being recorded"));

    let (title, detail) = recovery_notice_text(Some(&RecoveryNotice::FreshReplicaFailed));
    assert_eq!(title, "The reports replica could not be set aside.");
    assert!(detail.contains("left where they were"));
    assert!(detail.contains("not being recorded"));

    let (title, detail) = recovery_notice_text(Some(&RecoveryNotice::FreshReplicaStarted {
        snapshot_dir: std::path::PathBuf::from("saved-copy"),
    }));
    assert_eq!(title, "A fresh reports replica was started.");
    assert!(detail.contains("preserved in saved-copy."));
    assert!(!detail.contains("not being recorded"));
}
