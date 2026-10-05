//! The station's server work, shared by the Station and Telegram Settings tabs, owned by the backend.
//!
//! A job runs for minutes and may hand the terminal's bot over; its end must be applied whether
//! or not the Settings window is still open — a lost end would leave the terminal's bot down, or
//! its token on disk while the station polls it. So the jobs, their lines and their ends live
//! here, and the Settings segment only starts jobs and shows this state.
//!
//! One job runs at a time: the user's (a button; it owns the progress lines and the
//! outcome), a quiet read of the station's bot (on opening the tab; it touches only the bot's
//! state). A user's job pressed during a quiet read waits for it. A recovery read resolves a
//! persisted hand-over before local polling.
//!
//! Handing the bot over: the terminal's transport is suspended first (`TelegramState::suspend`:
//! one token, one poller) with the saved configuration untouched, so a Save meanwhile writes the
//! token as it was. The station's bot polls (paired, or offering a code when no chat came along) →
//! the token and the chats are erased here, on disk and in an open Settings draft; a failure the
//! station undid → local polling may resume only without a known station; a failure it could not
//! undo → the terminal's bot stays down, and the user is told to take it off the station. Ownership is journaled before
//! suspension and cleared only after the end is safely applied, so quitting cannot release it.

use std::sync::mpsc;
use std::time::Duration;

use gpui::{Context, Task};
use moon_core::config::AppConfig;
use moon_remote::station::bot::BotState;
use rust_i18n::t;

use crate::Backend;

pub(crate) mod cores_sync;
pub(crate) mod job;
pub(crate) mod pull;
mod recovery;
pub(crate) mod text;

/// How often the backend drains a running job's channel.
const POLL: Duration = Duration::from_millis(150);
/// How many of a job's lines are kept.
const MAX_LINES: usize = 400;

/// Who started a job, which decides what its end may change.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Kind {
    /// A button: owns the progress lines and the outcome.
    #[default]
    User,
    /// A read of the bot's state: touches that state only.
    Quiet,
    /// The header clock's zone pushed to the station: appends its line, keeps the outcome, and
    /// like a quiet read does not hold the buttons.
    Zone,
    /// A startup read that resolves durable bot ownership.
    Recovery,
}

/// Backend-owned bot-host admission and station jobs, as the Settings segment shows them.
#[derive(Default)]
pub(crate) struct StationJobs {
    /// A configured station owns the terminal's only bot, even when its service is unreachable.
    known: bool,
    /// A probe's exact destination and key awaiting explicit UI confirmation.
    pub(crate) address_change: Option<moon_remote::station::access::AddressChange>,
    /// A new server's host key awaiting explicit UI confirmation before the install.
    pub(crate) install_probe: Option<moon_remote::setup::HostKey>,
    pub(crate) lines: Vec<String>,
    /// Facts from the last explicit Status action, independent of background bot reads.
    pub(crate) status: Option<moon_tg::StatusFacts>,
    /// Bumped per line, so a view follows the newest one even once the list is full.
    pub(crate) line_seq: u64,
    pub(crate) running: bool,
    /// The last job's end: `Ok(summary)` or `Err(reason)`.
    pub(crate) outcome: Option<Result<String, String>>,
    pub(crate) bot: Option<BotState>,
    /// Outer None means unread; inner None means an older station without listing support.
    pub(crate) cores_seen: Option<Option<Vec<moon_core::station_api::ListedCore>>>,
    /// Retain the allocation floor with its listing across restart gaps.
    pub(crate) core_uid_high_water: Option<u64>,
    /// The last explicit core operation's result line.
    pub(crate) cores_result: Option<String>,
    /// The running job changes station cores.
    pub(crate) cores_job: bool,
    /// Why the last read of the bot's state failed; shown instead of a state that may be stale.
    pub(crate) bot_error: Option<String>,
    /// The server predates key-only sudo: its old administrator password is asked once.
    pub(crate) needs_old_admin: bool,
    /// Bumped at every user job's end, so an open segment clears the secrets it was given.
    pub(crate) finished: u64,
    /// Bumped when a hand-over erased the terminal's token, so the segment clears its input.
    pub(crate) erased: u64,
    /// Bumped after restoration so Settings refreshes its masked token input.
    pub(crate) restored: u64,
    /// An incomplete removal or failed disk save retains the credential for another return attempt.
    returned: Option<moon_remote::station::bot::ReturnedBot>,
    /// Bumped on every change of this state: part of the Settings window's repaint signature.
    pub(crate) revision: u64,
    /// Durable hand-over still awaiting a safe ownership decision.
    pending: Option<recovery::Pending>,
    /// An unreadable journal must never be treated as absent or overwritten.
    journal_unreadable: bool,
    kind: Kind,
    /// The running job hands the terminal's bot over.
    handing_over: bool,
    /// A user's job pressed while a quiet read or a zone push ran: it starts right after.
    waiting: Option<(job::Job, bool)>,
    /// The last user job said nothing about the bot: its state is read again after it.
    pending_refresh: bool,
    /// The running job is an "Apply on the server" of the chats and the bot's menu.
    pub(crate) applying: bool,
    /// How the last "Apply on the server" ended, shown beside its buttons; cleared by a revert.
    pub(crate) applied: Option<Result<String, String>>,
    /// The zone the last zone push carried: after it ends the zone is pushed again only when the
    /// header clock moved meanwhile, so a station that refuses it is not asked again at once.
    zone_pushed: Option<String>,
    task: Option<Task<()>>,
}

