//! Bot ownership, Settings draft preservation and station access lifecycle decisions.
use super::{StationJobs, can_restore_bot, job, restore_bot, save_returned_bot};
use crate::test_support::SharedWindowGuard as RestoreWindow;
use moon_core::config::AppConfig;
use moon_core::config::telegram_access::TelegramChatAccess;
use moon_core::config::{Secret, TelegramConfig};
use moon_core::market::trade_replay;
use moon_core::station_api::Access;
use moon_core::station_api::TapeWindow;
use moon_remote::hosts::Host;
use moon_remote::ssh::Target;
use moon_remote::station::access::AddressChange;
use moon_remote::station::bot::ReturnedBot;

/// Supply an isolated destination without contacting the station or reading saved hosts.
fn window_target() -> Target {
    Target {
        host: "synthetic.invalid".into(),
        port: 22,
    }
}

/// Build the real tape job used by the production admission helper.
fn window_job(tape: TapeWindow) -> job::Job {
    job::Job::Tape {
        target: window_target(),
        tape,
        catchup: None,
        keep: None,
    }
}

/// Supply a bot response with an explicit capability report, including an unsupported station.
fn window_answer(tape: Option<TapeWindow>) -> job::Done {
    let job::Done::Ok {
        bot: Some(mut bot), ..
    } = synthetic_station_answer()
    else {
        unreachable!();
    };
    bot.tape = tape;
    job::Done::Ok {
        transferred: false,
        bot: Some(bot),
        bot_off: false,
    }
}

/// A successful Tape job reports no bot, so the completion must schedule verification.
fn window_ok() -> job::Done {
    job::Done::Ok {
        transferred: false,
        bot: None,
        bot_off: false,
    }
}

/// A failed window write is latched rather than retried immediately from its own end.
fn window_failed() -> job::Done {
    job::Done::Failed {
        reason: "synthetic timeout".into(),
        station_may_poll: false,
    }
}

/// Automatic zone failures must not refill the failed window write's attempt budget.
#[test]
fn barrier_b6_zone_and_window_failures_stop_alternating() {
    let local = TapeWindow {
        margin_s: 600,
        long_position_min: 30,
    };
    let old = TapeWindow {
        margin_s: 180,
        long_position_min: 5,
    };
    let mut jobs = StationJobs {
        window_reconciled: true,
        ..StationJobs::default()
    };
    jobs.apply_quiet(window_answer(Some(old)));
    let mut writes = 0;
    for _ in 0..8 {
        if jobs.window_step(local).push.is_none() {
            break;
        }
        writes += 1;
        jobs.begin_job(super::Kind::Window, &window_job(local));
        jobs.apply_window(window_failed());
        jobs.begin_job(
            super::Kind::Zone,
            &job::Job::BotState {
                target: window_target(),
            },
        );
        jobs.apply_quiet(window_failed());
    }
    assert!(writes <= 2, "automatic jobs looped {writes} times");
    assert!(jobs.window_step(local).push.is_none());
}

/// An answered old station must disarm reconnect instead of being read every fifteen minutes.
#[test]
fn unsupported_station_disarms_reconnect_after_a_failed_read() {
    let mut jobs = StationJobs {
        known: true,
        ..StationJobs::default()
    };
    jobs.apply_quiet(window_failed());
    assert!(jobs.reconnect_at.is_some());
    jobs.apply_quiet(window_answer(None));
    assert!(jobs.reconnect_at.is_none());
    assert!(!jobs.reconnect_due(i64::MAX));
    jobs.apply_quiet(window_failed());
    assert!(
        jobs.reconnect_at.is_none(),
        "unsupported remains final until the station is forgotten"
    );
}

/// Unrelated completions may retry a write without repeating an unchanged failure in the UI.
#[test]
fn repeated_window_failure_is_logged_once_per_payload() {
    let mut jobs = StationJobs::default();
    let tape = TapeWindow {
        margin_s: 600,
        long_position_min: 30,
    };
    for _ in 0..3 {
        jobs.begin_job(super::Kind::Window, &window_job(tape));
        jobs.apply_window(window_failed());
        jobs.begin_job(super::Kind::Quiet, &window_job(tape));
        jobs.apply_quiet(window_answer(Some(tape)));
    }
    assert_eq!(jobs.lines, ["synthetic timeout"]);
}

