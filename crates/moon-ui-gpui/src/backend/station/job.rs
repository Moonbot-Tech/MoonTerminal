//! The station's server work, off the UI thread: every step is a blocking SSH exchange of seconds to
//! minutes. One job at a time; its lines and its end come back over a channel the backend drains.

use std::path::PathBuf;
use std::sync::mpsc;

use moon_core::config::Secret;
use moon_remote::setup::{self, FirstAccess, NeedsAdminPassword, Setup};
use moon_remote::ssh::Target;
use moon_remote::station::bot::{self, BotState, Pairing};
use moon_remote::station::{self, BotChange, CoreKey, TapeWindow};
use zeroize::Zeroizing;

/// The bot the station should run after a job, if any.
pub(crate) enum BotPlan {
    /// Leave the station's bot as it is.
    Keep,
    /// Hand the terminal's bot over: its token and its paired chats.
    Transfer {
        token: Secret,
        pairing: Pairing,
        change: BotChange,
    },
}

/// One unit of work on the server.
pub(crate) enum Job {
    /// Prepare a new server, install the station, send the cores, set the bot.
    Install {
        setup: Setup,
        cores: Vec<u64>,
        bot: BotPlan,
    },
    /// Run the setup again on a server set up before: updates the helper and the unit, and moves a
    /// server from before key-only sudo over (with its old administrator password).
    Resetup { setup: Setup },
    /// Install another station binary (health-checked, rolled back when it does not stay up).
    Update { target: Target, bin: PathBuf },
    /// Make the picked cores the station's whole set.
    Cores { target: Target, cores: Vec<u64> },
    /// Set the station's bot.
    Bot { target: Target, bot: BotPlan },
    /// Take the bot off the station: its token, its chats and `[telegram]`.
    BotOff { target: Target },
    /// Give the station's bot this token: a new bot, or a replacement that pairs anew.
    ServerToken {
        target: Target,
        token: Secret,
        change: BotChange,
    },
    /// Read the station's bot quietly: no lines, no outcome — the bot's status and pairing code.
    BotState { target: Target },
    /// Read the station's state.
    Status { target: Target },
    /// Read the tail of the station's journal.
    Logs { target: Target },
}

/// What a finished job reports.
pub(crate) enum Done {
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
            let mut say = move |line: &str| {
                let _ = lines.send(Event::Line(line.to_owned()));
            };
            let done = match run(job, &mut say) {
                Ok(done) => done,
                Err(e) if e.downcast_ref::<NeedsAdminPassword>().is_some() => {
                    Done::NeedsAdminPassword
                }
                Err(e) => Done::Failed {
                    station_may_poll: e
                        .downcast_ref::<moon_remote::station::bot::StationMayStillPoll>()
                        .is_some(),
                    reason: format!("{e:#}"),
                },
            };
            let _ = tx.send(Event::Done(done));
        })
        .map(|_| ())
        .unwrap_or_else(|e| log::warn!("station: no thread for the job ({e})"));
    rx
}