impl Kind {
    /// Work the user did not ask for and need not wait on: a user's job pressed meanwhile starts
    /// right after it.
    fn is_background(self) -> bool {
        matches!(self, Self::Quiet | Self::Zone)
    }
}

impl StationJobs {
    /// Keep the retry action available while removal or the local save still needs recovery.
    pub(crate) fn has_returned_bot(&self) -> bool {
        self.returned.is_some()
    }

    /// Return the recovery refusal key without initializing the UI dictionary on a worker stack.
    pub(crate) fn access_refusal(&self) -> Option<&'static str> {
        if self.holds_bot() {
            Some("telegram.server.access_pending_handover")
        } else if self.has_returned_bot() {
            Some("telegram.server.access_pending_return")
        } else {
            None
        }
    }

    /// A job the user sees as work in progress: buttons wait for it. A quiet read or a zone push
    /// does not count.
    pub(crate) fn busy(&self) -> bool {
        self.running && !self.kind.is_background()
    }

    /// Load station presence and durable ownership before saved Telegram can start transport.
    pub(crate) fn load() -> Self {
        let known = known_target().is_some();
        match recovery::load(&moon_core::config::paths::station_handover_path()) {
            Ok(pending) => Self {
                known,
                pending,
                ..Self::default()
            },
            Err(_) => Self {
                known,
                journal_unreadable: true,
                ..Self::default()
            },
        }
    }

    /// An outstanding or unreadable journal forbids starting the saved local token.
    pub(crate) fn holds_bot(&self) -> bool {
        self.pending.is_some() || self.journal_unreadable
    }

    /// Permit local transport only without a station or unresolved ownership journal.
    pub(crate) fn allows_terminal_bot(&self) -> bool {
        !self.known && !self.holds_bot()
    }

    /// Append a bounded progress line and advance the view cursor.
    fn push_line(&mut self, line: String) {
        self.lines.push(line);
        self.line_seq = self.line_seq.wrapping_add(1);
        if self.lines.len() > MAX_LINES {
            self.lines.drain(..self.lines.len() - MAX_LINES);
        }
    }

    /// Retain a listing through a restart gap; an answering old station reports unsupported.
    fn observe_bot(&mut self, bot: BotState) {
        if let Some(status) = &bot.station {
            self.cores_seen = Some(status.cores.clone());
            self.core_uid_high_water = status.core_uid_high_water;
        }
        self.bot = Some(bot);
    }

    /// The server was forgotten: nothing learnt about it applies to the next one.
    fn clear_known(&mut self, outcome: Result<String, String>) {
        let st = self;
        st.known = false;
        st.address_change = None;
        st.install_probe = None;
        st.bot = None;
        st.cores_seen = None;
        st.core_uid_high_water = None;
        st.cores_result = None;
        st.cores_job = false;
        st.bot_error = None;
        st.needs_old_admin = false;
        st.pending_refresh = false;
        st.zone_pushed = None;
        st.applying = false;
        st.applied = None;
        st.waiting = None;
        st.lines.clear();
        st.status = None;
        st.outcome = Some(outcome);
        st.revision = st.revision.wrapping_add(1);
    }
}

