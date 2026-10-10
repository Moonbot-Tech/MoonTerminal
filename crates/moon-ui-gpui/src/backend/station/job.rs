//! The station's server work, off the UI thread: every step is a blocking SSH exchange of seconds to
//! minutes. One job at a time; its lines and its end come back over a channel the backend drains.

use std::sync::mpsc;

use moon_core::config::Secret;
use moon_core::station_api::Access;
use moon_remote::error::StationError;
use moon_remote::progress::{Progress, Step};
use moon_remote::setup::{self, FirstAccess, NeedsAdminPassword, Setup};
use moon_remote::ssh::Target;
use moon_remote::station::bot::{self, BotState};
use moon_remote::station::{self, BotChange, CoreKey, TapeWindow};
use rust_i18n::t;
use zeroize::Zeroizing;

use super::cores_sync::{self, Upsert};

/// A stale core selection requires the user to read the current listing again.
#[derive(Debug)]
pub(crate) struct CoresChanged;
impl std::fmt::Display for CoresChanged {
    /// Preserve a secret-free diagnostic while the UI supplies the localized headline.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the station changed, refresh")
    }
}
impl std::error::Error for CoresChanged {}

/// Intersect persisted keys with the UI eligibility snapshot, which excludes synthetic cores.
fn saved_local_cores(
    all: &[moon_core::config::CoreKeyEntry],
    eligible: &[u64],
) -> Vec<cores_sync::LocalCore> {
    all.iter()
        .filter(|core| {
            eligible.contains(&core.uid) && cores_sync::eligible(core.active, false, &core.key)
        })
        .map(|core| {
            cores_sync::local_core(core.uid, &core.name, &core.key, &core.endpoint_override)
        })
        .collect()
}

/// The bot the station should run after a job, if any.
pub(crate) enum BotPlan {
    /// Leave the station's bot as it is.
    Keep,
    /// Hand the terminal's bot over: its token and its paired chats.
    Transfer {
        token: Secret,
        /// Boxed: the bot's settings make it the largest part of a plan.
        pairing: Box<Access>,
        change: BotChange,
    },
}

/// The control that starts a core push. One upsert cannot say this: a single new
/// core is either the row's "Add" or the bulk "Send changes (1)", and a single
/// update is either "Send name", "Send key", or that same bulk button.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CoreStart {
    /// The row button that adds one core missing on the station.
    Add,
    /// The row button that sends one differing name.
    Name,
    /// The row button that sends one differing key.
    Key,
    /// The bulk button, including when it sends a single core.
    Bulk,
}

/// One unit of work on the server.
pub(crate) enum Job {
    /// Read the destination fingerprint without authenticating or changing the saved host.
    AddressProbe {
        source: moon_remote::hosts::Host,
        target: Target,
    },
    /// Verify a user-confirmed fingerprint and move the administrator record.
    AddressChange {
        change: station::access::AddressChange,
    },
    /// Read a new server's host key before any credential goes to it, for the user to confirm.
    InstallProbe { target: Target },
    /// Remove station credentials before forgetting the known host.
    Remove { source: moon_remote::hosts::Host },
    /// Prepare a new server, install the station, send the cores, set the bot.
    Install {
        setup: Setup,
        cores: Vec<u64>,
        bot: BotPlan,
    },
    /// Run the setup again on a server set up before: updates the helper and the unit, and moves a
    /// server from before key-only sudo over (with its old administrator password).
    Resetup { setup: Setup },
    /// Update the station from the latest release (health-checked, rolled back when it does not
    /// stay up).
    Update { target: Target },
    /// Add or update selected address-matched cores, preserving station-only entries.
    Cores {
        target: Target,
        upsert: Vec<Upsert>,
        eligible: Vec<u64>,
        /// The button the user pressed. The upsert list does not name it.
        started: CoreStart,
    },
    /// Explicitly remove selected station identities, preserving their report history.
    CoresRemove {
        target: Target,
        uids: Vec<u64>,
        names: Vec<String>,
        addresses: Vec<Option<String>>,
        eligible: Vec<u64>,
    },
    /// Set the station's window around a trade.
    Tape { target: Target, tape: TapeWindow },
    /// Switch the station's own updates from the release: `[update] auto`, then a reload.
    AutoUpdate { target: Target, on: bool },
    /// Set the station's bot.
    Bot { target: Target, bot: BotPlan },
    /// Remove the station bot, recovering it only when the terminal has none.
    BotOff {
        target: Target,
        restore: bool,
        recovered: Option<bot::ReturnedBot>,
    },
    /// Give the station's bot this token: a new bot, or a replacement that pairs anew.
    ServerToken {
        target: Target,
        token: Secret,
        change: BotChange,
    },
    /// Read bot state and station core identities quietly: no progress lines or user outcome.
    BotState { target: Target },
    /// A pairing code from the station's bot, for one more chat.
    PairIssue { target: Target },
    /// Replace the station's paired chats, only while they are still `base` (as read).
    Access {
        target: Target,
        base: Access,
        access: Access,
        /// The user's edits of the chats or the bot's menu, applied by "Apply on the server" —
        /// not a reset of the pairing: its end is shown beside those buttons.
        edits: bool,
    },
    /// Switch the station's Mini App on or off: `[telegram] mini_app`, then a restart — the
    /// station picks its profile at start.
    MiniApp { target: Target, on: bool },
    /// Make the terminal's header-clock zone the one the station's reports are cut in, live.
    Zone { target: Target, zone: String },
    /// Send the terminal's saved core groups to the station, for the bot's report by cores —
    /// only on the user's word: another terminal's groups are not overwritten by one that has
    /// none.
    Groups {
        target: Target,
        groups: Vec<moon_core::config::CoreGroup>,
    },
    /// Read the station's state.
    Status { target: Target },
    /// Read the tail of the station's journal.
    Logs { target: Target },
}