/// Losing the component-wise maximum or releasing a latch on its own completion shrinks tape
/// or retries forever; unrelated reads and user work release a failed attempt.
#[test]
fn shared_window_reconciles_once_and_retries_only_after_unrelated_work() {
    use super::Kind;
    let local = TapeWindow {
        margin_s: 180,
        long_position_min: 5,
    };
    let larger = TapeWindow {
        margin_s: 600,
        long_position_min: 10,
    };
    let mixed = TapeWindow {
        margin_s: 600,
        long_position_min: 3,
    };
    let effective = TapeWindow {
        margin_s: 600,
        long_position_min: 5,
    };
    let mut jobs = StationJobs::default();
    jobs.apply_quiet(window_answer(Some(larger)));
    let step = jobs.window_step(local);
    assert_eq!(step.adopt, Some(larger));
    assert_eq!(step.push, None);
    assert!(step.reconciled);

    for unrelated in [Kind::Quiet, Kind::User] {
        let mut jobs = StationJobs::default();
        jobs.apply_quiet(window_answer(Some(mixed)));
        let step = jobs.window_step(local);
        assert_eq!(step.adopt, Some(effective));
        assert_eq!(step.push, Some(effective));
        jobs.begin_job(Kind::Window, &window_job(step.push.unwrap()));
        assert!(!jobs.busy(), "automatic writes must allow queued user work");
        jobs.apply_window(window_failed());
        assert_eq!(jobs.lines, ["synthetic timeout"]);
        assert!(!jobs.pending_refresh);
        assert_eq!(jobs.window_step(effective).push, None);
        let read = job::Job::BotState {
            target: window_target(),
        };
        jobs.begin_job(unrelated, &read);
        jobs.apply_quiet(window_answer(Some(mixed)));
        jobs.running = false;
        assert_eq!(jobs.window_step(effective).push, Some(effective));
        let lower = TapeWindow {
            margin_s: 120,
            long_position_min: 5,
        };
        let step = jobs.window_step(lower);
        assert_eq!(step.adopt, None);
        assert_eq!(step.push, Some(lower));
        jobs.begin_job(Kind::Window, &window_job(lower));
        jobs.apply_window(window_failed());
        let changed = TapeWindow {
            margin_s: 180,
            long_position_min: 5,
        };
        assert_eq!(jobs.window_step(changed).push, Some(changed));
    }
    let mut unsupported = StationJobs::default();
    unsupported.apply_quiet(window_answer(None));
    let step = unsupported.window_step(local);
    assert!(step.adopt.is_none() && step.push.is_none() && !step.reconciled);
}

/// Reusing the one-shot prefetch latch or retrying before the deadline loses offline startup
/// recovery; continued failures double the delay and a tape report stops reconnecting.
#[test]
fn shared_window_reconnect_obeys_deadlines_backoff_and_reconciliation() {
    use super::Kind;
    let mut jobs = StationJobs {
        known: true,
        ..StationJobs::default()
    };
    let (read, kind) = jobs.prefetch_read(Some(window_target())).unwrap();
    jobs.begin_job(kind, &read);
    jobs.running = false;
    jobs.apply_quiet(window_failed());
    let first = jobs.reconnect_at.unwrap();
    assert_eq!(jobs.reconnect_delay_ms, 60_000);
    assert!(
        jobs.reconnect_read(first - 1, Some(window_target()))
            .is_none()
    );
    let (read, kind) = jobs.reconnect_read(first, Some(window_target())).unwrap();
    assert!(kind == Kind::Quiet);
    assert!(jobs.reconnect_read(first, Some(window_target())).is_none());
    jobs.begin_job(kind, &read);
    jobs.running = false;
    jobs.apply_quiet(window_failed());
    assert_eq!(jobs.reconnect_delay_ms, 120_000);
    let second = jobs.reconnect_at.unwrap();
    let (read, kind) = jobs.reconnect_read(second, Some(window_target())).unwrap();
    jobs.begin_job(kind, &read);
    let tape = TapeWindow {
        margin_s: 600,
        long_position_min: 10,
    };
    jobs.apply_quiet(window_answer(Some(tape)));
    jobs.running = false;
    assert_eq!(jobs.window_step(tape).push, None);
    assert!(jobs.window_reconciled);
    assert_eq!(jobs.reconnect_at, None);
    assert!(
        jobs.reconnect_read(i64::MAX, Some(window_target()))
            .is_none()
    );
    jobs.clear_known(Ok("synthetic forgotten".into()));
    assert!(!jobs.window_reconciled);
    assert_eq!(jobs.window_attempt, None);
    assert_eq!(jobs.reconnect_at, None);
    assert_eq!(jobs.reconnect_delay_ms, 0);

    let mut unsupported = StationJobs {
        known: true,
        ..StationJobs::default()
    };
    for _ in 0..8 {
        unsupported.apply_quiet(window_answer(None));
    }
    assert_eq!(unsupported.reconnect_delay_ms, 0);
    assert!(unsupported.reconnect_at.is_none());
}

