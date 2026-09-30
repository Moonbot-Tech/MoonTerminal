//! Restart fixtures: dropping the in-memory job must not release a persisted token.
use super::{Decision, Pending, decide, load, save};
use moon_core::station_api::BotStatus;
use moon_core::telegram::TelegramStatus;
use moon_core::telegram::runtime::mini_app::MiniAppStatus;
use moon_remote::ssh::Target;
use moon_remote::station::bot::BotState;

/// Removing the journal publication loses crash recovery and permits a second poller.
#[test]
fn restart_retains_destination_and_erase_retry_until_atomic_clear() {
    let dir = std::env::temp_dir().join(format!("moon-handover-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("journal.json");
    let mut pending = Pending::new(&Target {
        host: "192.0.2.7".into(),
        port: 2222,
    });
    save(&path, Some(&pending)).unwrap();
    let restored = load(&path).unwrap().unwrap();
    assert_eq!(restored.target().addr(), "192.0.2.7:2222");
    assert!(!restored.erase_pending);
    pending.erase_pending = true;
    save(&path, Some(&pending)).unwrap();
    assert!(load(&path).unwrap().unwrap().erase_pending);
    save(&path, None).unwrap();
    assert!(load(&path).unwrap().is_none());
    std::fs::remove_file(&path).unwrap();
    assert!(load(&path).unwrap().is_none());
    std::fs::remove_dir(&dir).unwrap();
}

/// Treating a corrupt/read-failed marker as absence starts the saved bot beside the station.
#[test]
fn malformed_or_unreadable_journal_never_means_no_handover() {
    let dir = std::env::temp_dir().join(format!("moon-handover-invalid-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("journal.json");
    std::fs::write(&path, b"{").unwrap();
    assert!(load(&path).is_err());
    assert!(load(&dir).is_err());
    assert!(save(&path.join("child"), None).is_err());
    assert!(load(&path).is_err());
    std::fs::remove_file(&path).unwrap();
    std::fs::remove_dir(&dir).unwrap();
}

/// Erasing on polling alone loses a different local bot when the server already held a token.
#[test]
fn interrupted_transfer_holds_even_while_an_unidentified_station_bot_polls() {
    let pending = Pending::new(&Target {
        host: "192.0.2.7".into(),
        port: 22,
    });
    let mut state = BotState {
        has_token: true,
        stopped: true,
        ..BotState::default()
    };
    assert_eq!(decide(&pending, &state), Decision::Hold);
    state.stopped = false;
    state.no_api = true;
    assert_eq!(decide(&pending, &state), Decision::Hold);
    state.no_api = false;
    state.bot = Some(BotStatus {
        status: TelegramStatus::Starting,
        mini_app_on: false,
        mini_app: MiniAppStatus::Stopped,
        pairing: None,
    });
    assert_eq!(decide(&pending, &state), Decision::Hold);
    state.bot.as_mut().unwrap().status = TelegramStatus::Unpaired;
    assert_eq!(decide(&pending, &state), Decision::Hold);
    state.bot.as_mut().unwrap().status = TelegramStatus::Paired { chat_count: 2 };
    assert_eq!(decide(&pending, &state), Decision::Hold);
    state.bot = None;
    state.has_token = false;
    assert_eq!(decide(&pending, &state), Decision::Resume);
    state.stopped = true;
    assert_eq!(decide(&pending, &state), Decision::Resume);
}

/// Prioritizing erase_pending over proven absence deletes the local bot after "Take bot off".
#[test]
fn taking_bot_off_returns_ownership_after_a_failed_local_erase() {
    let mut pending = Pending::new(&Target {
        host: "192.0.2.7".into(),
        port: 22,
    });
    pending.erase_pending = true;
    assert_eq!(decide(&pending, &BotState::default()), Decision::Resume);
    assert_eq!(
        decide(
            &pending,
            &BotState {
                has_token: true,
                ..BotState::default()
            }
        ),
        Decision::Erase
    );
    assert_eq!(
        decide(
            &pending,
            &BotState {
                has_token: true,
                stopped: true,
                ..BotState::default()
            }
        ),
        Decision::Hold
    );
}