impl Job {
    /// These actions change the destination used to recover bot ownership or credentials.
    pub(super) fn changes_access(&self) -> bool {
        matches!(
            self,
            Self::Remove { .. } | Self::AddressProbe { .. } | Self::AddressChange { .. }
        )
    }

    /// The destination of an actual hand-over; a caller flag alone cannot create ownership.
    pub(super) fn handover_target(&self) -> Option<&Target> {
        match self {
            Self::Install {
                setup,
                bot: BotPlan::Transfer { .. },
                ..
            } => Some(&setup.target),
            Self::Bot {
                target,
                bot: BotPlan::Transfer { .. },
            } => Some(target),
            _ => None,
        }
    }
}

/// The control that starts a job, so a busy line can name that job instead of "working".
///
/// A zone push has no button and does not hold the buttons. Every other variant maps to the
/// locale key of the control that starts it. Where one variant has several controls, the payload
/// picks the one the user pressed: one added core, a chat-notification save, a token replacement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum JobButton {
    /// A locale key with no placeholders.
    Key(&'static str),
    /// `telegram.server.cores_send_all`, whose label carries how many cores are sent.
    CoresAll(usize),
    /// Not a button. The running line does not show it.
    Background,
}

impl JobButton {
    /// The control that starts `job`.
    ///
    /// Args:
    /// * `job`: The station job about to run.
    /// * `has_token`: Whether the station already holds a bot token, which chooses
    ///   "Replace" over "Set" for [`Job::ServerToken`].
    ///
    /// Returns:
    /// The button to name in the busy line.
    pub(crate) fn of(job: &Job, has_token: bool) -> Self {
        match job {
            Job::AddressProbe { .. } => Self::Key("telegram.server.address_probe"),
            Job::AddressChange { .. } => Self::Key("telegram.server.address_confirm"),
            Job::InstallProbe { .. } => Self::Key("telegram.server.install"),
            // The shared "Confirm" button also forgets a server locally. Name the action.
            Job::Remove { .. } => Self::Key("telegram.server.remove"),
            Job::Install { .. } => Self::Key("telegram.server.install_confirm"),
            Job::Resetup { .. } => Self::Key("telegram.server.resetup"),
            Job::Update { .. } => Self::Key("telegram.server.update"),
            Job::Cores {
                upsert, started, ..
            } => match started {
                CoreStart::Add => Self::Key("telegram.server.cores_add"),
                CoreStart::Name => Self::Key("telegram.server.cores_name"),
                CoreStart::Key => Self::Key("telegram.server.cores_key"),
                CoreStart::Bulk => Self::CoresAll(upsert.len()),
            },
            // The second press, "Yes, remove from the station", is what starts the job.
            Job::CoresRemove { .. } => Self::Key("telegram.server.cores_remove_confirm"),
            Job::Tape { .. } => Self::Key("telegram.server.tape_set"),
            Job::AutoUpdate { .. } => Self::Key("telegram.server.auto_update"),
            Job::Bot { .. } => Self::Key("telegram.server.move_bot"),
            Job::BotOff { .. } => Self::Key("telegram.server.bot_off"),
            Job::ServerToken { .. } if has_token => Self::Key("telegram.server.token_replace"),
            Job::ServerToken { .. } => Self::Key("telegram.server.token_set"),
            Job::BotState { .. } => Self::Key("telegram.server.refresh"),
            Job::PairIssue { .. } => Self::Key("telegram.pair_new"),
            Job::Access { edits: true, .. } => Self::Key("telegram.server.access_apply"),
            Job::Access { access, .. } if access.notify.is_some() => {
                Self::Key("telegram.notify_editor.save")
            }
            Job::Access { .. } => Self::Key("telegram.pair_reset"),
            Job::MiniApp { .. } => Self::Key("telegram.server.mini_app"),
            Job::Zone { .. } => Self::Background,
            Job::Groups { .. } => Self::Key("telegram.server.groups_send"),
            Job::Status { .. } => Self::Key("telegram.server.status"),
            Job::Logs { .. } => Self::Key("telegram.server.logs"),
        }
    }