/// A stale successful write must not create an endless push/read cycle, while a real
/// acknowledgement updates the shown tape and unrelated work permits a new bounded cycle.
#[test]
fn shared_window_success_verifies_and_bounds_stale_report_retries() {
    use super::Kind;
    let local = TapeWindow {
        margin_s: 600,
        long_position_min: 5,
    };
    let old = TapeWindow {
        margin_s: 180,
        long_position_min: 5,
    };
    for acknowledged in [true, false] {
        let mut jobs = StationJobs {
            window_reconciled: true,
            ..StationJobs::default()
        };
        jobs.apply_quiet(window_answer(Some(old)));
        jobs.outcome = Some(Ok("synthetic user outcome".into()));
        let mut sequence = Vec::new();
        for _ in 0..4 {
            let Some(tape) = jobs.window_step(local).push else {
                break;
            };
            sequence.push("Window");
            jobs.begin_job(Kind::Window, &window_job(tape));
            jobs.apply_window(window_ok());
            assert!(jobs.pending_refresh);
            let (read, kind) = jobs.bot_read(Some(window_target())).unwrap();
            sequence.push("Quiet");
            jobs.begin_job(kind, &read);
            jobs.apply_quiet(window_answer(Some(if acknowledged { local } else { old })));
        }
        assert_eq!(
            jobs.bot.as_ref().unwrap().tape,
            Some(if acknowledged { local } else { old })
        );
        assert_eq!(jobs.outcome, Some(Ok("synthetic user outcome".into())));
        assert!(!jobs.pending_refresh);
        assert_eq!(jobs.window_step(local).push, None);
        if acknowledged {
            assert_eq!(sequence, ["Window", "Quiet"]);
        } else {
            assert_eq!(sequence, ["Window", "Quiet", "Window", "Quiet"]);
            jobs.begin_job(
                Kind::Zone,
                &job::Job::Zone {
                    target: window_target(),
                    zone: "UTC".into(),
                },
            );
            jobs.finish_job();
            assert_eq!(jobs.window_step(local).push, None);
            jobs.begin_job(
                Kind::User,
                &job::Job::BotState {
                    target: window_target(),
                },
            );
            jobs.finish_job();
            assert_eq!(jobs.window_step(local).push, Some(local));
            let mut retried = Vec::new();
            for _ in 0..4 {
                let Some(tape) = jobs.window_step(local).push else {
                    break;
                };
                retried.push("Window");
                jobs.begin_job(Kind::Window, &window_job(tape));
                jobs.apply_window(window_ok());
                let (read, kind) = jobs.bot_read(Some(window_target())).unwrap();
                retried.push("Quiet");
                jobs.begin_job(kind, &read);
                jobs.apply_quiet(window_answer(Some(old)));
            }
            assert_eq!(retried, ["Window", "Quiet", "Window", "Quiet"]);
            let lowered = TapeWindow {
                margin_s: 120,
                long_position_min: 5,
            };
            let step = jobs.window_step(lowered);
            assert_eq!(step.adopt, None);
            assert_eq!(step.push, Some(lowered));
        }
    }
}

/// Starting the original queued payload would overwrite a newer picker value when a background
/// job ends; execution-time rebuilding must preserve the queued catchup and keep edits.
#[test]
fn queued_tape_rebuild_uses_current_shared_window_without_losing_policy() {
    use super::Kind;
    let _restore = RestoreWindow::take();
    let initial = TapeWindow {
        margin_s: 180,
        long_position_min: 5,
    };
    let current = TapeWindow {
        margin_s: 600,
        long_position_min: 10,
    };
    let keep = moon_core::station_api::TapeKeep {
        max_age_days: 90,
        record_emulator: false,
        tuner_only: Some(true),
        min_tail_s: Some(120),
    };
    let catchup = moon_core::station_api::CatchupSwitches {
        auto: true,
        archives: false,
    };
    let mut jobs = StationJobs::default();
    jobs.begin_job(Kind::Window, &window_job(initial));
    jobs.waiting = Some((
        job::Job::Tape {
            target: window_target(),
            tape: initial,
            catchup: Some(catchup),
            keep: Some(keep),
        },
        false,
    ));
    trade_replay::set_margin_s(current.margin_s);
    trade_replay::set_long_position_min(current.long_position_min);
    jobs.apply_window(window_failed());
    let (queued, _) = jobs.waiting.take().unwrap();
    let started = queued.with_current_tape_window();
    jobs.begin_job(Kind::User, &started);
    let job::Job::Tape {
        tape,
        catchup: sent_catchup,
        keep: sent_keep,
        ..
    } = started
    else {
        panic!("queued tape job lost")
    };
    assert_eq!(tape, current);
    assert_eq!(sent_catchup, Some(catchup));
    assert_eq!(sent_keep, Some(keep));
}