/// The station's server this terminal set up, if any (`remote/hosts.toml`).
pub(crate) fn known_target() -> Option<moon_remote::ssh::Target> {
    let hosts = moon_remote::hosts::Hosts::load(&moon_remote::hosts::Hosts::path()).ok()?;
    let host = hosts.first_set_up()?;
    let (name, port) = host.addr.rsplit_once(':')?;
    Some(moon_remote::ssh::Target {
        host: name.trim_matches(['[', ']']).to_owned(),
        port: port.parse().ok()?,
    })
}

impl Backend {
    /// Start the user's `job`; `hand_over` journals ownership and suspends the terminal's bot. During a
    /// quiet read or a zone push it waits for that; during another job it is not started (the
    /// buttons wait).
    pub(crate) fn station_start(&mut self, job: job::Job, hand_over: bool, cx: &mut Context<Self>) {
        if self.station.running {
            if self.station.kind.is_background() {
                self.station.waiting = Some((job, hand_over));
            }
            return;
        }
        self.station_begin(job, hand_over, Kind::User, cx);
    }

    /// Read the station's bot quietly, or reconcile an outstanding hand-over at its original
    /// destination. Ordinary reads preserve the outcome; recovery updates it. A running job waits.
    pub(crate) fn station_refresh_bot(&mut self, cx: &mut Context<Self>) {
        if self.station.running {
            return;
        }
        if let Some(pending) = &self.station.pending {
            self.station_begin(
                job::Job::BotState {
                    target: pending.target(),
                },
                false,
                Kind::Recovery,
                cx,
            );
        } else if let Some(target) = known_target() {
            self.station_begin(job::Job::BotState { target }, false, Kind::Quiet, cx);
        }
    }

    /// Push the header clock's zone to the station when the station's bot reports another one;
    /// a job that runs pushes it after. Nothing for a station without a bot or one that predates
    /// the bot's settings (its access carries none). A push that failed is tried again after the
    /// next job or read, never straight from its own end (see [`Self::station_next`]).
    pub(crate) fn station_zone_sync(&mut self, cx: &mut Context<Self>) {
        let wanted = moon_core::util::display_time::zone_or_utc(self.header_clock_zone())
            .name()
            .to_owned();
        let access = self
            .station
            .bot
            .as_ref()
            .and_then(|bot| bot.access.as_ref());
        if self.station.running || !zone_push_due(access, &wanted) {
            return;
        }
        let Some(target) = known_target() else {
            return;
        };
        self.station.zone_pushed = Some(wanted.clone());
        self.station_begin(
            job::Job::Zone {
                target,
                zone: wanted,
            },
            false,
            Kind::Zone,
            cx,
        );
    }

    /// Drop cached state and queued work after a confirmed forget, removal or address change.
    pub(crate) fn station_forgotten(&mut self, outcome: Result<String, String>) {
        self.station.clear_known(outcome);
        // Address changes use this path too; re-read instead of assuming the last station is gone.
        self.station.known = known_target().is_some();
        self.resume_terminal_telegram();
    }

    /// Keep saved local tokens gated while resolving outstanding hand-over ownership first.
    pub(crate) fn station_recover(&mut self, cx: &mut Context<Self>) {
        if self.station.holds_bot() {
            self.station.outcome = Some(Err(t!("telegram.server.local_bot_held").to_string()));
            if self.station.pending.is_some() {
                self.station_refresh_bot(cx);
            }
        }
    }

    /// Keep local transport revoked until a configured station and any ownership hold are gone.
    pub(crate) fn resume_terminal_telegram(&mut self) {
        if self.station.allows_terminal_bot() {
            self.telegram.resume(&self.config.telegram);
        } else {
            self.gate_terminal_telegram();
        }
    }

    /// Clear the durable journal before releasing local polling; failure keeps the bot held.
    fn station_clear_handover(&mut self) -> anyhow::Result<()> {
        recovery::save(&moon_core::config::paths::station_handover_path(), None)?;
        self.station.pending = None;
        Ok(())
    }