    /// The button's label in the active locale.
    ///
    /// Returns:
    /// Translated control text. Empty for [`JobButton::Background`], which the running line skips.
    pub(crate) fn text(self) -> String {
        match self {
            Self::Key(key) => t!(key).to_string(),
            Self::CoresAll(n) => t!("telegram.server.cores_send_all", n = n).to_string(),
            Self::Background => String::new(),
        }
    }
}

/// What a finished job reports.
pub(crate) enum Done {
    /// A completed installation retains skipped core names through the shared outcome channel.
    Installed {
        transferred: bool,
        bot: Option<BotState>,
        bot_off: bool,
        configured: usize,
        skipped: Vec<String>,
    },
    /// The station stopped polling and removed its bot; recovered data may be saved locally.
    BotOff { returned: Option<bot::ReturnedBot> },
    /// The destination fingerprint awaits an explicit confirmation in the Station tab.
    AddressProbed(station::access::AddressChange),
    /// A new server's host key awaits an explicit confirmation before the install.
    InstallProbed(setup::HostKey),
    /// The destination was verified and saved; cached station state must be discarded.
    AddressChanged,
    /// All remote secrets were removed. Even if forgetting locally failed, queued core writes
    /// must be cancelled so they cannot recreate credentials on the removed station.
    Removed { local_forget_error: Option<String> },
    Ok {
        /// The terminal's bot now runs on the station: its token goes from the terminal.
        transferred: bool,
        bot: Option<BotState>,
        /// The bot was taken off the station: a terminal bot held down may run again.
        bot_off: bool,
    },
    /// A server set up before key-only sudo: its old administrator password is needed once.
    NeedsAdminPassword,
    Failed {
        reason: String,
        /// The station may still poll the bot token: the terminal's bot stays down.
        station_may_poll: bool,
    },
}

/// Progress, structured readings and completion sent from the worker to Settings.
pub(crate) enum Event {
    /// Quarantined before removal; never starts a local poller until the successful end.
    Returned(bot::ReturnedBot),
    Line(String),
    /// A core operation's Text result, independent of diagnostic lines.
    CoresResult(String),
    /// Only an explicit Status action publishes these facts, never a quiet bot refresh.
    Status(moon_tg::StatusFacts),
    Done(Done),
}

/// Start `job` on its own thread.
pub(crate) fn start(job: Job) -> mpsc::Receiver<Event> {
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("station-setup".into())
        .spawn(move || {
            let status_requested = matches!(job, Job::Status { .. });
            let cores_job = matches!(job, Job::Cores { .. } | Job::CoresRemove { .. });
            let lines = tx.clone();
            let mut say = move |event: Progress| {
                if cores_job && let Progress::Text(line) = &event {
                    let _ = lines.send(Event::CoresResult(line.clone()));
                }
                if let Some(line) = super::text::progress(event) {
                    let _ = lines.send(Event::Line(line));
                }
            };
            let mut remember = |returned| {
                let _ = tx.send(Event::Returned(returned));
            };
            let done = match run(job, &mut remember, &mut say) {
                Ok(done) => done,
                Err(e) if e.downcast_ref::<NeedsAdminPassword>().is_some() => {
                    Done::NeedsAdminPassword
                }
                Err(e) => Done::Failed {
                    station_may_poll: e
                        .downcast_ref::<moon_remote::station::bot::StationMayStillPoll>()
                        .is_some(),
                    reason: super::text::error(&e),
                },
            };
            if status_requested
                && let Done::Ok {
                    bot: Some(state), ..
                } = &done
                && let Some(status) = &state.station
            {
                let _ = tx.send(Event::Status(moon_tg::StatusFacts::of(status)));
            }
            let _ = tx.send(Event::Done(done));
        })
        .map(|_| ())
        .unwrap_or_else(|e| log::warn!("station: no thread for the job ({e})"));
    rx
}