/// A tail push sends the shared window, preserves keep choices and retains the retry latch.
#[test]
fn tape_min_tail_job_changes_only_the_supported_floor() {
    use moon_core::station_api::{TapeCatchup, TapeWindow};
    let window = TapeWindow {
        margin_s: 180,
        long_position_min: 5,
    };
    let min_tail_job = |bot: Option<&moon_remote::station::bot::BotState>, target, secs| {
        StationJobs {
            bot: bot.cloned(),
            min_tail_pending: Some(secs),
            ..StationJobs::default()
        }
        .take_min_tail_job(Some(target), window)
    };
    let target = Target {
        host: "synthetic.invalid".into(),
        port: 22,
    };
    let tape = TapeWindow {
        margin_s: 600,
        long_position_min: 10,
    };
    let mut bot = moon_remote::station::bot::BotState {
        has_token: false,
        stopped: false,
        no_api: false,
        bot: None,
        pairing_until: None,
        access: None,
        tape: Some(tape),
        station: Some(Box::new(moon_core::station_api::Status {
            station_version: "synthetic".into(),
            cores_ready: 0,
            cores_total: 0,
            bot: None,
            tape: Some(tape),
            tape_catchup: Some(Box::new(TapeCatchup {
                max_age_days: Some(90),
                record_emulator: Some(false),
                tuner_only: Some(true),
                min_tail_s: Some(60),
                ..TapeCatchup::default()
            })),
            host: None,
            last_update: None,
            auto_update: None,
            cores: None,
            core_uid_high_water: None,
        })),
    };
    let Some(job::Job::Tape {
        target: sent,
        tape: window,
        catchup,
        keep: Some(keep),
    }) = min_tail_job(Some(&bot), target.clone(), 120)
    else {
        panic!("capable station must receive the new floor");
    };
    assert_eq!(sent.addr(), target.addr());
    assert_eq!(
        window,
        TapeWindow {
            margin_s: 180,
            long_position_min: 5
        }
    );
    assert_eq!(catchup, None);
    assert_eq!(
        (
            keep.max_age_days,
            keep.record_emulator,
            keep.tuner_only,
            keep.min_tail_s
        ),
        (90, false, Some(true), Some(120))
    );
    assert!(min_tail_job(Some(&bot), target.clone(), 60).is_none());
    let mut no_tape = bot.clone();
    no_tape.tape = None;
    assert!(min_tail_job(Some(&no_tape), target.clone(), 120).is_none());
    let mut queued = StationJobs {
        running: true,
        bot: Some(bot.clone()),
        min_tail_pending: Some(120),
        ..StationJobs::default()
    };
    assert!(
        queued
            .take_min_tail_job(Some(target.clone()), window)
            .is_none()
    );
    queued.min_tail_pending = Some(180);
    assert!(
        queued
            .take_min_tail_job(Some(target.clone()), window)
            .is_none()
    );
    queued.running = false;
    let Some(job::Job::Tape {
        keep: Some(latest), ..
    }) = queued.take_min_tail_job(Some(target.clone()), window)
    else {
        panic!("idle station must take the latest queued floor");
    };
    assert_eq!(latest.min_tail_s, Some(180));
    assert_eq!(queued.min_tail_pending, Some(180));
    queued.apply_quiet(job::Done::Failed {
        reason: "synthetic timeout".into(),
        station_may_poll: false,
    });
    assert!(queued.outcome.is_none());
    queued.kind = super::Kind::Tail;
    queued.min_tail_pushed = Some(180);
    assert!(
        !queued.min_tail_retry_due(),
        "a failed write cannot re-push from its own end"
    );
    queued.kind = super::Kind::Quiet;
    assert!(
        queued.min_tail_retry_due(),
        "an unrelated read permits another attempt"
    );
    assert!(
        matches!(queued.take_min_tail_job(Some(target.clone()), window),
        Some(job::Job::Tape { keep: Some(keep), .. }) if keep.min_tail_s == Some(180))
    );
    queued
        .bot
        .as_mut()
        .unwrap()
        .station
        .as_mut()
        .unwrap()
        .tape_catchup
        .as_mut()
        .unwrap()
        .min_tail_s = Some(180);
    assert!(
        queued
            .take_min_tail_job(Some(target.clone()), window)
            .is_none()
    );
    assert_eq!(queued.min_tail_pending, None);
    bot.station
        .as_mut()
        .unwrap()
        .tape_catchup
        .as_mut()
        .unwrap()
        .min_tail_s = None;
    assert!(min_tail_job(Some(&bot), target.clone(), 120).is_none());
    assert!(min_tail_job(None, target, 120).is_none());
}

