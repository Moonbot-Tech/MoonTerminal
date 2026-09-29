//! The station as the host of the Telegram bot and its Mini App (`moon_tg`).
//!
//! Everything the bot says and does lives in `moon-tg`, shared with the terminal; this file only
//! lends it the station: the configuration with the bot's part filled in, the sessions, the
//! reports' zone, a Panic Sell override of its own (the terminal's is shared with its chart
//! button, which the station has not), the reconnect queue its loop drains, and threads for the
//! reads that must not hold the loop.
//!
//! What the terminal keeps in `servers.enc` — the chats paired with the bot, its owner and the
//! viewers' grants — the station keeps in `telegram.json` in its data root: its own state, which
//! it rewrites on every pairing. The token is a systemd credential and never lands there.
//!
//! No settings page issues a pairing code here: while no chat is paired, the station issues one
//! itself and logs it, and `moon-remote status --logs` shows it to the administrator.

use std::collections::HashMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::Context;
use chrono_tz::Tz;
use moon_core::config::AppConfig;
use moon_core::config::telegram_access::TelegramChatAccess;
use moon_core::session::panic_override::{
    PanicLocal, effective_panic_armed, panic_local_settled, panic_snapshot_armed,
};
use moon_core::session::{CoreId, SessionManager};
use moon_core::telegram::TelegramStatus;
use moon_core::telegram::runtime::mini_app::MiniAppStatus;
use moon_tg::{Finish, HostKind, Job, TelegramState, TgHost};
use serde::{Deserialize, Serialize};

use crate::cores::Telegram;

/// The station's own Telegram state file, in the data root.
const PAIRING_FILE: &str = "telegram.json";
/// How often a lasting `Conflict` is warned about again.
const CONFLICT_LOG_EVERY: std::time::Duration = std::time::Duration::from_secs(600);

/// Who may talk to the bot: what pairing changes, saved as the station's state.
///
/// Unknown fields are ignored, not refused: a binary rolled back after a newer one wrote the file
/// must still start its bot. Nothing secret is ever written here — the token is a credential.
#[derive(Default, Serialize, Deserialize)]
struct Pairing {
    #[serde(default)]
    authorized_chat_ids: Vec<i64>,
    #[serde(default)]
    owner_chat_id: Option<i64>,
    #[serde(default)]
    chat_access: Vec<TelegramChatAccess>,
}

/// The bot, the Mini App, and what the station keeps for them between two ticks.
pub struct StationTg {
    state: TelegramState,
    zone: Tz,
    pairing_path: PathBuf,
    /// Optimistic Panic Sell override by `(core, market)`, reconciled every tick.
    panic_local: HashMap<(CoreId, String), PanicLocal>,
    /// Cores the Mini App asked to reconnect, for the loop to rebuild.
    reconnect: Vec<CoreId>,
    /// Finished reads, applied on the loop's thread.
    finished_tx: Sender<Finish>,
    finished_rx: Receiver<Finish>,
    /// The pairing code last written to the log, so each is logged once.
    logged_code: Option<String>,
    /// The bot's and the Mini App's state last written to the log: the journal is the only place
    /// the station shows them — a 409 from a second poller of the token above all.
    logged_status: Option<(TelegramStatus, MiniAppStatus)>,
    /// When the last `Conflict` was warned about.
    conflict_logged_at: Option<Instant>,
}

impl StationTg {
    /// Fill the bot's part of `config` — the token, the Mini App switch, the saved pairing — and
    /// start the bot.
    ///
    /// Args:
    ///     config: The station's configuration; its `telegram` is replaced.
    ///     telegram: `[telegram]` of `station.toml`, with its token.
    ///     data_root: Where `telegram.json` lives.
    ///
    /// Returns:
    ///     `Err` when the saved pairing cannot be read: the bot then does not start, rather than
    ///     start unpaired and overwrite the grants on the first pairing.
    pub fn start(
        config: &mut AppConfig,
        telegram: &Telegram,
        data_root: &Path,
    ) -> anyhow::Result<Self> {
        let token = telegram.token.clone().ok_or_else(|| {
            anyhow::anyhow!("no bot token: the credential telegram-token is missing or empty")
        })?;
        let pairing_path = data_root.join(PAIRING_FILE);
        let pairing = load_pairing(&pairing_path)?;
        moon_tg::set_locale(telegram.language);
        let bot = &mut config.telegram;
        bot.token = token;
        bot.mini_app_enabled = telegram.mini_app;
        bot.authorized_chat_ids = pairing.authorized_chat_ids;
        bot.owner_chat_id = pairing.owner_chat_id;
        bot.chat_access = pairing.chat_access;
        log::info!(
            "telegram: bot starting, {} chat(s) paired, Mini App {}, zone {}",
            bot.authorized_chat_ids.len(),
            if bot.mini_app_enabled { "on" } else { "off" },
            telegram.zone
        );
        let (finished_tx, finished_rx) = mpsc::channel();
        Ok(Self {
            state: TelegramState::new(bot),
            zone: telegram.zone,
            pairing_path,
            panic_local: HashMap::new(),
            reconnect: Vec::new(),
            finished_tx,
            finished_rx,
            logged_code: None,
            logged_status: None,
            conflict_logged_at: None,
        })
    }