fn run(job: Job, say: &mut dyn FnMut(&str)) -> anyhow::Result<Done> {
    match job {
        Job::Install { setup, cores, bot } => {
            let target = setup.target.clone();
            anyhow::ensure!(
                setup.station_bin.is_some(),
                "the station binary is not chosen"
            );
            // Read before the server changes: a terminal without a core key has nothing to
            // install a station for.
            let keys = core_keys(&cores)?;
            setup::run(&setup, say)?;
            // Before the first start: the station opens the cache as its own.
            send_valuation(&target, say);
            station::push_cores(&target, &keys, tape_window(), say)?;
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
        Job::Update { target, bin } => {
            let host = known_host(&target)?;
            let app = moon_remote::app_key::load_or_create()?;
            setup::install_station(&target, &host.0, &app, Some(host.1.as_str()), &bin, say)?;
            Ok(Done::Ok {
                transferred: false,
                bot: None,
                bot_off: false,
            })
        }
        Job::Cores { target, cores } => {
            station::push_cores(&target, &core_keys(&cores)?, tape_window(), say)?;
            Ok(Done::Ok {
                transferred: false,
                bot: None,
                bot_off: false,
            })
        }
        Job::Bot { target, bot } => set_bot(&target, bot, say),
        Job::BotOff { target } => {
            station::push_telegram(&target, None, &BotChange::default(), true, say)?;
            Ok(Done::Ok {
                transferred: false,
                bot: Some(bot::bot_state(&target)?),
                bot_off: true,
            })
        }
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
        Job::Status { target } => {
            let conn = station::admin_conn(&target)?;
            let out = moon_remote::script::checked(conn.run(
                &moon_remote::script::helper("status", &[]),
                &[],
                moon_remote::script::STEP_TIMEOUT,
            )?)?;
            for line in out.stdout_text().lines() {
                say(line);
            }
            Ok(Done::Ok {
                transferred: false,
                bot: Some(bot::bot_state(&target)?),
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
            for line in out.stdout_text().lines() {
                say(line);
            }
            Ok(Done::Ok {
                transferred: false,
                bot: None,
                bot_off: false,
            })
        }
    }
}

fn set_bot(target: &Target, plan: BotPlan, say: &mut dyn FnMut(&str)) -> anyhow::Result<Done> {
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

/// The administrator and the pinned host key of a server this terminal set up.
fn known_host(target: &Target) -> anyhow::Result<(String, String)> {
    let hosts = moon_remote::hosts::Hosts::load(&moon_remote::hosts::Hosts::path())?;
    let host = hosts
        .get(&target.addr())
        .ok_or_else(|| anyhow::anyhow!("{} was never set up here", target.addr()))?;
    let admin = host
        .admin
        .clone()
        .ok_or_else(|| anyhow::anyhow!("{}: the setup did not finish", target.addr()))?;
    Ok((admin, host.fingerprint.clone()))
}

/// The picked cores' keys from the terminal's `servers.enc`, read here — never held by the view.
fn core_keys(picked: &[u64]) -> anyhow::Result<Vec<CoreKey>> {
    anyhow::ensure!(!picked.is_empty(), "no core is picked");
    let all = moon_core::config::read_core_keys()?;
    picked
        .iter()
        .map(|uid| {
            let entry = all
                .iter()
                .find(|e| e.uid == *uid)
                .ok_or_else(|| anyhow::anyhow!("core {uid} is not in servers.enc"))?;
            anyhow::ensure!(!entry.key.is_empty(), "core {:?} has no key", entry.name);
            Ok(CoreKey {
                uid: entry.uid,
                name: entry.name.clone(),
                transport: entry.transport,
                key: entry.key.clone(),
            })
        })
        .collect()
}

/// The terminal's window around a trade, for the station's tape.
fn tape_window() -> Option<TapeWindow> {
    if !moon_core::config::paths::storage_path().exists() {
        return None;
    }
    let cfg = moon_core::config::storage::load().trade_replay;
    Some(TapeWindow {
        margin_s: cfg.margin_s,
        long_position_min: cfg.long_position_min,
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
            password: password.ok_or_else(|| anyhow::anyhow!("the password is empty"))?,
            user: login,
        }),
    }
}

/// The terminal's USDT valuation cache to a new station: a consistent snapshot (`VACUUM INTO`,
/// the writer keeps running) sent before the station first starts. Only a speed-up — the station
/// values everything itself without it, in hours rather than minutes — so a failure is a line in
/// the progress, not a failed install.
fn send_valuation(target: &Target, say: &mut dyn FnMut(&str)) {
    let source = moon_core::config::paths::valuation_db_path();
    if !source.exists() {
        say("valuation cache: none in this terminal, the station values from scratch");
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
        say(&format!(
            "valuation cache: not sent ({e:#}); the station values from scratch"
        ));
    }
}