/// Retrying a failed save must map raw station UID 1 to terminal XBETA UID 2 exactly once,
/// while both the saved config and an open draft receive XALPHA UID 1 from station UID 11.
#[test]
fn issue_891_return_save_retry_keeps_raw_grants_and_updates_saved_and_draft_configs() {
    use super::{cores_sync, returned_to_terminal};
    use moon_core::config::ServerConfig;
    use moon_core::station_api::ListedCore;

    let key = "sX85BQAAAAD4HMdln7gLXlN0DqD1Qs810ml1VLTx0vkRfwzU9VrjS+XMkD1SzrhZWGd2JDVy92AArwH8gJLfmM/47yuKci+sFrrtNibJShbRnc1HGycnqLRazhICIMdoPAhGryNcv1KZClUCEhH6mRG/Np81EodJlA=="; // gitleaks:allow
    let servers = [(1, "XALPHA"), (2, "XBETA")]
        .into_iter()
        .map(|(uid, name)| {
            let mut server: ServerConfig = serde_json::from_str("{\"id\":1}").unwrap();
            server.uid = uid;
            server.name = name.into();
            server.key = Secret::new(key);
            server.endpoint_override = format!("{name}.example.invalid:4510");
            server.endpoint_to_station = true;
            server
        })
        .collect();
    let mut saved = AppConfig::headless(servers);
    let listing: Vec<_> = cores_sync::trace_cores(&saved)
        .iter()
        .zip([11, 1])
        .map(|(core, uid)| ListedCore {
            uid,
            name: core.name.clone(),
            address: core.address.clone(),
            key_fp: Some(core.key_fp.clone()),
            endpoint_override: Some(core.endpoint_override.clone()),
        })
        .collect();
    let mut access = TelegramConfig::default();
    access.pair_chat(101);
    access.pair_chat(202);
    access.chat_profile_mut(202).core_uids = vec![11, 1, 99];
    let raw = ReturnedBot {
        token: Secret::new("synthetic-returned"),
        access: Access::of(&access),
    };
    let mut draft = Some(saved.clone());
    let (translated, unmapped) = returned_to_terminal(&saved, &raw, &listing);
    assert_eq!(unmapped, 1);
    assert_eq!(translated.access.chat_access[0].core_uids, [1, 2]);
    assert!(
        save_returned_bot(&mut saved, &mut draft, &translated, |_| Err(
            anyhow::anyhow!("synthetic failure")
        ))
        .is_err()
    );
    assert!(saved.telegram.token.is_empty());
    assert!(draft.as_ref().unwrap().telegram.token.is_empty());
    assert_eq!(raw.access.chat_access[0].core_uids, [11, 1, 99]);
    let (translated, unmapped) = returned_to_terminal(&saved, &raw, &listing);
    assert_eq!(unmapped, 1);
    assert!(save_returned_bot(&mut saved, &mut draft, &translated, |_| Ok(())).unwrap());
    for config in [&saved, draft.as_ref().unwrap()] {
        assert_eq!(config.telegram.chat_access[0].core_uids, [1, 2]);
        assert_eq!(config.telegram.owner_chat_id, Some(101));
    }
}

/// Ignoring station presence would start a second bot even without a pending hand-over journal.
/// Removal permits local polling again, but cannot bypass unresolved or unreadable ownership.
#[test]
fn telegram_station_presence_blocks_local_polling_until_removed() {
    let mut jobs = StationJobs {
        known: true,
        ..StationJobs::default()
    };
    assert!(!jobs.holds_bot());
    assert!(!jobs.allows_terminal_bot());
    jobs.clear_known(Ok("synthetic removal".into()));
    assert!(jobs.allows_terminal_bot());
    jobs.pending = Some(super::recovery::Pending::new(&Target {
        host: "synthetic".into(),
        port: 22,
    }));
    assert!(!jobs.allows_terminal_bot());
    jobs.pending = None;
    jobs.journal_unreadable = true;
    assert!(!jobs.allows_terminal_bot());
}

/// Clearing only the token leaves stale ownership/grants; clearing all settings loses Mini App choice.
#[test]
fn telegram_handover_erases_bot_identity_and_preserves_mini_app_choice() {
    let mut config = TelegramConfig {
        token: Secret::new("synthetic-obsolete"),
        authorized_chat_ids: vec![42, 73],
        owner_chat_id: Some(42),
        chat_access: vec![TelegramChatAccess {
            chat_id: 73,
            name: "Synthetic viewer".into(),
            core_uids: vec![9],
        }],
        mini_app_enabled: true,
        ..TelegramConfig::default()
    };
    super::forget_bot(&mut config);
    assert!(config.token.is_empty());
    assert!(config.authorized_chat_ids.is_empty());
    assert!(config.owner_chat_id.is_none());
    assert!(config.chat_access.is_empty());
    assert!(config.mini_app_enabled);
}

/// Overwriting either a saved or unsaved local token discards the user's second bot.
#[test]
fn local_saved_or_draft_bots_block_restoration() {
    let empty = TelegramConfig::default();
    let own = TelegramConfig {
        token: Secret::new("second-synthetic-bot"),
        ..TelegramConfig::default()
    };
    assert!(can_restore_bot(&empty, None));
    assert!(can_restore_bot(&empty, Some(&empty)));
    assert!(!can_restore_bot(&own, None));
    assert!(!can_restore_bot(&own, Some(&empty)));
    assert!(!can_restore_bot(&empty, Some(&own)));
}

/// Omitting Access loses viewer grants; replacing switches loses the terminal preference.
#[test]
fn restored_configs_keep_mini_app_and_all_access() {
    let returned = ReturnedBot {
        token: Secret::new("station-synthetic-bot"),
        access: Access {
            authorized_chat_ids: vec![42, 73],
            owner_chat_id: Some(42),
            chat_access: vec![TelegramChatAccess {
                chat_id: 73,
                name: "Observer".into(),
                core_uids: vec![9, 12],
            }],
            ..Access::default()
        },
    };
    let mut saved = TelegramConfig::default();
    let mut draft = TelegramConfig {
        mini_app_enabled: true,
        ..TelegramConfig::default()
    };
    restore_bot(&mut saved, &returned);
    restore_bot(&mut draft, &returned);
    for config in [&saved, &draft] {
        assert_eq!(config.token.expose(), "station-synthetic-bot");
        assert_eq!(config.authorized_chat_ids, [42, 73]);
        assert_eq!(config.owner_chat_id, Some(42));
        assert_eq!(config.chat_access[0].chat_id, 73);
        assert_eq!(config.chat_access[0].name, "Observer");
        assert_eq!(config.chat_access[0].core_uids, [9, 12]);
    }
    assert!(!saved.mini_app_enabled);
    assert!(draft.mini_app_enabled);
}