/// Execute one queued station job; return bot data only after server polling has stopped.
fn run(
    job: Job,
    remember: &mut dyn FnMut(bot::ReturnedBot),
    say: &mut dyn FnMut(Progress),
) -> anyhow::Result<Done> {
    match job {
        Job::AddressProbe { source, target } => {
            say(Progress::step(
                Step::AddressProbe,
                "probe destination host key",
            ));
            Ok(Done::AddressProbed(station::access::probe_address(
                source, target,
            )?))
        }
        Job::AddressChange { change } => {
            say(Progress::step(
                Step::AddressVerify,
                "verify confirmed destination",
            ));
            station::access::change_address(&change)?;
            Ok(Done::AddressChanged)
        }
        Job::InstallProbe { target } => {
            say(Progress::step(
                Step::InstallProbe,
                "probe new server host key",
            ));
            Ok(Done::InstallProbed(setup::probe_host_key(target)?))
        }
        Job::Remove { source } => {
            say(Progress::step(
                Step::StationRemove,
                "remove station credentials",
            ));
            match station::access::remove_station(&source) {
                Ok(()) => Ok(Done::Removed {
                    local_forget_error: None,
                }),
                Err(e)
                    if matches!(
                        e.downcast_ref::<station::access::RemovalError>(),
                        Some(station::access::RemovalError::LocalForgetFailed)
                    ) =>
                {
                    Ok(Done::Removed {
                        local_forget_error: Some(super::text::error(&e)),
                    })
                }
                Err(e) => Err(e),
            }
        }
        Job::Install { setup, cores, bot } => {
            let target = setup.target.clone();
            // Read before the server changes: a terminal without a core key has nothing to
            // install a station for.
            let (keys, skipped) = core_keys(&cores)?;
            setup::run(&setup, say)?;
            // Before the first start: the station opens the cache as its own.
            send_valuation(&target, say);
            let status = bot::bot_state(&target).ok().and_then(|state| state.station);
            let listing = status.as_ref().and_then(|status| status.cores.as_deref());
            let high_water = status
                .as_ref()
                .and_then(|status| status.core_uid_high_water);
            let changes = install_changes(&keys, listing, high_water)?;
            if !changes.is_empty() {
                let keys: Vec<_> = changes
                    .iter()
                    .map(|change| {
                        let key = keys
                            .iter()
                            .find(|key| key.uid == change.terminal_uid)
                            .ok_or(CoresChanged)?;
                        Ok(CoreKey {
                            uid: change.station_uid,
                            name: key.name.clone(),
                            transport: key.transport,
                            key: key.key.clone(),
                            endpoint_override: key.endpoint_override.clone(),
                        })
                    })
                    .collect::<anyhow::Result<_>>()?;
                let adds: Vec<_> = changes.iter().map(|change| change.add).collect();
                // The terminal's window only starts a station that has none.
                station::push_cores_with_add_flags(
                    &target,
                    &keys,
                    &adds,
                    station::terminal_tape(),
                    say,
                )?;
            }
            let (transferred, bot) = set_bot(&target, bot, say)?;
            Ok(Done::Installed {
                transferred,
                bot,
                bot_off: false,
                configured: keys.len(),
                skipped,
            })
        }
        Job::Resetup { setup } => {
            setup::run(&setup, say)?;
            Ok(Done::Ok {
                transferred: false,
                bot: None,
                bot_off: false,
            })
        }
        Job::Update { target } => {
            station::update_from_release(&target, say)?;
            Ok(Done::Ok {
                transferred: false,
                bot: None,
                bot_off: false,
            })
        }
        Job::Cores {
            target,
            upsert,
            eligible,
            ..
        } => {
            // Do not reinstall credentials after a remote removal or failed local forget.
            let conn = station::admin_conn(&target)?;
            let status = moon_remote::script::checked(conn.run(
                &moon_remote::script::helper("status", &[]),
                &[],
                moon_remote::script::STEP_TIMEOUT,
            )?)?
            .stdout_text();
            if moon_remote::script::value(&status, "config") != Some("yes") {
                return Err(station::access::RemovalError::NotConfigured.into());
            }
            let status = bot::bot_state(&target)?.station.ok_or(CoresChanged)?;
            let high_water = status.core_uid_high_water;
            let listing = status.cores.ok_or(CoresChanged)?;
            let all = moon_core::config::read_core_keys()?;
            let here = saved_local_cores(&all, &eligible);
            let fresh = cores_sync::reconcile(&here, &listing, high_water);
            let selected = cores_sync::selected_changes(&upsert, &fresh).ok_or(CoresChanged)?;
            if !selected.is_empty() {
                let keys = upsert_keys(&all, &selected)?;
                let adds: Vec<_> = selected.iter().map(|change| change.add).collect();
                station::push_cores_with_add_flags(&target, &keys, &adds, None, say)?;
            }
            let bot = bot::wait_status(&target, say)?;
            let single = (upsert.len() == 1 && selected.len() == 1)
                .then(|| {
                    fresh
                        .iter()
                        .find(|row| row.terminal_uid == Some(selected[0].terminal_uid))
                })
                .flatten();
            let result = match single {
                Some(row) => match row.state {
                    cores_sync::RowState::OnlyHere => {
                        rust_i18n::t!("telegram.server.cores_added", name = &row.name).to_string()
                    }
                    cores_sync::RowState::NameDiffers => rust_i18n::t!(
                        "telegram.server.cores_renamed",
                        name = &row.name,
                        was = row.station_name.as_deref().unwrap_or_default()
                    )
                    .to_string(),
                    cores_sync::RowState::KeyDiffers => {
                        rust_i18n::t!("telegram.server.cores_key_sent", name = &row.name)
                            .to_string()
                    }
                    _ => rust_i18n::t!("telegram.server.cores_pushed", n = selected.len())
                        .to_string(),
                },
                None => {
                    rust_i18n::t!("telegram.server.cores_pushed", n = selected.len()).to_string()
                }
            };
            say(Progress::Text(result));
            Ok(Done::Ok {
                transferred: false,
                bot: Some(bot),
                bot_off: false,
            })
        }
        Job::CoresRemove {
            target,
            uids,
            names,
            addresses,
            eligible,
        } => {
            let conn = station::admin_conn(&target)?;
            let status = moon_remote::script::checked(conn.run(
                &moon_remote::script::helper("status", &[]),
                &[],
                moon_remote::script::STEP_TIMEOUT,
            )?)?
            .stdout_text();
            if moon_remote::script::value(&status, "config") != Some("yes") {
                return Err(station::access::RemovalError::NotConfigured.into());
            }
            let status = bot::bot_state(&target)?.station.ok_or(CoresChanged)?;
            let high_water = status.core_uid_high_water;
            let listing = status.cores.ok_or(CoresChanged)?;
            let all = moon_core::config::read_core_keys()?;
            let fresh =
                cores_sync::reconcile(&saved_local_cores(&all, &eligible), &listing, high_water);
            anyhow::ensure!(
                cores_sync::removal_matches(&uids, &names, &addresses, &fresh),
                CoresChanged
            );
            station::remove_cores(&target, &uids, say)?;
            let bot = bot::wait_status(&target, say)?;
            say(Progress::Text(
                rust_i18n::t!(
                    "telegram.server.cores_removed",
                    name = names.join(", "),
                    n = uids.len()
                )
                .to_string(),
            ));
            Ok(Done::Ok {
                transferred: false,
                bot: Some(bot),
                bot_off: false,
            })
        }
        Job::Tape { target, tape } => {
            station::push_tape(&target, tape, say)?;
            Ok(Done::Ok {
                transferred: false,
                bot: None,
                bot_off: false,
            })
        }
        Job::AutoUpdate { target, on } => {
            station::push_auto_update(&target, on, say)?;
            Ok(Done::Ok {
                transferred: false,
                bot: None,
                bot_off: false,
            })
        }
        Job::Bot { target, bot } => {
            let (transferred, bot) = set_bot(&target, bot, say)?;
            Ok(Done::Ok {
                transferred,
                bot,
                bot_off: false,
            })
        }
        Job::BotOff {
            target,
            restore,
            recovered,
        } => Ok(Done::BotOff {
            returned: bot::return_bot(&target, restore, recovered, remember, say)?,
        }),
        Job::ServerToken {
            target,
            token,
            change,
        } => Ok(Done::Ok {
            transferred: false,
            bot: Some(bot::set_token(&target, &token, &change, say)?),
            bot_off: false,
        }),
        Job::BotState { target } => Ok(Done::Ok {
            transferred: false,
            bot: Some(bot::bot_state(&target)?),
            bot_off: false,
        }),
        Job::PairIssue { target } => {
            let code = station::api::issue_pairing(&target)?;
            say(Progress::Text(
                rust_i18n::t!("station.progress.pair_issued", code = code.code).to_string(),
            ));
            Ok(Done::Ok {
                transferred: false,
                bot: Some(bot::bot_state(&target)?),
                bot_off: false,
            })
        }
        Job::Access {
            target,
            base,
            access,
            ..
        } => {
            let saved = station::api::set_access(&target, &base, &access)?;
            say(Progress::Text(
                rust_i18n::t!(
                    "station.progress.chats_saved",
                    count = saved.authorized_chat_ids.len()
                )
                .to_string(),
            ));
            // Changed permissions restart the bot's transport: its state once it polls again. The
            // change is saved either way, so a bot slow to come back — or a read of it that fails
            // too — is a line, not a failure; the read after the job brings the new state.
            let state = settled(&target, bot::wait_bot(&target, say), say);
            if let Err(error) = &state {
                say(Progress::Text(super::text::error(error)));
            }
            Ok(Done::Ok {
                transferred: false,
                bot: state.ok(),
                bot_off: false,
            })
        }
        Job::Zone { target, zone } => {
            let saved = station::api::set_zone(&target, &zone)?;
            say(Progress::Text(match saved {
                Some(_) => rust_i18n::t!("telegram.server.zone_pushed", zone = zone).to_string(),
                None => rust_i18n::t!("telegram.server.zone_unsupported").to_string(),
            }));
            // The push has landed: a read that fails after it does not make it a failure.
            Ok(Done::Ok {
                transferred: false,
                bot: bot::bot_state(&target).ok(),
                bot_off: false,
            })
        }
        Job::Groups { target, groups } => {
            let saved = station::api::set_groups(&target, &groups)?;
            say(Progress::Text(match saved {
                Some(_) => {
                    rust_i18n::t!("telegram.server.groups_pushed", n = groups.len()).to_string()
                }
                None => rust_i18n::t!("telegram.server.groups_unsupported").to_string(),
            }));
            // The push has landed: a read that fails after it does not make it a failure.
            Ok(Done::Ok {
                transferred: false,
                bot: bot::bot_state(&target).ok(),
                bot_off: false,
            })
        }
        Job::MiniApp { target, on } => {
            let change = BotChange {
                mini_app: Some(on),
                ..BotChange::default()
            };
            station::push_telegram(&target, None, &change, false, say)?;
            Ok(Done::Ok {
                transferred: false,
                bot: Some(settled(&target, bot::wait_mini_app(&target, on, say), say)?),
                bot_off: false,
            })
        }
        Job::Status { target } => {
            let conn = station::admin_conn(&target)?;
            let out = moon_remote::script::checked(conn.run(
                &moon_remote::script::helper("status", &[]),
                &[],
                moon_remote::script::STEP_TIMEOUT,
            )?)?;
            for line in out.stdout_text().lines() {
                say(Progress::Diagnostic(line.to_owned()));
            }
            // The station's own figures, in the words meant for the bot's chat "Status" too.
            let state = bot::bot_state(&target)?;
            show_status(&state, say);
            Ok(Done::Ok {
                transferred: false,
                bot: Some(state),
                bot_off: false,
            })
        }
        Job::Logs { target } => {
            let conn = station::admin_conn(&target)?;
            let out = moon_remote::script::checked(conn.run(
                &moon_remote::script::helper("logs", &["200"]),
                &[],
                moon_remote::script::STEP_TIMEOUT,
            )?)?;
            show_journal(&out.stdout_text(), say);
            Ok(Done::Ok {
                transferred: false,
                bot: None,
                bot_off: false,
            })
        }
    }
}