    /// One pass of the loop: apply finished reads, drain the transport's work, reconcile the
    /// Panic Sell overrides, and keep a pairing code out while no chat is paired.
    ///
    /// Returns:
    ///     The cores the Mini App asked to reconnect since the last pass.
    pub fn tick(&mut self, config: &mut AppConfig, session: &mut SessionManager) -> Vec<CoreId> {
        let finished: Vec<Finish> = self.finished_rx.try_iter().collect();
        let mut host = StationHost {
            tg: self,
            config,
            session,
        };
        for finish in finished {
            finish(&mut host);
        }
        moon_tg::tick(&mut host);
        let StationHost {
            tg,
            config,
            session,
        } = host;
        tg.settle_panic(session);
        tg.offer_pairing(config);
        tg.log_status();
        std::mem::take(&mut tg.reconnect)
    }

    /// Ask the bot's transport to stop without waiting: a long poll winds down while the caller
    /// does its own stopping.
    pub fn request_stop(&mut self) {
        self.state.request_stop();
    }

    /// Stop and join the bot's transport.
    pub fn stop(&mut self) {
        self.state.stop();
    }

    /// Log the bot's and the Mini App's state whenever either changes.
    ///
    /// A second poller of the token makes the status alternate between `Conflict` and the
    /// working one every back-off: the conflict is warned about at most every
    /// [`CONFLICT_LOG_EVERY`], and the working status is compared with the last one logged, not
    /// with the conflict in between — so the journal gets a line per ten minutes, not per poll.
    fn log_status(&mut self) {
        let status = self.state.status.clone();
        if status == TelegramStatus::Conflict {
            if self
                .conflict_logged_at
                .is_none_or(|at| at.elapsed() >= CONFLICT_LOG_EVERY)
            {
                log::warn!(
                    "telegram: the token is polled elsewhere (a terminal or another station with \
                     the same bot, or a webhook); one token runs one bot"
                );
                self.conflict_logged_at = Some(Instant::now());
            }
            return;
        }
        let now = (status, self.state.mini_status.clone());
        if self.logged_status.as_ref() == Some(&now) {
            return;
        }
        let (status, mini) = &now;
        log::info!("telegram: bot {status:?}, Mini App {mini:?}");
        self.logged_status = Some(now);
    }

    /// Drop every override the core has confirmed or that has expired.
    fn settle_panic(&mut self, session: &SessionManager) {
        self.panic_local.retain(|(core, market), local| {
            !panic_local_settled(
                local.want,
                local.at.elapsed(),
                panic_snapshot_armed(session.store(), *core, market),
            )
        });
    }

    /// While no chat is paired, keep a pairing code issued and log each new one.
    fn offer_pairing(&mut self, config: &AppConfig) {
        if !config.telegram.authorized_chat_ids.is_empty() {
            self.logged_code = None;
            return;
        }
        if self.state.pairing.is_none() {
            moon_tg::issue_pairing(&mut self.state);
        }
        let Some((code, _)) = &self.state.pairing else {
            return;
        };
        if self.logged_code.as_ref() != Some(code) {
            log::info!(
                "telegram: no chat is paired — send /pair {code} to the bot within 10 minutes"
            );
            self.logged_code = Some(code.clone());
        }
    }
}

/// The station lent to `moon_tg` for one tick.
struct StationHost<'a> {
    tg: &'a mut StationTg,
    config: &'a mut AppConfig,
    session: &'a mut SessionManager,
}