/// Publishing before disk commit would start an unsaved bot and corrupt an open Settings draft.
#[test]
fn failed_save_leaves_saved_and_draft_bot_fields_unchanged() {
    let mut saved = AppConfig::headless(Vec::new());
    let mut draft = Some(saved.clone());
    draft.as_mut().unwrap().telegram.mini_app_enabled = true;
    let returned = ReturnedBot {
        token: Secret::new("synthetic-restored"),
        access: Access::default(),
    };
    let result = save_returned_bot(&mut saved, &mut draft, &returned, |candidate| {
        assert_eq!(candidate.telegram.token.expose(), "synthetic-restored");
        Err(anyhow::anyhow!("synthetic disk failure"))
    });
    assert!(result.is_err());
    assert!(saved.telegram.token.is_empty());
    assert!(draft.as_ref().unwrap().telegram.token.is_empty());
    assert!(draft.as_ref().unwrap().telegram.mini_app_enabled);
}

/// Skipping the draft update after saving lets a later Settings Save erase the restored bot.
#[test]
fn committed_return_updates_both_configs_after_save() {
    let mut saved = AppConfig::headless(Vec::new());
    let mut draft = Some(saved.clone());
    draft.as_mut().unwrap().telegram.mini_app_enabled = true;
    let returned = ReturnedBot {
        token: Secret::new("synthetic-restored"),
        access: Access {
            authorized_chat_ids: vec![42],
            owner_chat_id: Some(42),
            chat_access: Vec::new(),
            ..Access::default()
        },
    };
    let committed = std::cell::Cell::new(false);
    assert!(
        save_returned_bot(&mut saved, &mut draft, &returned, |candidate| {
            assert_eq!(candidate.telegram.authorized_chat_ids, [42]);
            committed.set(true);
            Ok(())
        })
        .unwrap()
    );
    assert!(committed.get());
    for config in [&saved, draft.as_ref().unwrap()] {
        assert_eq!(config.telegram.token.expose(), "synthetic-restored");
        assert_eq!(config.telegram.owner_chat_id, Some(42));
    }
    assert!(!saved.telegram.mini_app_enabled);
    assert!(draft.as_ref().unwrap().telegram.mini_app_enabled);
}

/// Rechecking only the job-start snapshot overwrites a local bot saved while SSH was running.
#[test]
fn local_bot_created_during_ssh_is_never_saved_over() {
    let mut saved = AppConfig::headless(Vec::new());
    let mut draft = Some(saved.clone());
    draft.as_mut().unwrap().telegram.token = Secret::new("new-local-bot");
    let returned = ReturnedBot {
        token: Secret::new("synthetic-station"),
        access: Access::default(),
    };
    assert!(
        !save_returned_bot(&mut saved, &mut draft, &returned, |_| panic!(
            "must not save over a local bot"
        ))
        .unwrap()
    );
    assert!(saved.telegram.token.is_empty());
    assert_eq!(
        draft.as_ref().unwrap().telegram.token.expose(),
        "new-local-bot"
    );
}

/// Reintroducing the local-only station_begin shortcut after a failed save can start a second
/// poller: server-token actions remain available between attempts. This binary-only owner cannot
/// be constructed without the production host, so check its dispatch boundary as source text.
#[test]
fn bot_off_retries_never_publish_locally_before_dispatching_server_work() {
    let source = include_str!("../station.rs");
    let begin = source.split("    fn station_begin(").nth(1).unwrap();
    let begin = begin.split("    fn station_next(").next().unwrap();
    assert!(!begin.contains("self.station_apply_returned()"));
    assert!(!begin.contains("self.telegram.resume("));
}

/// Retaining queued work, listings or confirmation state applies the old station to a new one.
#[test]
fn losing_a_known_station_cancels_queued_work_and_fingerprint_consent() {
    let mut jobs = StationJobs {
        cores_seen: Some(Some(Vec::new())),
        cores_result: Some("old result".into()),
        cores_job: true,
        pending_refresh: true,
        needs_old_admin: true,
        waiting: Some((
            job::Job::Status {
                target: Target {
                    host: "old".into(),
                    port: 22,
                },
            },
            false,
        )),
        address_change: Some(AddressChange {
            source: Host {
                addr: "old:22".into(),
                fingerprint: "SHA256:old".into(),
                admin: Some("moon".into()),
            },
            target: Target {
                host: "new".into(),
                port: 22,
            },
            fingerprint: "SHA256:new".into(),
        }),
        finished: 7,
        ..StationJobs::default()
    };
    jobs.clear_known(Ok("removed".into()));
    assert!(!jobs.pending_refresh);
    assert!(jobs.cores_seen.is_none() && jobs.cores_result.is_none());
    assert!(!jobs.cores_job);
    assert!(jobs.waiting.is_none() && jobs.address_change.is_none());
    assert!(!jobs.needs_old_admin);
    assert_eq!(
        jobs.finished, 7,
        "the UI still needs the job-end counter to observe the lost station"
    );
}