/// Keep compatibility guidance as progress; available readings travel as typed facts instead.
fn show_status(state: &BotState, say: &mut dyn FnMut(Progress)) {
    if let Some(station) = &state.station {
        if let Some(line) = older_service(&station.station_version) {
            say(Progress::Text(line));
        }
    } else {
        let text = if state.stopped {
            rust_i18n::t!("telegram.server.bot_stopped")
        } else {
            rust_i18n::t!("telegram.server.bot_no_api")
        };
        say(Progress::Text(text.to_string()));
    }
}

/// Show the requested journal beneath one localized heading, preserving each raw line verbatim.
/// Journal entries are visible content, unlike hidden helper status/progress diagnostics.
fn show_journal(text: &str, say: &mut dyn FnMut(Progress)) {
    say(Progress::Text(
        rust_i18n::t!("station.progress.logs").to_string(),
    ));
    for line in text.lines() {
        say(Progress::Text(line.to_owned()));
    }
}

/// Apply a bot plan, returning ownership and observed state for either install or bot completion.
fn set_bot(
    target: &Target,
    plan: BotPlan,
    say: &mut dyn FnMut(Progress),
) -> anyhow::Result<(bool, Option<BotState>)> {
    match plan {
        BotPlan::Keep => Ok((false, None)),
        BotPlan::Transfer {
            token,
            pairing,
            change,
        } => {
            let state = bot::transfer_bot(target, &token, &pairing, &change, say)?;
            Ok((true, Some(state)))
        }
    }
}

