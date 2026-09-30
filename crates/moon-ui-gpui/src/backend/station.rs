//! The station's server work (Settings -> Telegram -> the bot's segment), owned by the backend.
//!
//! A job runs for minutes and may hand the terminal's bot over; its end must be applied whether
//! or not the Settings window is still open — a lost end would leave the terminal's bot down, or
//! its token on disk while the station polls it. So the jobs, their lines and their ends live
//! here, and the Settings segment only starts jobs and shows this state.
//!
//! One job runs at a time: the user's (a button; it owns the progress lines and the
//! outcome), a quiet read of the station's bot (on opening the tab; it touches only the bot's
//! state), and the automatic core keys after a Save changed the cores (its lines append, the last
//! outcome stays). A user's job pressed during a quiet read waits for it; cores changed during any
//! job go right after it. A recovery read resolves a persisted hand-over before local polling.
//!
//! Handing the bot over: the terminal's transport is suspended first (`TelegramState::suspend`:
//! one token, one poller) with the saved configuration untouched, so a Save meanwhile writes the
//! token as it was. The station's bot polls (paired, or offering a code when no chat came along) →
//! the token and the chats are erased here, on disk and in an open Settings draft; a failure the
//! station undid → the terminal's bot resumes; a failure it could not undo → the terminal's bot
//! stays down, and the user is told to take the bot off the station. Ownership is journaled before
//! suspension and cleared only after the end is safely applied, so quitting cannot release it.

use std::sync::mpsc;
use std::time::Duration;

use gpui::{Context, Task};
use moon_core::config::AppConfig;
use moon_remote::station::bot::BotState;
use rust_i18n::t;

use crate::Backend;

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
    /// The core keys after a Save: appends its lines, keeps the outcome unless it fails.
    Auto,
    /// A startup read that resolves durable bot ownership.
    Recovery,
}

/// The station jobs' state, as the Settings segment shows it.
#[derive(Default)]
pub(crate) struct StationJobs {
    pub(crate) lines: Vec<String>,
    /// Bumped per line, so a view follows the newest one even once the list is full.
    pub(crate) line_seq: u64,
    pub(crate) running: bool,
    /// The last job's end: `Ok(summary)` or `Err(reason)`.
    pub(crate) outcome: Option<Result<String, String>>,
    pub(crate) bot: Option<BotState>,
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
    /// A user's job pressed while a quiet read ran: it starts right after.
    waiting: Option<(job::Job, bool)>,
    /// The terminal's cores changed while a job ran: their keys go to the station after it.
    pending_cores: bool,
    /// The last user job said nothing about the bot: its state is read again after it.
    pending_refresh: bool,
    task: Option<Task<()>>,
}

impl StationJobs {
    /// Keep the retry action available while removal or the local save still needs recovery.
    pub(crate) fn has_returned_bot(&self) -> bool {
        self.returned.is_some()
    }

    /// A job the user sees as work in progress: buttons wait for it. A quiet read does not count.
    pub(crate) fn busy(&self) -> bool {
        self.running && self.kind != Kind::Quiet
    }

    /// Load ownership before the saved Telegram configuration can start transport.
    pub(crate) fn load() -> Self {
        match recovery::load(&moon_core::config::paths::station_handover_path()) {
            Ok(pending) => Self {
                pending,
                ..Self::default()
            },
            Err(_) => Self {
                journal_unreadable: true,
                ..Self::default()
            },
        }
    }

    /// An outstanding or unreadable journal forbids starting the saved local token.
    pub(crate) fn holds_bot(&self) -> bool {
        self.pending.is_some() || self.journal_unreadable
    }