    /// Start remote work only after publishing any hand-over ownership synchronously.
    fn station_begin(
        &mut self,
        job: job::Job,
        hand_over: bool,
        kind: Kind,
        cx: &mut Context<Self>,
    ) {
        // Recheck at execution time too: a queued action must not discard recovery access.
        if job.changes_access()
            && let Some(reason) = self.station.access_refusal()
        {
            self.station.outcome = Some(Err(t!(reason).to_string()));
            self.station.revision = self.station.revision.wrapping_add(1);
            cx.notify();
            return;
        }
        let job = match job {
            job::Job::BotOff { target, .. } => {
                // A queued job decides from the current saved and draft state when it starts.
                let restore = can_restore_bot(
                    &self.config.telegram,
                    self.preview.as_ref().map(|p| &p.telegram),
                );
                job::Job::BotOff {
                    target,
                    restore,
                    recovered: self.station.returned.clone(),
                }
            }
            other => other,
        };
        let hand_over = hand_over || job.handover_target().is_some();
        if hand_over {
            let pending = job.handover_target().map(recovery::Pending::new);
            let saved = if self.station.holds_bot() || pending.is_none() {
                Err(anyhow::anyhow!("unresolved station hand-over"))
            } else {
                recovery::save(
                    &moon_core::config::paths::station_handover_path(),
                    pending.as_ref(),
                )
            };
            if saved.is_err() {
                self.station.outcome =
                    Some(Err(t!("telegram.server.handover_save_failed").to_string()));
                self.station.revision = self.station.revision.wrapping_add(1);
                cx.notify();
                return;
            }
            self.station.pending = pending;
            self.telegram.suspend();
        }
        let applying = matches!(job, job::Job::Access { edits: true, .. });
        let st = &mut self.station;
        st.running = true;
        st.applying = applying;
        st.cores_job = matches!(job, job::Job::Cores { .. } | job::Job::CoresRemove { .. });
        if st.cores_job {
            st.cores_result = None;
        }
        if applying {
            st.applied = None;
        }
        st.kind = kind;
        st.handing_over = hand_over;
        match kind {
            Kind::User => {
                st.address_change = None;
                st.install_probe = None;
                st.outcome = None;
                st.lines.clear();
                st.status = None;
            }
            Kind::Quiet | Kind::Zone | Kind::Recovery => {}
        }
        st.revision = st.revision.wrapping_add(1);
        let rx = job::start(job);
        st.task = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(POLL).await;
                let mut events = Vec::new();
                let mut gone = false;
                loop {
                    match rx.try_recv() {
                        Ok(event) => events.push(event),
                        Err(mpsc::TryRecvError::Empty) => break,
                        Err(mpsc::TryRecvError::Disconnected) => {
                            gone = true;
                            break;
                        }
                    }
                }
                // A job thread gone without its end still ends the job here: nothing else would.
                if gone && !events.iter().any(|e| matches!(e, job::Event::Done(_))) {
                    events.push(job::Event::Done(job::Done::Failed {
                        reason: t!("telegram.server.job_lost").to_string(),
                        station_may_poll: hand_over,
                    }));
                }
                // Nothing new wakes nobody: the backend has many observers.
                if events.is_empty() {
                    continue;
                }
                let mut finished = false;
                let alive = this.update(cx, |b, cx| {
                    for event in events {
                        finished |= b.station_event(event);
                    }
                    b.station.revision = b.station.revision.wrapping_add(1);
                    if finished {
                        b.station_next(cx);
                    }
                    cx.notify();
                });
                if alive.is_err() || finished {
                    break;
                }
            }
        }));
        cx.notify();
    }

    /// After a job: start queued user work, then refresh when the last job gave no bot state.
    fn station_next(&mut self, cx: &mut Context<Self>) {
        if let Some((job, hand_over)) = self.station.waiting.take() {
            self.station_begin(job, hand_over, Kind::User, cx);
        }
        if !self.station.running && std::mem::take(&mut self.station.pending_refresh) {
            self.station_refresh_bot(cx);
        }
        // Last: with the bot's state as now read, its zone may lag the header clock's. After a push
        // itself only when the header clock moved during it: the same zone refused would be sent
        // again at once, in a loop.
        let wanted = moon_core::util::display_time::zone_or_utc(self.header_clock_zone()).name();
        if self.station.kind != Kind::Zone || self.station.zone_pushed.as_deref() != Some(wanted) {
            self.station_zone_sync(cx);
        }
    }

    /// Apply one job event; `true` once the job has ended.
    fn station_event(&mut self, event: job::Event) -> bool {
        let done = match event {
            job::Event::Returned(returned) => {
                self.station.returned = Some(returned);
                return false;
            }
            job::Event::Line(line) => {
                self.station.push_line(line);
                return false;
            }
            job::Event::CoresResult(line) => {
                if self.station.cores_job {
                    self.station.cores_result = Some(line);
                }
                return false;
            }
            job::Event::Status(facts) => {
                self.station.status = Some(facts);
                return false;
            }
            job::Event::Done(done) => done,
        };
        self.station.known = known_target().is_some();
        self.gate_terminal_telegram();
        let handing_over = std::mem::take(&mut self.station.handing_over);
        let kind = self.station.kind;
        self.station.running = false;
        self.station.cores_job = false;
        if kind == Kind::Recovery {
            self.station_apply_recovery(done);
            return true;
        }
        if kind == Kind::Zone {
            // A zone push changes only the bot's state; its line says what happened.
            match done {
                job::Done::Ok { bot, .. } => {
                    if let Some(bot) = bot {
                        self.station.observe_bot(bot);
                        self.station.bot_error = None;
                    }
                }
                job::Done::NeedsAdminPassword => {
                    // The Station tab then asks for the old administrator password.
                    self.station.needs_old_admin = true;
                    self.station
                        .push_line(t!("telegram.server.needs_old_admin").to_string());
                }
                job::Done::Failed { reason, .. } => self.station.push_line(reason),
                job::Done::BotOff { .. }
                | job::Done::AddressProbed(_)
                | job::Done::InstallProbed(_)
                | job::Done::AddressChanged
                | job::Done::Removed { .. } => {}
            }
            return true;
        }
        if kind == Kind::Quiet {
            // A quiet read changes only the bot's state, or says why it could not read it.
            match done {
                job::Done::Ok { bot: Some(bot), .. } => {
                    self.station.observe_bot(bot);
                    self.station.bot_error = None;
                }
                job::Done::Ok { .. }
                | job::Done::BotOff { .. }
                | job::Done::AddressProbed(_)
                | job::Done::InstallProbed(_)
                | job::Done::AddressChanged
                | job::Done::Removed { .. } => {}
                job::Done::NeedsAdminPassword => {
                    self.station.bot_error =
                        Some(t!("telegram.server.needs_old_admin").to_string());
                }
                job::Done::Failed { reason, .. } => self.station.bot_error = Some(reason),
            }
            return true;
        }
        if kind == Kind::User {
            self.station.finished = self.station.finished.wrapping_add(1);
        }
        let mut transferred = false;
        let mut station_polls = false;
        let mut said_bot = false;
        match done {
            job::Done::BotOff { returned } => {
                self.station.returned = returned;
                self.station_apply_returned();
            }
            job::Done::AddressProbed(change) => {
                self.station.address_change = Some(change);
                self.station.outcome = Some(Ok(t!("telegram.server.address_review").to_string()));
                return true;
            }
            job::Done::InstallProbed(key) => {
                self.station.install_probe = Some(key);
                self.station.outcome = Some(Ok(t!("telegram.server.install_review").to_string()));
                return true;
            }
            job::Done::AddressChanged => {
                self.station_forgotten(Ok(t!("telegram.server.address_changed").to_string()));
                self.station.pending_refresh = true;
                return true;
            }
            job::Done::Removed { local_forget_error } => {
                let outcome = match local_forget_error {
                    Some(reason) => Err(reason),
                    None => Ok(t!("telegram.server.removed").to_string()),
                };
                self.station_forgotten(outcome);
                return true;
            }
            job::Done::Ok {
                transferred: t,
                bot,
                bot_off,
            } => {
                transferred = t;
                // A terminal bot held down after an undo that failed runs again once the station
                // no longer polls its token.
                if bot_off && !handing_over && self.telegram.suspended() {
                    // A different server's job cannot release the marked destination.
                    self.station.pending_refresh = true;
                }
                self.station.needs_old_admin = false;
                if let Some(bot) = bot {
                    self.station.observe_bot(bot);
                    self.station.bot_error = None;
                    said_bot = true;
                }
                if kind == Kind::User {
                    self.station.outcome = Some(Ok(t!("telegram.server.done").to_string()));
                }
            }
            job::Done::NeedsAdminPassword => {
                self.station.needs_old_admin = true;
                self.station.outcome = Some(Err(t!("telegram.server.needs_old_admin").to_string()));
            }
            job::Done::Failed {
                reason,
                station_may_poll,
            } => {
                station_polls = station_may_poll;
                self.station.outcome = Some(Err(if self.station.returned.is_some() {
                    format!("{reason}\n{}", t!("telegram.server.return_remove_failed"))
                } else {
                    reason
                }));
            }
        }
        // An "Apply on the server" keeps its own end beside its buttons, and in the log.
        if std::mem::take(&mut self.station.applying) {
            match &self.station.outcome {
                Some(Err(reason)) => log::warn!("station: apply on the server failed: {reason}"),
                _ => log::info!("station: applied on the server"),
            }
            self.station.applied = self.station.outcome.clone();
        }
        // A job that ended without the bot's state may have changed it: read it again.
        self.station.pending_refresh |= !said_bot;
        if !handing_over {
            return true;
        }
        if transferred {
            if let Err(e) = self.station_erase_local_bot() {
                // The token is still on disk while the station polls it: the terminal's bot stays
                // down for this run, and the user is told why.
                self.station.outcome = Some(Err(format!(
                    "{}: {e:#}",
                    t!("telegram.server.erase_failed")
                )));
            }
        } else if station_polls {
            // The station may still poll the token: the terminal's bot stays down until the bot
            // is taken off the station ("Take the bot off the server"), or the two answer 409.
            if let Some(Err(reason)) = self.station.outcome.take() {
                self.station.outcome = Some(Err(format!(
                    "{reason}\n{}",
                    t!("telegram.server.local_bot_held")
                )));
            }
        } else if self.station_clear_handover().is_ok() {
            self.resume_terminal_telegram();
        } else {
            self.station.outcome = Some(Err(t!("telegram.server.local_bot_held").to_string()));
        }
        true
    }

    /// Save before considering local transport admission; a known station keeps it suspended.
    /// A failed disk save retains the only returned bot in protected memory.
    /// Every retry first checks and removes the current server bot again:
    /// a token may have been installed there since the failed local save.
    fn station_apply_returned(&mut self) {
        let Some(returned) = self.station.returned.take() else {
            if self.telegram.suspended() {
                self.resume_terminal_telegram();
            }
            self.station.outcome = Some(Ok(t!("telegram.server.return_skipped").to_string()));
            return;
        };
        let restored = match save_returned_bot(
            &mut self.config,
            &mut self.preview,
            &returned,
            AppConfig::save_telegram,
        ) {
            Ok(restored) => restored,
            Err(error) => {
                self.station.returned = Some(returned);
                self.station.outcome = Some(Err(format!(
                    "{}: {error:#}",
                    t!("telegram.server.return_save_failed")
                )));
                return;
            }
        };
        self.resume_terminal_telegram();
        if restored {
            self.station.restored = self.station.restored.wrapping_add(1);
        }
        self.station.outcome = Some(Ok(if restored {
            t!("telegram.server.return_done").to_string()
        } else {
            t!("telegram.server.return_skipped").to_string()
        }));
    }

    /// Apply a read without guessing when the station is unreachable or owns a stopped bot.
    fn station_apply_recovery(&mut self, done: job::Done) {
        let job::Done::Ok { bot: Some(bot), .. } = done else {
            self.station.bot_error = Some(t!("telegram.server.local_bot_held").to_string());
            self.station.outcome = Some(Err(t!("telegram.server.local_bot_held").to_string()));
            return;
        };
        let decision = self
            .station
            .pending
            .as_ref()
            .map(|p| recovery::decide(p, &bot));
        self.station.bot_error = None;
        let stopped_with_token = bot.stopped && bot.has_token;
        self.station.observe_bot(bot);
        let result = match decision {
            Some(recovery::Decision::Erase) => self.station_erase_local_bot(),
            Some(recovery::Decision::Resume) => self.station_clear_handover().map(|()| {
                self.resume_terminal_telegram();
            }),
            _ => Err(anyhow::anyhow!("station ownership is unresolved")),
        };
        let message = if stopped_with_token {
            format!(
                "{}\n{}",
                t!("telegram.server.bot_stopped"),
                t!("telegram.server.local_bot_held")
            )
        } else if result.is_err() {
            t!("telegram.server.local_bot_held").to_string()
        } else {
            t!("telegram.server.done").to_string()
        };
        self.station.outcome = Some(if result.is_ok() {
            Ok(message)
        } else {
            Err(message)
        });
    }

    /// The bot runs on the station now: forget its token and chats here, on disk and in an open
    /// Settings draft alike, so a later Save does not bring them back. The draft's other Telegram
    /// fields keep their unsaved edits.
    fn station_erase_local_bot(&mut self) -> anyhow::Result<()> {
        if let Some(pending) = self.station.pending.as_mut() {
            pending.erase_pending = true;
            recovery::save(
                &moon_core::config::paths::station_handover_path(),
                Some(pending),
            )?;
        }
        let mut candidate = self.config.clone();
        forget_bot(&mut candidate.telegram);
        candidate.save_telegram()?;
        self.config = candidate;
        if let Some(draft) = self.preview.as_mut() {
            forget_bot(&mut draft.telegram);
        }
        // The Settings input must forget the token even if clearing the journal then fails.
        self.station.erased = self.station.erased.wrapping_add(1);
        self.station_clear_handover()?;
        self.resume_terminal_telegram();
        Ok(())
    }
}