/// The bot's state after a change the station has already taken: what the wait saw, or — when it
/// did not see the bot settle — why, as a line, and the state as it is now.
fn settled(
    target: &Target,
    waited: anyhow::Result<BotState>,
    say: &mut dyn FnMut(Progress),
) -> anyhow::Result<BotState> {
    waited.or_else(|e| {
        say(Progress::Text(
            rust_i18n::t!(
                "station.progress.bot_settling",
                reason = super::text::error(&e)
            )
            .to_string(),
        ));
        bot::bot_state(target)
    })
}

/// "The service is older than this terminal" when the station's release (`v0.51.0 (<rev>)`) is
/// behind the terminal's: "Update the service" takes it to the newest release, which is at least
/// the terminal's. Nothing for a development build on either side.
fn older_service(station_version: &str) -> Option<String> {
    use moon_core::update::ReleaseVersion;
    let station = ReleaseVersion::parse(station_version.split_whitespace().next()?)?;
    let terminal = ReleaseVersion::parse(option_env!("MOONTERMINAL_RELEASE_BASE")?)?;
    (station < terminal).then(|| {
        rust_i18n::t!(
            "telegram.server.older_service",
            station = station.to_string(),
            terminal = terminal.to_string(),
            button = rust_i18n::t!("telegram.server.update")
        )
        .to_string()
    })
}