/// Regression tripwire: omitting preflight recreates secrets after a completed removal whose local forget failed.
#[test]
fn explicit_core_writes_and_removals_require_a_configured_station() {
    let jobs = include_str!("job.rs").split("fn run(").nth(1).unwrap();
    for (arm, next, operation) in [
        (
            "Job::Cores {",
            "Job::CoresRemove",
            "station::push_cores_with_add_flags(",
        ),
        (
            "        Job::CoresRemove {",
            "Job::Tape",
            "station::remove_cores(",
        ),
    ] {
        let sync = jobs.split(arm).nth(1).unwrap().split(next).next().unwrap();
        let refusal = sync.find("RemovalError::NotConfigured").unwrap();
        assert!(sync[..refusal].contains("script::value(&status, \"config\") != Some(\"yes\")"));
        assert!(refusal < sync.find(operation).unwrap());
    }
}

/// A restarting service reports no Status; discarding the listing would erase comparison state.
#[test]
fn bot_reads_without_station_status_preserve_the_last_core_listing() {
    let mut jobs = StationJobs::default();
    let mut bot = moon_remote::station::bot::BotState {
        has_token: false,
        stopped: false,
        no_api: false,
        bot: None,
        pairing_until: None,
        access: None,
        tape: None,
        station: None,
    };
    jobs.observe_bot(bot.clone());
    assert!(jobs.cores_seen.is_none());
    bot.station = Some(Box::new(moon_core::station_api::Status {
        station_version: "synthetic".into(),
        core_uid_high_water: Some(7),
        cores_total: 0,
        cores_ready: 0,
        cores: Some(Vec::new()),
        bot: None,
        tape: None,
        host: None,
        tape_catchup: None,
        last_update: None,
        auto_update: None,
    }));
    jobs.observe_bot(bot.clone());
    assert_eq!(jobs.cores_seen, Some(Some(Vec::new())));
    assert_eq!(jobs.core_uid_high_water, Some(7));
    bot.station = None;
    jobs.observe_bot(bot.clone());
    assert_eq!(jobs.cores_seen, Some(Some(Vec::new())));
    assert_eq!(jobs.core_uid_high_water, Some(7));
    bot.station = Some(Box::new(moon_core::station_api::Status {
        station_version: "old".into(),
        core_uid_high_water: None,
        cores_total: 0,
        cores_ready: 0,
        cores: None,
        bot: None,
        tape: None,
        host: None,
        tape_catchup: None,
        last_update: None,
        auto_update: None,
    }));
    jobs.observe_bot(bot);
    assert_eq!(jobs.cores_seen, Some(None));
    assert_eq!(jobs.core_uid_high_water, None);
}

/// Dropping either ownership marker permits Forget/Remove to discard the only recovery route.
#[test]
fn pending_or_unreadable_handover_blocks_access_changes() {
    let clear = StationJobs::default();
    assert!(clear.access_refusal().is_none());
    let pending = StationJobs {
        pending: Some(super::recovery::Pending::new(&Target {
            host: "original".into(),
            port: 22,
        })),
        ..StationJobs::default()
    };
    assert!(pending.access_refusal().is_some());
    let unreadable = StationJobs {
        journal_unreadable: true,
        ..StationJobs::default()
    };
    assert!(unreadable.access_refusal().is_some());
}

/// Ignoring the retained snapshot redirects a failed bot-return retry to a different server.
#[test]
fn returned_bot_blocks_access_changes_until_recovered() {
    let mut jobs = StationJobs {
        returned: Some(ReturnedBot {
            token: Secret::new("synthetic-retained"),
            access: Access::default(),
        }),
        ..StationJobs::default()
    };
    assert!(jobs.access_refusal().is_some());
    jobs.returned = None;
    assert!(jobs.access_refusal().is_none());
}

/// Removing the execution-time guard lets a queued removal bypass a newly retained snapshot;
/// omitting the Forget guard discards access even when its inline confirmation became stale.
#[test]
fn queued_access_changes_and_forgetting_recheck_recovery_state() {
    let jobs = include_str!("../station.rs");
    let begin = jobs
        .split("    fn station_begin(")
        .nth(1)
        .unwrap()
        .split("    fn station_next(")
        .next()
        .unwrap();
    assert!(begin.find("job.changes_access()").unwrap() < begin.find("job::start(job,").unwrap());
    assert!(begin.contains("self.station.access_refusal()"));
    let view = include_str!("../../settings/telegram/server_bot.rs");
    let forget = view
        .split("    pub(super) fn server_bot_forget(")
        .nth(1)
        .unwrap()
        .split("    ///")
        .next()
        .unwrap();
    assert!(
        forget.find("station.access_refusal()").unwrap() < forget.find("hosts.forget(").unwrap()
    );
    let host = Host {
        addr: "old:22".into(),
        fingerprint: "SHA256:old".into(),
        admin: Some("moon".into()),
    };
    assert!(
        job::Job::Remove {
            source: host.clone()
        }
        .changes_access()
    );
    assert!(
        job::Job::AddressProbe {
            source: host,
            target: Target {
                host: "new".into(),
                port: 22
            }
        }
        .changes_access()
    );
    assert!(
        !job::Job::BotOff {
            target: Target {
                host: "old".into(),
                port: 22
            },
            restore: true,
            recovered: None
        }
        .changes_access()
    );
}