/// A Telegram configuration without its bot: no token, no paired chat, no access.
fn forget_bot(telegram: &mut moon_core::config::TelegramConfig) {
    telegram.token = moon_core::config::Secret::new(String::new());
    telegram.authorized_chat_ids.clear();
    telegram.owner_chat_id = None;
    telegram.chat_access.clear();
}

/// Whether the station's bot needs the header clock's `wanted` zone: its access was read, it
/// knows the bot's settings (a station that predates them has no live zone), and it reports
/// another zone — after a failed push, a rollback, or a terminal that set its own.
fn zone_push_due(access: Option<&moon_core::station_api::Access>, wanted: &str) -> bool {
    access.is_some_and(|access| access.bot.is_some() && access.zone.as_deref() != Some(wanted))
}

/// Preserve both saved local bots and unsaved Settings tokens; never merge their chats.
fn can_restore_bot(
    saved: &moon_core::config::TelegramConfig,
    draft: Option<&moon_core::config::TelegramConfig>,
) -> bool {
    saved.token.is_empty() && draft.is_none_or(|d| d.token.is_empty())
}

/// Restore exactly the bot fields, keeping the terminal's own Mini App preference.
fn restore_bot(
    telegram: &mut moon_core::config::TelegramConfig,
    returned: &moon_remote::station::bot::ReturnedBot,
) {
    telegram.token = returned.token.clone();
    returned.access.apply_to(telegram);
}

/// Atomically save before publishing bot data to the live config or open draft. A failed save
/// leaves both unchanged; `false` preserves a local bot created while SSH was still running.
fn save_returned_bot(
    saved: &mut AppConfig,
    draft: &mut Option<AppConfig>,
    returned: &moon_remote::station::bot::ReturnedBot,
    save: impl FnOnce(&AppConfig) -> anyhow::Result<()>,
) -> anyhow::Result<bool> {
    if !can_restore_bot(&saved.telegram, draft.as_ref().map(|d| &d.telegram)) {
        return Ok(false);
    }
    let mut candidate = saved.clone();
    restore_bot(&mut candidate.telegram, returned);
    save(&candidate)?;
    *saved = candidate;
    if let Some(draft) = draft {
        restore_bot(&mut draft.telegram, returned);
    }
    Ok(true)
}

#[cfg(test)]
mod tests;
