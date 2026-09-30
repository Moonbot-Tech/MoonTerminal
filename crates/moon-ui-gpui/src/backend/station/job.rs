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
use zeroize::Zeroizing;

/// The bot the station should run after a job, if any.
pub(crate) enum BotPlan {
    /// Leave the station's bot as it is.
    Keep,
    /// Hand the terminal's bot over: its token and its paired chats.
    Transfer {
        token: Secret,
        pairing: Access,
        change: BotChange,
    },
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
    /// Make the picked cores the station's whole set.
    Cores { target: Target, cores: Vec<u64> },
    /// Set the station's window around a trade.
    Tape { target: Target, tape: TapeWindow },
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
    /// Read the station's bot quietly: no lines, no outcome — its status, code and chats.
    BotState { target: Target },
    /// A pairing code from the station's bot, for one more chat.
    PairIssue { target: Target },
    /// Replace the station's paired chats, only while they are still `base` (as read).
    Access {
        target: Target,
        base: Access,
        access: Access,
    },
    /// Switch the station's Mini App on or off: `[telegram] mini_app`, then a restart — the
    /// station picks its profile at start.
    MiniApp { target: Target, on: bool },
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

/// What a finished job reports.
pub(crate) enum Done {
    /// The station stopped polling and removed its bot; recovered data may be saved locally.
    BotOff { returned: Option<bot::ReturnedBot> },
    /// The destination fingerprint awaits an explicit confirmation in the Station tab.
    AddressProbed(station::access::AddressChange),
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

pub(crate) enum Event {
    /// Quarantined before removal; never starts a local poller until the successful end.
    Returned(bot::ReturnedBot),
    Line(String),
    Done(Done),
}

/// Start `job` on its own thread.
pub(crate) fn start(job: Job) -> mpsc::Receiver<Event> {
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("station-setup".into())
        .spawn(move || {
            let lines = tx.clone();
            let mut say = move |event: Progress| {
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
            let keys = core_keys(&cores)?;
            setup::run(&setup, say)?;
            // Before the first start: the station opens the cache as its own.
            send_valuation(&target, say);
            // The terminal's window only starts a station that has none.
            station::push_cores(&target, &keys, station::terminal_tape(), say)?;
            set_bot(&target, bot, say)
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
        Job::Cores { target, cores } => {
            // A remote wipe may have succeeded even when forgetting the local record failed,
            // or another terminal removed the station. A Save must not reinstall its secrets.
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
            // The station's window is set by hand only: a change of cores brings none.
            station::push_cores(&target, &core_keys(&cores)?, None, say)?;
            Ok(Done::Ok {
                transferred: false,
                bot: None,
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
        Job::Bot { target, bot } => set_bot(&target, bot, say),
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
            // change is saved either way, so a bot slow to come back is a line, not a failure.
            let state = settled(&target, bot::wait_bot(&target, say), say)?;
            Ok(Done::Ok {
                transferred: false,
                bot: Some(state),
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

/// Preserve readable Status feedback even before the service exposes its control API.
fn show_status(state: &BotState, say: &mut dyn FnMut(Progress)) {
    if let Some(station) = &state.station {
        for line in moon_tg::station_status_text(station).lines() {
            say(Progress::Text(line.to_owned()));
        }
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

/// The explicitly requested journal stays visible beneath a localized heading as details.
/// This is diagnostic content, distinct from the helper's hidden status/progress tokens.
fn show_journal(text: &str, say: &mut dyn FnMut(Progress)) {
    say(Progress::Text(
        rust_i18n::t!("station.progress.logs").to_string(),
    ));
    for line in text.lines() {
        say(Progress::Text(
            rust_i18n::t!("station.detail", detail = line).to_string(),
        ));
    }
}

/// Apply the selected bot plan without changing ownership decisions.
fn set_bot(target: &Target, plan: BotPlan, say: &mut dyn FnMut(Progress)) -> anyhow::Result<Done> {
    match plan {
        BotPlan::Keep => Ok(Done::Ok {
            transferred: false,
            bot: None,
            bot_off: false,
        }),
        BotPlan::Transfer {
            token,
            pairing,
            change,
        } => {
            let state = bot::transfer_bot(target, &token, &pairing, &change, say)?;
            Ok(Done::Ok {
                transferred: true,
                bot: Some(state),
                bot_off: false,
            })
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

/// The picked cores' keys from the terminal's `servers.enc`, read here — never held by the view.
fn core_keys(picked: &[u64]) -> anyhow::Result<Vec<CoreKey>> {
    anyhow::ensure!(!picked.is_empty(), StationError::NoCorePicked);
    let all = moon_core::config::read_core_keys()?;
    picked
        .iter()
        .map(|uid| picked_core_key(&all, *uid))
        .collect()
}

/// Resolve a picked core without allowing missing or empty keys into an install.
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