impl StationHost<'_> {
    /// Apply `change` to the pairing, save it, and adopt it only once saved.
    fn save_pairing(
        &mut self,
        change: impl FnOnce(&mut moon_core::config::TelegramConfig),
    ) -> bool {
        let mut candidate = self.config.telegram.clone();
        change(&mut candidate);
        let pairing = Pairing {
            authorized_chat_ids: candidate.authorized_chat_ids.clone(),
            owner_chat_id: candidate.owner_chat_id,
            chat_access: candidate.chat_access.clone(),
        };
        if let Err(e) = write_pairing(&self.tg.pairing_path, &pairing) {
            log::error!("telegram: pairing not saved: {e:#}");
            return false;
        }
        self.config.telegram = candidate;
        true
    }
}

impl TgHost for StationHost<'_> {
    fn kind(&self) -> HostKind {
        HostKind::Station
    }

    fn config(&self) -> &AppConfig {
        self.config
    }

    fn session(&self) -> &SessionManager {
        self.session
    }

    fn session_mut(&mut self) -> &mut SessionManager {
        self.session
    }

    fn state(&self) -> &TelegramState {
        &self.tg.state
    }

    fn state_mut(&mut self) -> &mut TelegramState {
        &mut self.tg.state
    }

    fn report_zone(&self) -> Tz {
        self.tg.zone
    }

    fn save_paired_chat(&mut self, chat_id: i64) -> bool {
        let saved = self.save_pairing(|telegram| {
            telegram.pair_chat(chat_id);
        });
        if saved {
            log::info!(
                "telegram: a chat paired, {} in all",
                self.config.telegram.authorized_chat_ids.len()
            );
        }
        saved
    }

    fn save_cleared_pairing(&mut self) -> bool {
        self.save_pairing(|telegram| telegram.clear_pairing())
    }

    fn is_panic_armed(&self, core: CoreId, market: &str) -> bool {
        let local = self
            .tg
            .panic_local
            .iter()
            .find(|((c, m), _)| *c == core && m.as_str() == market)
            .map(|(_, l)| (l.want, l.at.elapsed()));
        effective_panic_armed(local, || {
            panic_snapshot_armed(self.session.store(), core, market)
        })
    }

    fn toggle_panic_sell(&mut self, core: CoreId, market: String) -> bool {
        let on = !self.is_panic_armed(core, &market);
        if let Err(error) = self.session.panic_sell_market(core, market.clone(), on) {
            log::warn!("panic sell market failed: {error:#}");
            return false;
        }
        self.tg.panic_local.insert(
            (core, market),
            PanicLocal {
                want: on,
                at: Instant::now(),
            },
        );
        true
    }

    fn request_reconnect(&mut self, core: CoreId) {
        if !self.tg.reconnect.contains(&core) {
            self.tg.reconnect.push(core);
        }
    }

    fn spawn(&mut self, job: Job) {
        // Handed over through a slot, so a thread that cannot be started leaves the job here to
        // run in place: a job dropped unrun would leave its read `Busy` for good.
        let slot = Arc::new(Mutex::new(Some(job)));
        let theirs = Arc::clone(&slot);
        let tx = self.tg.finished_tx.clone();
        let started = std::thread::Builder::new()
            .name("tg-read".into())
            .spawn(move || {
                let job = theirs.lock().ok().and_then(|mut job| job.take());
                if let Some(job) = job {
                    let _ = tx.send(job());
                }
            });
        if let Err(error) = started {
            log::warn!("telegram: no thread for a read ({error}); running it in place");
            let job = slot.lock().ok().and_then(|mut job| job.take());
            if let Some(job) = job {
                let _ = self.tg.finished_tx.send(job());
            }
        }
    }

    fn repaint(&mut self) {
        // Nothing on the station shows the bot's state.
    }
}

/// The saved pairing; none yet is an empty one.
fn load_pairing(path: &Path) -> anyhow::Result<Pairing> {
    match std::fs::read_to_string(path) {
        Ok(text) => {
            serde_json::from_str(&text).with_context(|| format!("parse {}", path.display()))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Pairing::default()),
        Err(e) => Err(e).with_context(|| format!("read {}", path.display())),
    }
}

/// Replace the saved pairing whole: a new file beside it, flushed, then renamed over it — a crash
/// leaves the old pairing or the new, never half of one.
fn write_pairing(path: &Path, pairing: &Pairing) -> anyhow::Result<()> {
    let tmp = path.with_extension("json.new");
    let mut file =
        std::fs::File::create(&tmp).with_context(|| format!("create {}", tmp.display()))?;
    file.write_all(serde_json::to_string_pretty(pairing)?.as_bytes())?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&tmp, path).with_context(|| format!("replace {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests;