    /// Append a bounded progress line and advance the view cursor.
    fn push_line(&mut self, line: String) {
        self.lines.push(line);
        self.line_seq = self.line_seq.wrapping_add(1);
        if self.lines.len() > MAX_LINES {
            self.lines.drain(..self.lines.len() - MAX_LINES);
        }
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
    /// quiet read it waits for that read; during another job it is not started (the buttons wait).
    pub(crate) fn station_start(&mut self, job: job::Job, hand_over: bool, cx: &mut Context<Self>) {
        if self.station.running {
            if self.station.kind == Kind::Quiet {
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

    /// The terminal's cores changed (added, removed, switched on or off, renamed, a key or
    /// transport replaced): every active core's key goes to the station — now, or right after the
    /// job that runs.
    pub(crate) fn station_cores_changed(&mut self, cx: &mut Context<Self>) {
        if known_target().is_none() {
            return;
        }
        if self.station.running {
            self.station.pending_cores = true;
            return;
        }
        self.station_send_cores(cx);
    }

    /// The server was forgotten: nothing learnt about it applies to the next one.
    pub(crate) fn station_forgotten(&mut self, outcome: Result<String, String>) {
        let st = &mut self.station;
        st.bot = None;
        st.bot_error = None;
        st.needs_old_admin = false;
        st.pending_cores = false;
        st.pending_refresh = false;
        st.waiting = None;
        st.lines.clear();
        st.outcome = Some(outcome);
        st.revision = st.revision.wrapping_add(1);
    }

    fn station_send_cores(&mut self, cx: &mut Context<Self>) {
        self.station.pending_cores = false;
        let Some(target) = known_target() else {
            return;
        };
        let cores: Vec<u64> = self
            .config
            .servers
            .iter()
            .filter(|s| s.active && s.uid != 0)
            .map(|s| s.uid)
            .collect();
        if cores.is_empty() {
            // A station without cores would still be a station; the keys it has stay until one
            // is switched on again.
            self.station
                .push_line(t!("telegram.server.no_active_cores").to_string());
            self.station.revision = self.station.revision.wrapping_add(1);
            return;
        }
        self.station_begin(job::Job::Cores { target, cores }, false, Kind::Auto, cx);
    }

    /// Show a held-bot outcome and quietly reconcile the original hand-over destination.
    pub(crate) fn station_recover(&mut self, cx: &mut Context<Self>) {
        if self.station.holds_bot() {
            self.station.outcome = Some(Err(t!("telegram.server.local_bot_held").to_string()));
            if self.station.pending.is_some() {
                self.station_refresh_bot(cx);
            }
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
        let st = &mut self.station;
        st.running = true;
        st.kind = kind;
        st.handing_over = hand_over;
        match kind {
            Kind::User => {
                st.outcome = None;
                st.lines.clear();
            }
            Kind::Auto => st.push_line(t!("telegram.server.auto_cores").to_string()),
            Kind::Quiet | Kind::Recovery => {}
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

    /// After a job: the user's job that waited, then the cores that changed, then a read of the
    /// bot when the last job said nothing about it.
    fn station_next(&mut self, cx: &mut Context<Self>) {
        if let Some((job, hand_over)) = self.station.waiting.take() {
            self.station_begin(job, hand_over, Kind::User, cx);
        } else if self.station.pending_cores {
            self.station_send_cores(cx);
        }
        // Also after cores that had nothing to send: nothing else would run the read.
        if !self.station.running && std::mem::take(&mut self.station.pending_refresh) {
            self.station_refresh_bot(cx);
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
            job::Event::Done(done) => done,
        };
        let handing_over = std::mem::take(&mut self.station.handing_over);
        let kind = self.station.kind;
        self.station.running = false;
        if kind == Kind::Recovery {
            self.station_apply_recovery(done);
            return true;
        }
        if kind == Kind::Quiet {
            // A quiet read changes only the bot's state, or says why it could not read it.
            match done {
                job::Done::Ok { bot: Some(bot), .. } => {
                    self.station.bot = Some(bot);
                    self.station.bot_error = None;
                }
                job::Done::Ok { .. } | job::Done::BotOff { .. } => {}
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
                    self.station.bot = Some(bot);
                    self.station.bot_error = None;
                    said_bot = true;
                }
                match kind {
                    Kind::User => {
                        self.station.outcome = Some(Ok(t!("telegram.server.done").to_string()))
                    }
                    _ => self
                        .station
                        .push_line(t!("telegram.server.auto_cores_done").to_string()),
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
            self.telegram.resume(&self.config.telegram);
        } else {
            self.station.outcome = Some(Err(t!("telegram.server.local_bot_held").to_string()));
        }
        true
    }

    /// Save before resuming the transport; a failed disk save retains the only returned bot in
    /// protected memory. Every retry first checks and removes the current server bot again:
    /// a token may have been installed there since the failed local save.
    fn station_apply_returned(&mut self) {
        let Some(returned) = self.station.returned.take() else {
            if self.telegram.suspended() {
                self.telegram.resume(&self.config.telegram);
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
        self.telegram.resume(&self.config.telegram);
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
        self.station.bot = Some(bot);
        let result = match decision {
            Some(recovery::Decision::Erase) => self.station_erase_local_bot(),
            Some(recovery::Decision::Resume) => self.station_clear_handover().map(|()| {
                self.telegram.resume(&self.config.telegram);
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
        self.telegram.resume(&self.config.telegram);
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

/// Whether the cores the station would get differ: one added or removed, switched on or off,
/// renamed, or its key or transport replaced. Compared in place, so no key is copied out of the
/// configuration.
pub(crate) fn cores_differ(before: &AppConfig, after: &AppConfig) -> bool {
    fn active(c: &AppConfig) -> Vec<&moon_core::config::ServerConfig> {
        c.servers
            .iter()
            .filter(|s| s.active && s.uid != 0)
            .collect()
    }
    let (a, b) = (active(before), active(after));
    a.len() != b.len()
        || a.iter().zip(&b).any(|(x, y)| {
            x.uid != y.uid
                || x.name != y.name
                || x.transport != y.transport
                || x.key.expose() != y.key.expose()
        })
}

#[cfg(test)]
mod tests;
