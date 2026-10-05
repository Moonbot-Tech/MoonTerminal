//! Bot ownership, Settings draft preservation and station access lifecycle decisions.
use super::{StationJobs, can_restore_bot, job, restore_bot, save_returned_bot};
use moon_core::config::AppConfig;
use moon_core::config::telegram_access::TelegramChatAccess;
use moon_core::config::{Secret, TelegramConfig};
use moon_core::station_api::Access;
use moon_remote::hosts::Host;
use moon_remote::ssh::Target;
use moon_remote::station::access::AddressChange;
use moon_remote::station::bot::ReturnedBot;

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
        cores_total: 0,
        cores_ready: 0,
        cores: Some(Vec::new()),
        bot: None,
        tape: None,
        host: None,
        last_update: None,
        auto_update: None,
    }));
    jobs.observe_bot(bot.clone());
    assert_eq!(jobs.cores_seen, Some(Some(Vec::new())));
    bot.station = None;
    jobs.observe_bot(bot.clone());
    assert_eq!(jobs.cores_seen, Some(Some(Vec::new())));
    bot.station = Some(Box::new(moon_core::station_api::Status {
        station_version: "old".into(),
        cores_total: 0,
        cores_ready: 0,
        cores: None,
        bot: None,
        tape: None,
        host: None,
        last_update: None,
        auto_update: None,
    }));
    jobs.observe_bot(bot);
    assert_eq!(jobs.cores_seen, Some(None));
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
    assert!(begin.find("job.changes_access()").unwrap() < begin.find("job::start(job)").unwrap());
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