/// The zone is pushed while a station that knows the bot's settings reports another zone; never
/// to one that predates them or has not been read.
#[test]
fn the_zone_is_pushed_to_a_station_that_takes_it() {
    let current = Access {
        bot: Some(Default::default()),
        zone: Some("UTC".into()),
        ..Access::default()
    };
    assert!(super::zone_push_due(Some(&current), "Europe/Moscow"));
    assert!(!super::zone_push_due(Some(&current), "UTC"));
    let old = Access {
        bot: None,
        ..current.clone()
    };
    assert!(!super::zone_push_due(Some(&old), "Europe/Moscow"));
    assert!(!super::zone_push_due(None, "Europe/Moscow"));
}

/// A synthetic station's answer to a bot read: a running bot with two paired chats, their
/// notification rows, the bot's menu and layout settings, one listed core and a tape window.
fn synthetic_station_answer() -> job::Done {
    let access = Access {
        authorized_chat_ids: vec![42, 73],
        owner_chat_id: Some(42),
        bot: Some(Default::default()),
        notify: Some(
            [(42, Default::default()), (73, Default::default())]
                .into_iter()
                .collect(),
        ),
        ..Access::default()
    };
    let bot = moon_remote::station::bot::BotState {
        has_token: true,
        stopped: false,
        no_api: false,
        bot: None,
        pairing_until: None,
        access: Some(access),
        tape: Some(moon_core::station_api::TapeWindow {
            margin_s: 30,
            long_position_min: 60,
        }),
        station: Some(Box::new(moon_core::station_api::Status {
            station_version: "synthetic".into(),
            core_uid_high_water: Some(1),
            cores_total: 1,
            cores_ready: 1,
            cores: Some(vec![moon_core::station_api::ListedCore {
                uid: 1,
                name: "Synthetic core".into(),
                address: None,
                key_fp: None,
                endpoint_override: None,
            }]),
            bot: None,
            tape: None,
            host: None,
            tape_catchup: None,
            last_update: None,
            auto_update: None,
        })),
    };
    job::Done::Ok {
        transferred: false,
        bot: Some(bot),
        bot_off: false,
    }
}

/// Dropping the startup read (or gating it on an opened page) brings back Settings opening on
/// «state not received yet» / «0 chats» / «reading station cores»; repeating it per tick polls
/// the station; firing it without a station sends SSH work nowhere.
#[test]
fn startup_prefetch_fills_station_pages_once_and_never_without_a_station() {
    let station = Target {
        host: "synthetic".into(),
        port: 22,
    };
    let mut sent = Vec::new();
    let mut jobs = StationJobs::default();
    for _ in 0..3 {
        if let Some((read, kind)) = jobs.prefetch_read(Some(station.clone())) {
            sent.push((read, kind));
        }
    }
    assert_eq!(sent.len(), 1, "the startup read is attempted exactly once");
    let (read, kind) = sent.pop().unwrap();
    assert!(kind == super::Kind::Quiet);
    assert!(matches!(read, job::Job::BotState { target } if target.addr() == station.addr()));

    // The fake station answers; nothing in Settings was opened.
    jobs.apply_quiet(synthetic_station_answer());
    let bot = jobs.bot.as_ref().expect("bot status populated");
    assert!(bot.has_token);
    let access = bot.access.as_ref().expect("chats populated");
    assert_eq!(access.authorized_chat_ids, vec![42, 73]);
    assert_eq!(access.notify.as_ref().map(|n| n.len()), Some(2));
    assert!(access.bot.is_some(), "menu and message layout populated");
    assert!(bot.tape.is_some(), "tape window populated");
    assert_eq!(
        jobs.cores_seen.as_ref().map(|c| c.as_ref().map(Vec::len)),
        Some(Some(1))
    );
    assert!(jobs.bot_error.is_none());

    let mut unconfigured = StationJobs::default();
    assert!(unconfigured.prefetch_read(None).is_none());
    assert!(
        unconfigured.prefetch_read(Some(station)).is_none(),
        "latched after the first try"
    );
}

/// A failed startup read records its reason for the page, and never touches the user's outcome.
#[test]
fn a_failed_startup_read_stays_quiet() {
    let mut jobs = StationJobs::default();
    jobs.apply_quiet(job::Done::Failed {
        reason: "synthetic timeout".into(),
        station_may_poll: false,
    });
    assert_eq!(jobs.bot_error.as_deref(), Some("synthetic timeout"));
    assert!(jobs.outcome.is_none());
    assert!(jobs.lines.is_empty());
}
