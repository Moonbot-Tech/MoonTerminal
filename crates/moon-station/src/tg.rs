//! The station as the host of the Telegram bot and its Mini App (`moon_tg`).
//!
//! Everything the bot says and does lives in `moon-tg`, shared with the terminal; this file only
//! lends it the station: the configuration with the bot's part filled in, the sessions, the
//! reports' zone, a Panic Sell override of its own (the terminal's is shared with its chart
//! button, which the station has not), the reconnect queue its loop drains, and threads for the
//! reads that must not hold the loop.
//!
//! What the terminal keeps in `servers.enc` — the chats paired with the bot, its owner and the
//! viewers' grants, the bot's menu and report settings — the station keeps in `telegram.json` in
//! its data root: its own state, which it rewrites on every pairing. So does the zone the
//! terminal pushes from its header clock, which then wins over `station.toml`'s `[telegram]
//! zone`. The token is a systemd credential and never lands there.
//!
//! While no chat is paired the station keeps a pairing code issued itself and logs it. The
//! terminal's Settings reach the rest through the control API (`api.rs`): the bot's state, a code
//! for one more chat, and the chats' access read and replaced ([`StationTg::set_access`]).
//!
//! The chat's "Status" is the station's own status, read on the main loop
//! ([`StationTg::answer_status`]) and sent from a thread that first asks GitHub for a newer
//! release; "Update" files the request the root updater acts on (`release.rs`).

use std::collections::HashMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicU64;
use std::sync::mpsc::{self, Receiver, Sender, SyncSender};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::Context;
use chrono_tz::Tz;
use moon_core::config::AppConfig;
use moon_core::session::panic_override::{
    PanicLocal, effective_panic_armed, panic_local_settled, panic_snapshot_armed,
};
use moon_core::session::{CoreId, SessionManager};
use moon_core::station_api::{Access, BotStatus, PairingCode, Status};
use moon_core::telegram::TelegramStatus;
use moon_core::telegram::runtime::Response;
use moon_core::telegram::runtime::mini_app::MiniAppStatus;
use moon_tg::{Finish, HostKind, Job, ReportRevision, TelegramState, TgHost};

use crate::cores::Telegram;
use crate::release::{self, ReleaseWatch};

/// The station's own Telegram state file, in the data root.
const PAIRING_FILE: &str = "telegram.json";
/// Notification settings and outbox, beside [`PAIRING_FILE`].
const NOTIFY_FILE: &str = "telegram_notify.json";
/// How often a lasting `Conflict` is warned about again.
const CONFLICT_LOG_EVERY: std::time::Duration = std::time::Duration::from_secs(600);