/// Select enabled saved keys before remote work, retaining keyless names for the outcome.
fn core_keys(picked: &[u64]) -> anyhow::Result<(Vec<CoreKey>, Vec<String>)> {
    anyhow::ensure!(!picked.is_empty(), StationError::NoCorePicked);
    let all = moon_core::config::read_core_keys()?;
    install_core_keys(all, picked)
}

/// Refuse stale picks and all-keyless selections before setup, while allowing partial installs.
fn install_core_keys(
    all: Vec<moon_core::config::CoreKeyEntry>,
    picked: &[u64],
) -> anyhow::Result<(Vec<CoreKey>, Vec<String>)> {
    for uid in picked {
        anyhow::ensure!(
            all.iter().any(|entry| entry.uid == *uid),
            StationError::CoreMissing(*uid)
        );
    }
    let selected = moon_core::config::select_station_cores(
        all.into_iter()
            .filter(|entry| picked.contains(&entry.uid))
            .collect(),
    );
    if selected.keyed.is_empty() {
        return Err(match selected.skipped.first() {
            Some(name) => StationError::CoreWithoutKey(name.clone()),
            None => StationError::NoCorePicked,
        }
        .into());
    }
    let keys = selected
        .keyed
        .into_iter()
        .map(|entry| CoreKey {
            uid: entry.uid,
            name: entry.name,
            key: entry.key,
            transport: entry.transport,
            endpoint_override: entry.endpoint_override,
        })
        .collect();
    Ok((keys, selected.skipped))
}

/// Reinstallation updates address matches and adds unmatched picks without removing remote cores.
fn install_changes(
    keys: &[CoreKey],
    listing: Option<&[moon_core::station_api::ListedCore]>,
    high_water: Option<u64>,
) -> anyhow::Result<Vec<Upsert>> {
    let Some(listing) = listing else {
        return Ok(keys
            .iter()
            .map(|key| Upsert {
                terminal_uid: key.uid,
                station_uid: key.uid,
                add: true,
            })
            .collect());
    };
    let here: Vec<_> = keys
        .iter()
        .map(|key| cores_sync::local_core(key.uid, &key.name, &key.key, &key.endpoint_override))
        .collect();
    let fresh = cores_sync::reconcile(&here, listing, high_water);
    anyhow::ensure!(
        !fresh
            .iter()
            .any(|row| row.terminal_uid.is_some() && row.station_uid.is_none()),
        CoresChanged
    );
    Ok(cores_sync::bulk(&fresh))
}

/// Carry each saved key and override under its freshly reconciled station uid.
fn upsert_keys(
    all: &[moon_core::config::CoreKeyEntry],
    picked: &[Upsert],
) -> anyhow::Result<Vec<CoreKey>> {
    picked
        .iter()
        .map(|upsert| {
            let mut key = picked_core_key(all, upsert.terminal_uid)?;
            key.uid = upsert.station_uid;
            Ok(key)
        })
        .collect()
}

/// Resolve a picked credential and its stored override, rejecting missing/empty keys before install.
fn picked_core_key(all: &[moon_core::config::CoreKeyEntry], uid: u64) -> anyhow::Result<CoreKey> {
    let entry = all
        .iter()
        .find(|e| e.uid == uid)
        .ok_or_else(|| anyhow::anyhow!(StationError::CoreMissing(uid)))?;
    anyhow::ensure!(
        !entry.key.is_empty(),
        StationError::CoreWithoutKey(entry.name.clone())
    );
    Ok(CoreKey {
        uid: entry.uid,
        name: entry.name.clone(),
        transport: entry.transport,
        key: entry.key.clone(),
        endpoint_override: entry.endpoint_override.clone(),
    })
}

/// How the user gets in the first time.
pub(crate) fn first_access(
    login: String,
    password: Option<Zeroizing<String>>,
    key: Option<(String, Option<Zeroizing<String>>)>,
) -> anyhow::Result<FirstAccess> {
    match key {
        Some((path, passphrase)) => {
            let text = std::fs::read_to_string(path.trim())
                .map_err(|e| anyhow::anyhow!("read {path}: {e}"))?;
            let key = moon_remote::keys::parse(&text, passphrase.as_deref().map(|p| p.as_str()))?;
            Ok(FirstAccess::Key {
                sudo_password: (login != "root").then_some(password).flatten(),
                user: login,
                key: Box::new(key),
            })
        }
        None => Ok(FirstAccess::Password {
            password: password.ok_or_else(|| anyhow::anyhow!(StationError::EmptyPassword))?,
            user: login,
        }),
    }
}

/// The terminal's USDT valuation cache to a new station: a consistent snapshot (`VACUUM INTO`,
/// the writer keeps running) sent before the station first starts. Only a speed-up — the station
/// values everything itself without it, in hours rather than minutes — so a failure is a line in
/// the progress, not a failed install.
fn send_valuation(target: &Target, say: &mut dyn FnMut(Progress)) {
    let source = moon_core::config::paths::valuation_db_path();
    if !source.exists() {
        say(Progress::Text(
            rust_i18n::t!("station.progress.valuation_none").to_string(),
        ));
        return;
    }
    let snapshot = std::env::temp_dir().join(format!(
        "moon-valuation-snapshot-{}.sqlite",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&snapshot);
    let sent = moon_core::db::maint::backup_db(&source, &snapshot)
        .and_then(|()| station::put_valuation(target, &snapshot, say));
    let _ = std::fs::remove_file(&snapshot);
    if let Err(e) = sent {
        say(Progress::Text(
            rust_i18n::t!(
                "station.progress.valuation_failed",
                reason = super::text::error(&e)
            )
            .to_string(),
        ));
    }
}

#[cfg(test)]
mod tests;