/// The bot, the Mini App, and what the station keeps for them between two ticks.
pub struct StationTg {
    state: TelegramState,
    /// The zone reports are cut in: [`Self::zone_pushed`] when there is one, else `station.toml`'s.
    zone: Tz,
    /// The zone the terminal pushed (`telegram.json`), kept to be written back with the pairing.
    zone_pushed: Option<String>,
    /// The report writer's and the valuation worker's generations, which the Mini App's cached
    /// reads are compared on.
    generations: (Arc<AtomicU64>, Option<Arc<AtomicU64>>),
    pairing_path: PathBuf,
    /// Where the update request is filed.
    data_root: PathBuf,
    /// Chats' "Status" waiting for the main loop to read the station's status.
    status_asks: Vec<(SyncSender<Response>, bool)>,
    /// The look at the latest release each "Status" takes.
    release: ReleaseWatch,
    /// Optimistic Panic Sell override by `(core, market)`, reconciled every tick.
    panic_local: HashMap<(CoreId, String), PanicLocal>,
    /// Cores the Mini App asked to reconnect, for the loop to rebuild.
    reconnect: Vec<CoreId>,
    /// Finished reads, applied on the loop's thread.
    finished_tx: Sender<Finish>,
    finished_rx: Receiver<Finish>,
    /// The pairing code last written to the log, so each is logged once.
    logged_code: Option<String>,
    /// The bot's and the Mini App's state last written to the log, so a change is logged once: the
    /// administrator's record of them beside the API's live answer — a 409 from a second poller of
    /// the token above all.
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
    ///     generations: The report writer's generation and the valuation worker's, when it runs.
    ///
    /// Returns:
    ///     `Err` when the saved pairing cannot be read: the bot then does not start, rather than
    ///     start unpaired and overwrite the grants on the first pairing.
    pub fn start(
        config: &mut AppConfig,
        telegram: &Telegram,
        data_root: &Path,
        generations: (Arc<AtomicU64>, Option<Arc<AtomicU64>>),
    ) -> anyhow::Result<Self> {
        let token = telegram.token.clone().ok_or_else(|| {
            anyhow::anyhow!("no bot token: the credential telegram-token is missing or empty")
        })?;
        let pairing_path = data_root.join(PAIRING_FILE);
        let mut pairing = load_pairing(&pairing_path)?;
        if pairing.adopt_legacy_owner() {
            write_pairing(&pairing_path, &pairing)?;
        }
        moon_tg::set_locale(telegram.language);
        // A pushed zone this build cannot read leaves the one `station.toml` names.
        if let Some(name) = pairing.zone.as_deref().filter(|n| n.parse::<Tz>().is_err()) {
            log::warn!("telegram: zone {name:?} in {PAIRING_FILE} is not an IANA time zone");
        }
        let pushed = pushed_zone(&pairing);
        let zone = pushed.as_ref().map_or(telegram.zone, |(_, zone)| *zone);
        // The core groups a terminal sent, for the report by cores; uids are the terminal's.
        config.core_groups = pairing.groups.clone().unwrap_or_default();
        moon_core::config::sanitize_core_groups(&mut config.core_groups);
        let bot = &mut config.telegram;
        bot.token = token;
        bot.mini_app_enabled = telegram.mini_app;
        bot.authorized_chat_ids = pairing.authorized_chat_ids;
        bot.owner_chat_id = pairing.owner_chat_id;
        bot.chat_access = pairing.chat_access;
        bot.bot = pairing.bot.unwrap_or_default();
        log::info!(
            "telegram: bot starting, {} chat(s) paired, Mini App {}, zone {zone}",
            bot.authorized_chat_ids.len(),
            if bot.mini_app_enabled { "on" } else { "off" },
        );
        let (finished_tx, finished_rx) = mpsc::channel();
        Ok(Self {
            state: TelegramState::new_with_notifications(
                bot,
                HostKind::Station,
                Some(data_root.join(NOTIFY_FILE)),
            ),
            zone,
            zone_pushed: pushed.map(|(name, _)| name),
            generations,
            pairing_path,
            data_root: data_root.to_path_buf(),
            status_asks: Vec::new(),
            release: ReleaseWatch::default(),
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

    /// Answer the chats that asked for "Status" since the last pass: the station's status read now
    /// through `status` (given the bot's own), sent from a thread that first looks for a newer
    /// release — GitHub can take seconds, the loop must not.
    pub fn answer_status(&mut self, config: &AppConfig, status: impl FnOnce(BotStatus) -> Status) {
        if self.status_asks.is_empty() {
            return;
        }
        let asks = std::mem::take(&mut self.status_asks);
        let status = status(self.status(config));
        let navigation = moon_tg::station_owner_navigation(&config.telegram);
        let release = self.release.clone();
        let answer = move |asks: Vec<(SyncSender<Response>, bool)>,
                           check: moon_tg::ReleaseCheck| {
            for (ask, from_settings) in asks {
                let _ = ask.try_send(moon_tg::station_status_reply(
                    &status,
                    &check,
                    navigation.clone(),
                    from_settings,
                ));
            }
        };
        // Handed over through a slot, so a thread that cannot be started leaves the answer here:
        // then without the look at the release, rather than none at all.
        let slot = Arc::new(Mutex::new(Some((asks, answer))));
        let theirs = Arc::clone(&slot);
        let started = std::thread::Builder::new()
            .name("tg-status".into())
            .spawn(move || {
                let taken = theirs.lock().ok().and_then(|mut slot| slot.take());
                if let Some((asks, answer)) = taken {
                    answer(asks, release.check_within());
                }
            });
        if let Err(error) = started {
            log::warn!(
                "telegram: no thread for the status ({error}); answering without a release check"
            );
            let taken = slot.lock().ok().and_then(|mut slot| slot.take());
            if let Some((asks, answer)) = taken {
                answer(
                    asks,
                    moon_tg::ReleaseCheck::Failed(moon_tg::ReleaseFailure::Unavailable(
                        error.to_string(),
                    )),
                );
            }
        }
    }

    /// The bot now, for the control API.
    pub fn status(&self, config: &AppConfig) -> BotStatus {
        BotStatus {
            status: self.state.status.clone(),
            mini_app_on: config.telegram.mini_app_enabled,
            mini_app: self.state.mini_status.clone(),
            pairing: self.pairing_code(),
        }
    }

    /// A fresh ten-minute code for one more chat; `None` while the bot's transport is not up —
    /// then the code offered so far stays.
    pub fn issue_pairing(&mut self) -> Option<PairingCode> {
        let offered = self.state.pairing.clone();
        moon_tg::issue_pairing(&mut self.state);
        if self.state.pairing.is_none() {
            self.state.pairing = offered;
            return None;
        }
        self.pairing_code()
    }

    /// The chats, the bot's settings and the zone in force, as the control API answers them.
    pub fn access(&self, config: &AppConfig) -> Access {
        Access {
            zone: Some(self.zone.name().to_owned()),
            notify: moon_tg::notify_rows(&self.state, &config.telegram),
            groups: Some(config.core_groups.clone()),
            ..Access::of(&config.telegram)
        }
    }

    /// Replace the paired chats with `access`, only while they are still `base` — what the
    /// terminal read before its user edited them; a chat paired here since is not dropped, nor
    /// bot settings changed here since. The bot's settings and the zone `access` leaves out stay
    /// as they are. Saved before it is adopted, then applied to the running bot as a terminal's
    /// Save applies it: a revoked or changed grant restarts the transport; captions, added chats,
    /// the menu and the zone reach it in place.
    pub fn set_access(
        &mut self,
        config: &mut AppConfig,
        base: &Access,
        access: Access,
    ) -> Result<Access, String> {
        let notify = access.notify.clone();
        // Chats' notifications are checked against the chats they will land on before anything
        // is written: a stale or invalid row refuses the whole change, not half of it.
        if let Some(rows) = notify.as_ref().filter(|rows| !rows.is_empty()) {
            let mut landing = config.telegram.clone();
            Access {
                notify: None,
                ..access.clone()
            }
            .apply_to(&mut landing);
            moon_tg::check_notify_rows(&self.state, &landing, rows)?;
        }
        let saved = plan_access(
            &self.access(config),
            self.zone_pushed.as_deref(),
            base,
            access,
        )?;
        write_pairing(&self.pairing_path, &saved).map_err(|e| format!("{e:#}"))?;
        let before = config.telegram.clone();
        saved.apply_to(&mut config.telegram);
        if let Some(groups) = &saved.groups {
            config.core_groups = groups.clone();
        }
        if let Some((name, zone)) = pushed_zone(&saved) {
            if zone != self.zone {
                log::info!("telegram: report zone now {name}");
            }
            self.zone = zone;
            self.zone_pushed = Some(name);
        }
        moon_tg::reconcile(&mut self.state, &config.telegram, &before);
        log::info!(
            "telegram: chats replaced by the terminal, {} paired",
            saved.authorized_chat_ids.len()
        );
        // Chats' notifications go to their own file, against the chats just saved.
        if let Some(rows) = notify.filter(|rows| !rows.is_empty()) {
            moon_tg::save_notify_rows(&self.state, &config.telegram, &rows, self.zone)?;
            log::info!(
                "telegram: notifications of {} chat(s) changed by the terminal",
                rows.len()
            );
        }
        Ok(self.access(config))
    }

    /// The code the bot accepts now, while it is still accepted.
    fn pairing_code(&self) -> Option<PairingCode> {
        let (code, until) = self.state.pairing.as_ref()?;
        let left = until.saturating_duration_since(Instant::now());
        (!left.is_zero()).then(|| PairingCode {
            code: code.clone(),
            expires_in_s: left.as_secs(),
        })
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
                "telegram: no chat is paired — the pairing code is shown in the terminal's Settings"
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
        let pairing = Access {
            zone: self.tg.zone_pushed.clone(),
            // A pairing saved from the chat keeps the groups the terminal sent.
            groups: Some(self.config.core_groups.clone()),
            ..Access::of(&candidate)
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

    /// Return the station's notification file beside its pairing state without creating it.
    fn notifications_path(&self) -> PathBuf {
        self.tg.data_root.join(NOTIFY_FILE)
    }

    fn report_revision(&self) -> Option<ReportRevision> {
        let (reports, valuation) = &self.tg.generations;
        ReportRevision::current(reports, valuation.as_deref())
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

    fn save_bot_settings(&mut self, bot: moon_core::config::telegram_menu::BotSettings) -> bool {
        let saved = self.save_pairing(|telegram| telegram.bot = bot);
        if saved {
            log::info!("telegram: the bot's settings changed from the chat");
        }
        saved
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

    fn station_status(&mut self, reply: SyncSender<Response>, from_settings: bool) -> bool {
        // The status is read on the loop after this tick, where the cores and the host are.
        self.tg.status_asks.push((reply, from_settings));
        true
    }

    fn request_station_update(&mut self) -> Option<Result<(), moon_tg::UpdateRefusal>> {
        Some(release::request_update(&self.tg.data_root))
    }
}

/// The zone a saved pairing carries, when it names one this build reads.
fn pushed_zone(pairing: &Access) -> Option<(String, Tz)> {
    let name = pairing.zone.as_deref()?;
    name.parse::<Tz>().ok().map(|zone| (name.to_owned(), zone))
}

/// What a change from the terminal saves, or why it is refused.
///
/// Args:
///     current: The station's access now ([`StationTg::access`]: its zone is the one in force).
///     pushed: The zone the terminal pushed earlier, if any — kept when the change names none,
///         so `station.toml`'s zone stays the fallback until a push.
///     base: What the terminal read before its edit; refused when it no longer holds.
///     access: The change; a part it leaves out keeps the station's.
///
/// Returns:
///     The whole pairing to save, with the bot's settings always present.
fn plan_access(
    current: &Access,
    pushed: Option<&str>,
    base: &Access,
    access: Access,
) -> Result<Access, String> {
    if !base.base_holds(current) {
        return Err(
            "the chats or the bot's settings changed on the station after these edits began: discard the edits and make them again".into(),
        );
    }
    access.check()?;
    if let Some(name) = access.zone.as_deref().filter(|n| n.parse::<Tz>().is_err()) {
        return Err(format!("zone {name:?} is not an IANA time zone"));
    }
    // Groups go on their own and are not part of the base: a terminal's set replaces the
    // station's whole, kept in the shape the terminal's own are.
    let groups = access.groups.clone().map(|mut groups| {
        moon_core::config::sanitize_core_groups(&mut groups);
        groups
    });
    Ok(Access {
        bot: access.bot.or_else(|| current.bot.clone()),
        zone: access.zone.or_else(|| pushed.map(str::to_owned)),
        groups: groups.or_else(|| current.groups.clone()),
        // Notifications have a file of their own, never `telegram.json`.
        notify: None,
        ..access
    })
}

/// Whether the bot's saved menu shows the Control section — only a menu the terminal delivered;
/// a station that never got one runs as it did. An unreadable file says no, and says why in the
/// log: the bot will not start on it either.
pub fn control_shown(data_root: &Path) -> bool {
    match load_pairing(&data_root.join(PAIRING_FILE)) {
        Ok(pairing) => pairing.bot.is_some_and(|bot| {
            bot.menu
                .shows(moon_core::config::telegram_menu::MenuItem::Control)
        }),
        Err(e) => {
            log::warn!("telegram: {PAIRING_FILE} unreadable, the light profile stays: {e:#}");
            false
        }
    }
}

/// The saved pairing; none yet is an empty one.
fn load_pairing(path: &Path) -> anyhow::Result<Access> {
    match std::fs::read_to_string(path) {
        Ok(text) => {
            serde_json::from_str(&text).with_context(|| format!("parse {}", path.display()))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Access::default()),
        Err(e) => Err(e).with_context(|| format!("read {}", path.display())),
    }
}

/// Replace the saved pairing whole: a new file beside it, flushed, then renamed over it — a crash
/// leaves the old pairing or the new, never half of one.
fn write_pairing(path: &Path, pairing: &Access) -> anyhow::Result<()> {
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
