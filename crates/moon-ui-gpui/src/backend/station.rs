//! The station's server work, shared by the Station and Telegram Settings tabs, owned by the backend.
//!
//! A job runs for minutes and may hand the terminal's bot over; its end must be applied whether
//! or not the Settings window is still open — a lost end would leave the terminal's bot down, or
//! its token on disk while the station polls it. So the jobs, their lines and their ends live
//! here, and the Settings segment only starts jobs and shows this state.
//!
//! One job runs at a time: the user's (a button; it owns the progress lines and the
//! outcome), a quiet read of the station's bot and core listing (on opening the tab; it preserves
//! the user's outcome). A user's job pressed during a quiet read waits for it. A recovery read
//! resolves a persisted hand-over before local polling.
//!
//! Handing the bot over: the terminal's transport is suspended first (`TelegramState::suspend`:
//! one token, one poller) with the saved configuration untouched, so a Save meanwhile writes the
//! token as it was. The station's bot polls (paired, or offering a code when no chat came along) →
//! the token and the chats are erased here, on disk and in an open Settings draft; a failure the
//! station undid → local polling may resume only without a known station; a failure it could not
//! undo → the terminal's bot stays down, and the user is told to take it off the station. Ownership is journaled before
//! suspension and cleared only after the end is safely applied, so quitting cannot release it.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::time::Duration;

use gpui::{Context, Task};
use moon_core::config::AppConfig;
use moon_core::market::trade_replay;
use moon_core::station_api::TapeWindow;
use moon_remote::station::bot::BotState;
use rust_i18n::t;

use crate::Backend;

pub(crate) mod cores_sync;
pub(crate) mod groups_sync;
pub(crate) mod job;
pub(crate) mod pull;
mod recovery;
pub(crate) mod text;
pub(crate) mod uid_translation;

/// How often the backend drains a running job's channel.
const POLL: Duration = Duration::from_millis(150);
/// How many of a job's lines are kept.
const MAX_LINES: usize = 400;

/// Render-time station facts, refreshed by host reads and backend status observations.
static KNOWN: AtomicBool = AtomicBool::new(false);
static TAIL_FINAL: AtomicU64 = AtomicU64::new(u64::MAX);

/// Read station presence without accessing the hosts file.
pub(crate) fn known_cached() -> bool {
    KNOWN.load(Ordering::Acquire)
}

/// Last reported final-tail count; an unread or unsupported count stays distinct from zero.
pub(crate) fn tail_final_cached() -> u64 {
    TAIL_FINAL.load(Ordering::Acquire)
}

/// Who started a job, which decides what its end may change.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Kind {
    /// A button: owns the progress lines and the outcome.
    #[default]
    User,
    /// A bot/core read that preserves the user's outcome.
    Quiet,
    /// An automatic tail-floor write must not retry directly from its own completion.
    Tail,
    /// Automatic shared-window writes preserve user outcomes and latch their payload.
    Window,
    /// The header clock's zone pushed to the station: appends its line, keeps the outcome, and
    /// like a quiet read does not hold the buttons.
    Zone,
    /// Automatic group writes preserve the user's outcome and latch each payload attempt.
    Groups,
    /// A startup read that resolves durable bot ownership.
    Recovery,
    /// A moved core's new address sent: its result goes to that core's move, not to the
    /// station's own outcome.
    CoreMove,
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
    /// The control that started the running job, so the busy lines can name it.
    /// A quiet read and a zone push record one too; [`Self::busy`] is false for them,
    /// so those lines stay hidden. Cleared when the job ends.
    pub(crate) running_label: Option<job::JobButton>,
    /// The last job's end: `Ok(summary)` or `Err(reason)`.
    pub(crate) outcome: Option<Result<String, String>>,
    /// Distinct unmapped core identities reported by this user's payload translation.
    unmapped_cores: usize,
    pub(crate) bot: Option<BotState>,
    /// UTC milliseconds when `bot` was read, so a view can name how old its numbers are.
    pub(crate) bot_read_ms: Option<i64>,
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
    /// Source identities captured with the quarantined station bot, never terminal-numbered grants.
    returned_cores: Vec<moon_core::station_api::ListedCore>,
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
    /// Moved cores whose new address waits for the station, sent one at a time when it is free.
    core_moves: Vec<u64>,
    /// The moved core whose address the running job sends.
    core_move_running: Option<u64>,
    /// The last user job said nothing about the bot: its state is read again after it.
    pending_refresh: bool,
    /// The running job is an "Apply on the server" of the chats and the bot's menu.
    pub(crate) applying: bool,
    /// How the last "Apply on the server" ended, shown beside its buttons; cleared by a revert.
    pub(crate) applied: Option<Result<String, String>>,
    /// The zone the last zone push carried: after it ends the zone is pushed again only when the
    /// header clock moved meanwhile, so a station that refuses it is not asked again at once.
    zone_pushed: Option<String>,
    /// First-contact reconciliation is performed once per configured destination.
    window_reconciled: bool,
    /// Payload and job epoch of the most recent automatic window attempt.
    window_attempt: Option<(TapeWindow, u64)>,
    /// Initial write plus at most one retry caused by its verification reads.
    window_attempts: u8,
    /// Suppress an unchanged automatic failure until its payload or reason changes.
    window_failure: Option<(TapeWindow, String)>,
    /// The next pending quiet refresh was scheduled by a successful window write.
    window_refresh_pending: bool,
    /// The running quiet read verifies a window write rather than unrelated work.
    window_verification: bool,
    /// Advances for every started job, distinguishing a push from unrelated work.
    job_epoch: u64,
    /// Earliest UTC millisecond for another unreconciled station read.
    reconnect_at: Option<i64>,
    /// Last reconnect delay, doubled after failures up to fifteen minutes.
    reconnect_delay_ms: i64,
    /// Destination-scoped automatic group attempts, retained even after failed writes.
    groups_sync: groups_sync::GroupsSync,
    /// Latest tuner tail waiting for an idle station and a capability report.
    min_tail_pending: Option<u32>,
    /// Last automatic floor attempted, used to avoid retrying the same failed write at once.
    min_tail_pushed: Option<u32>,
    /// The startup read was attempted: it is never repeated, whatever it found.
    prefetched: bool,
    task: Option<Task<()>>,
}

impl Kind {
    /// Work the user did not ask for and need not wait on: a user's job pressed meanwhile starts
    /// right after it.
    fn is_background(self) -> bool {
        matches!(
            self,
            Self::Quiet | Self::Tail | Self::Window | Self::Zone | Self::Groups
        )
    }
}

impl StationJobs {
    /// Reconcile a reported window and retain admission state across completions.
    fn window_step(&mut self, local: TapeWindow) -> WindowStep {
        let mut step = window_admit(
            local,
            self.bot.as_ref().and_then(|bot| bot.tape),
            self.window_reconciled,
            self.window_attempt,
            self.job_epoch,
            self.kind,
        );
        let effective = step.adopt.unwrap_or(local);
        if self
            .window_attempt
            .is_some_and(|(payload, _)| payload != effective)
        {
            self.window_attempts = 0;
        }
        if self.window_attempts >= 2 {
            step.push = None;
        }
        self.window_reconciled = step.reconciled;
        if step.reconciled {
            self.reconnect_at = None;
            self.reconnect_delay_ms = 0;
        }
        step
    }

    /// Record the actual start so only unrelated work releases the same-payload latch.
    fn begin_job(&mut self, kind: Kind, job: &job::Job) {
        self.job_epoch = self.job_epoch.wrapping_add(1);
        self.kind = kind;
        self.running = true;
        self.window_verification =
            kind == Kind::Quiet && std::mem::take(&mut self.window_refresh_pending);
        if kind == Kind::Quiet {
            self.pending_refresh = false;
        }
        if kind == Kind::Window
            && let job::Job::Tape { tape, .. } = job
        {
            if self
                .window_attempt
                .is_none_or(|(payload, _)| payload != *tape)
            {
                self.window_attempts = 0;
            }
            self.window_attempts += 1;
            self.window_attempt = Some((*tape, self.job_epoch));
        }
    }

    /// Only non-automatic completions release the window verification budget.
    fn finish_job(&mut self) {
        self.running = false;
        if !matches!(
            self.kind,
            Kind::Window | Kind::Zone | Kind::Tail | Kind::Groups
        ) && !self.window_verification
        {
            self.window_attempts = 0;
        }
        self.window_verification = false;
    }

    /// Refresh a successful window write without replacing the user's last outcome.
    fn apply_window(&mut self, done: job::Done) {
        self.finish_job();
        match done {
            job::Done::Ok { bot, .. } | job::Done::Installed { bot, .. } => {
                self.pending_refresh = true;
                self.window_refresh_pending = true;
                if let Some(bot) = bot {
                    self.observe_bot(bot);
                    self.bot_error = None;
                }
            }
            job::Done::NeedsAdminPassword => {
                self.needs_old_admin = true;
                self.push_line(t!("telegram.server.needs_old_admin").to_string());
            }
            job::Done::Failed { reason, .. } => {
                if let Some((payload, _)) = self.window_attempt {
                    let failure = (payload, reason.clone());
                    if self.window_failure.as_ref() != Some(&failure) {
                        self.push_line(reason);
                    }
                    self.window_failure = Some(failure);
                }
            }
            job::Done::BotOff { .. }
            | job::Done::AddressProbed(_)
            | job::Done::InstallProbed(_)
            | job::Done::AddressChanged
            | job::Done::Removed { .. } => {}
        }
    }

    /// Back off failed reads, keeping answered unsupported stations disarmed until forgotten.
    fn reconnect_after_read(&mut self, now_ms: i64, answered: bool) {
        if self.window_reconciled
            || !self.known
            || (answered && self.bot_error.is_none())
            || self.bot.as_ref().is_some_and(|bot| bot.tape.is_none())
        {
            self.reconnect_at = None;
            self.reconnect_delay_ms = 0;
        } else {
            self.reconnect_delay_ms = (self.reconnect_delay_ms * 2).clamp(60_000, 900_000);
            self.reconnect_at = Some(now_ms.saturating_add(self.reconnect_delay_ms));
        }
    }

    /// Admit one fresh read at its deadline without reusing the startup prefetch latch.
    fn reconnect_read(
        &mut self,
        now_ms: i64,
        target: Option<moon_remote::ssh::Target>,
    ) -> Option<(job::Job, Kind)> {
        if !self.reconnect_due(now_ms) {
            return None;
        }
        let read = self.bot_read(target)?;
        self.reconnect_at = None;
        Some(read)
    }

    /// Keep host-file reads off ordinary coordination ticks until a reconnect is due.
    fn reconnect_due(&self, now_ms: i64) -> bool {
        !self.running
            && self.known
            && !self.window_reconciled
            && self.reconnect_at.is_some_and(|deadline| now_ms >= deadline)
    }

    /// Retry after unrelated work, or when the requested floor changed during the last push.
    fn min_tail_retry_due(&self) -> bool {
        self.kind != Kind::Tail || self.min_tail_pending != self.min_tail_pushed
    }

    /// Retain the latest tail until a status acknowledges it, retrying when idle and supported.
    fn take_min_tail_job(
        &mut self,
        target: Option<moon_remote::ssh::Target>,
        window: TapeWindow,
    ) -> Option<job::Job> {
        if self.running {
            return None;
        }
        let secs = self.min_tail_pending?;
        let bot = self.bot.as_ref()?;
        let catchup = bot.station.as_ref()?.tape_catchup.as_deref()?;
        if catchup.min_tail_s? == secs {
            self.min_tail_pending = None;
            return None;
        }
        min_tail_job(bot, catchup, target?, secs, window)
    }

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

    /// Retain a listing through restart gaps; station_next syncs the newly observed window.
    fn observe_bot(&mut self, bot: BotState) {
        TAIL_FINAL.store(
            bot.station
                .as_ref()
                .and_then(|status| status.tape_catchup.as_ref())
                .and_then(|catchup| catchup.tail_final)
                .map_or(u64::MAX, u64::from),
            Ordering::Release,
        );
        if let Some(status) = &bot.station {
            self.cores_seen = Some(status.cores.clone());
            self.core_uid_high_water = status.core_uid_high_water;
        }
        self.bot = Some(bot);
        self.bot_read_ms = Some(moon_core::util::time::now_unix_ms_i64());
    }

    /// The read that refreshes the station's bot and core listing, and the kind it runs as: a
    /// recovery read toward an outstanding hand-over's destination, else a quiet read of `known`.
    ///
    /// Args:
    ///     known: The configured station, if any (`known_target`).
    ///
    /// Returns:
    ///     `None` while a job runs or when no station is configured and nothing is pending.
    fn bot_read(&self, known: Option<moon_remote::ssh::Target>) -> Option<(job::Job, Kind)> {
        if self.running {
            return None;
        }
        if let Some(pending) = &self.pending {
            return Some((
                job::Job::BotState {
                    target: pending.target(),
                },
                Kind::Recovery,
            ));
        }
        known.map(|target| (job::Job::BotState { target }, Kind::Quiet))
    }

    /// The startup read that warms the Telegram and Station pages before Settings opens.
    ///
    /// Latched: the first call marks the attempt, so later calls (another startup path, a loop
    /// tick) return nothing even when the first found no station or a running job.
    ///
    /// Args:
    ///     known: The configured station, if any (`known_target`).
    ///
    /// Returns:
    ///     The read to start, at most once per process.
    fn prefetch_read(
        &mut self,
        known: Option<moon_remote::ssh::Target>,
    ) -> Option<(job::Job, Kind)> {
        if std::mem::replace(&mut self.prefetched, true) {
            return None;
        }
        self.bot_read(known)
    }

    /// Apply a quiet read's end: bot state and core identities, or the reason the read failed,
    /// while the user's outcome stays untouched; failed capability reads arm reconnect.
    fn apply_quiet(&mut self, done: job::Done) {
        self.finish_job();
        let answered = matches!(
            &done,
            job::Done::Ok { bot: Some(_), .. } | job::Done::Installed { bot: Some(_), .. }
        );
        match done {
            job::Done::Ok { bot: Some(bot), .. } | job::Done::Installed { bot: Some(bot), .. } => {
                self.observe_bot(bot);
                self.bot_error = None;
            }
            job::Done::Ok { .. }
            | job::Done::Installed { .. }
            | job::Done::BotOff { .. }
            | job::Done::AddressProbed(_)
            | job::Done::InstallProbed(_)
            | job::Done::AddressChanged
            | job::Done::Removed { .. } => {}
            job::Done::NeedsAdminPassword => {
                self.bot_error = Some(t!("telegram.server.needs_old_admin").to_string());
            }
            job::Done::Failed { reason, .. } => {
                log::warn!("station quiet bot read failed: {reason}");
                self.bot_error = Some(reason);
            }
        }
        self.reconnect_after_read(moon_core::util::time::now_unix_ms_i64(), answered);
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
        st.window_reconciled = false;
        st.window_attempt = None;
        st.window_attempts = 0;
        st.window_failure = None;
        st.window_refresh_pending = false;
        st.window_verification = false;
        st.job_epoch = 0;
        st.reconnect_at = None;
        st.reconnect_delay_ms = 0;
        st.groups_sync = groups_sync::GroupsSync::default();
        st.applying = false;
        st.applied = None;
        st.waiting = None;
        st.lines.clear();
        st.status = None;
        st.outcome = Some(outcome);
        st.revision = st.revision.wrapping_add(1);
    }
}

/// A first-contact adoption and independently admitted station write.
struct WindowStep {
    adopt: Option<TapeWindow>,
    push: Option<TapeWindow>,
    reconciled: bool,
}

/// Adopt the larger first report, then allow lower user values without immediate self-retry.
fn window_admit(
    local: TapeWindow,
    reported: Option<TapeWindow>,
    reconciled: bool,
    attempt: Option<(TapeWindow, u64)>,
    epoch_now: u64,
    last_kind: Kind,
) -> WindowStep {
    let Some(reported) = reported else {
        return WindowStep {
            adopt: None,
            push: None,
            reconciled,
        };
    };
    let effective = if reconciled {
        local
    } else {
        trade_replay::adopt_larger(local, reported)
    };
    let same_push_end = attempt == Some((effective, epoch_now)) && last_kind == Kind::Window;
    WindowStep {
        adopt: (effective != local).then_some(effective),
        push: (reported != effective && !same_push_end).then_some(effective),
        reconciled: true,
    }
}

/// Build a tail policy update using the local window without reverting it to the station's report.
fn min_tail_job(
    bot: &BotState,
    catchup: &moon_core::station_api::TapeCatchup,
    target: moon_remote::ssh::Target,
    secs: u32,
    window: TapeWindow,
) -> Option<job::Job> {
    // A window report is the capability gate; the payload always uses the shared window.
    bot.tape.as_ref()?;
    Some(job::Job::Tape {
        target,
        tape: window,
        catchup: None,
        keep: Some(moon_core::station_api::TapeKeep {
            max_age_days: catchup.max_age_days?,
            record_emulator: catchup.record_emulator?,
            tuner_only: catchup.tuner_only,
            min_tail_s: Some(secs),
        }),
    })
}

/// The station's server this terminal set up, if any (`remote/hosts.toml`).
pub(crate) fn known_target() -> Option<moon_remote::ssh::Target> {
    let target = read_known_target();
    KNOWN.store(target.is_some(), Ordering::Release);
    target
}

/// Resolve the configured host for actions while refreshing the render cache at the caller.
fn read_known_target() -> Option<moon_remote::ssh::Target> {
    let hosts = moon_remote::hosts::Hosts::load(&moon_remote::hosts::Hosts::path()).ok()?;
    let host = hosts.first_set_up()?;
    let (name, port) = host.addr.rsplit_once(':')?;
    Some(moon_remote::ssh::Target {
        host: name.trim_matches(['[', ']']).to_owned(),
        port: port.parse().ok()?,
    })
}

impl Backend {
    /// Persist and apply the shared window before station work or autoload reads it.
    pub(crate) fn tape_window_set(&mut self, window: TapeWindow, cx: &mut Context<Self>) {
        self.tape_window_apply(window, cx);
        self.station_window_sync(cx);
    }

    /// Apply an adoption before queued jobs without admitting an automatic station write.
    fn tape_window_apply(&mut self, window: TapeWindow, cx: &mut Context<Self>) {
        let old = trade_replay::window();
        let new = trade_replay::normalize(window);
        if old != new {
            moon_core::config::storage::update(|cfg| {
                cfg.trade_replay.margin_s = new.margin_s;
                cfg.trade_replay.long_position_min = new.long_position_min;
            });
            trade_replay::set_margin_s(new.margin_s);
            trade_replay::set_long_position_min(new.long_position_min);
            self.station.revision = self.station.revision.wrapping_add(1);
            crate::analytics::tape_autoload::window_grown(old, new);
            cx.notify();
        }
    }

    /// Reconcile the first report before rebuilding user work, without starting a write.
    fn station_window_adopt(&mut self, cx: &mut Context<Self>) {
        if !self.station.window_reconciled {
            let step = self.station.window_step(trade_replay::window());
            if let Some(adopt) = step.adopt {
                self.tape_window_apply(adopt, cx);
            }
        }
    }

    /// Reconcile once and push idle station differences through the existing tape job.
    pub(crate) fn station_window_sync(&mut self, cx: &mut Context<Self>) {
        let step = self.station.window_step(trade_replay::window());
        if let Some(adopt) = step.adopt {
            self.tape_window_apply(adopt, cx);
        }
        if self.station.running || self.station.waiting.is_some() || self.station.pending_refresh {
            return;
        }
        if let Some(tape) = step.push
            && let Some(target) = known_target()
        {
            self.station_begin(
                job::Job::Tape {
                    target,
                    tape,
                    catchup: None,
                    keep: None,
                },
                false,
                Kind::Window,
                cx,
            );
        }
    }

    /// Retry an unreconciled station at its bounded reconnect deadline on the coordination tick.
    pub(crate) fn station_reconnect_tick(&mut self, now_ms: i64, cx: &mut Context<Self>) {
        if !self.station.reconnect_due(now_ms) {
            return;
        }
        if let Some((job, kind)) = self.station.reconnect_read(now_ms, known_target()) {
            self.station_begin(job, false, kind, cx);
        }
    }

    /// Send changed local groups after a read or edit; running jobs defer admission until idle.
    pub(crate) fn station_groups_sync(&mut self, cx: &mut Context<Self>) {
        let here = cores_sync::trace_cores(&self.config);
        let access = self
            .station
            .bot
            .as_ref()
            .and_then(|bot| bot.access.as_ref());
        let listing = groups_sync::listing(self.station.bot.as_ref());
        let Some(job) = self.station.groups_sync.take_job(
            self.station.running,
            known_target(),
            access,
            &self.config.core_groups,
            &here,
            listing,
        ) else {
            return;
        };
        self.station_begin(job, false, Kind::Groups, cx);
    }

    /// Retain the newest tuner setting across running jobs, then use the station tape path.
    pub(crate) fn station_min_tail_sync(&mut self, secs: u32, cx: &mut Context<Self>) {
        self.station.min_tail_pending = Some(secs);
        self.station_push_min_tail(cx);
    }

    /// Push only after a capability report, preserving the station's acknowledged policy.
    fn station_push_min_tail(&mut self, cx: &mut Context<Self>) {
        let Some(job) = self
            .station
            .take_min_tail_job(known_target(), trade_replay::window())
        else {
            return;
        };
        self.station.min_tail_pushed = self.station.min_tail_pending;
        self.station_begin(job, false, Kind::Tail, cx);
    }

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
        if let Some((job, kind)) = self.station.bot_read(known_target()) {
            self.station_begin(job, false, kind, cx);
        }
    }

    /// Warm the Telegram and Station Settings pages once at startup with the same read their
    /// opening fires, so they show the station's bot, chats, notifications, menu, layout, cores
    /// and tape window at once. Nothing without a configured station; never repeated.
    pub(crate) fn station_prefetch(&mut self, cx: &mut Context<Self>) {
        if let Some((job, kind)) = self.station.prefetch_read(known_target()) {
            self.station_begin(job, false, kind, cx);
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

    /// Rebuild queued tape values and publish hand-over ownership before starting remote work.
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
        let job = match job.with_current_tape_window() {
            job::Job::BotOff { target, .. } => {
                // A queued job decides from the current saved and draft state when it starts.
                let restore = can_restore_bot(
                    &self.config.telegram,
                    self.preview.as_ref().map(|p| &p.telegram),
                );
                job::Job::BotOff {
                    target,
                    restore,
                    recovered: self
                        .station
                        .returned
                        .clone()
                        .map(|returned| (returned, self.station.returned_cores.clone())),
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
        let has_token = self.station.bot.as_ref().is_some_and(|bot| bot.has_token);
        let running_label = job::JobButton::of(&job, has_token);
        let st = &mut self.station;
        st.begin_job(kind, &job);
        st.running_label = Some(running_label);
        st.applying = applying;
        st.cores_job = matches!(job, job::Job::Cores { .. } | job::Job::CoresRemove { .. });
        if st.cores_job {
            st.cores_result = None;
        }
        if applying {
            st.applied = None;
        }
        st.handing_over = hand_over;
        match kind {
            Kind::User => {
                st.address_change = None;
                st.install_probe = None;
                st.outcome = None;
                st.unmapped_cores = 0;
                st.lines.clear();
                st.status = None;
            }
            Kind::Quiet
            | Kind::Tail
            | Kind::Window
            | Kind::Zone
            | Kind::Groups
            | Kind::Recovery
            | Kind::CoreMove => {}
        }
        st.revision = st.revision.wrapping_add(1);
        let rx = if kind == Kind::Groups {
            job::start_groups_sync(job, cores_sync::trace_cores(&self.config))
        } else {
            job::start(job, cores_sync::trace_cores(&self.config))
        };
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

    /// Have the station follow a core moved to a new address, behind whatever job runs now. A
    /// terminal with no station of its own tells the move so and sends nothing.
    pub(crate) fn station_follow_core(&mut self, uid: u64, cx: &mut Context<Self>) {
        let Some(target) = known_target() else {
            self.core_move_note(uid, t!("core_move.station.none").to_string());
            return;
        };
        if self.station.running {
            if !self.station.core_moves.contains(&uid) {
                self.station.core_moves.push(uid);
            }
            return;
        }
        self.station.core_move_running = Some(uid);
        self.station_begin(job::Job::CoreKey { target, uid }, false, Kind::CoreMove, cx);
    }

    /// After a job, drain user work and refreshes before reconciling the observed shared window.
    fn station_next(&mut self, cx: &mut Context<Self>) {
        // Adopt the first report before rebuilding queued tape work; user work and refreshes
        // still take priority over automatic pushes.
        self.station_window_adopt(cx);
        if let Some((job, hand_over)) = self.station.waiting.take() {
            self.station_begin(job, hand_over, Kind::User, cx);
        }
        if !self.station.running && !self.station.core_moves.is_empty() {
            let uid = self.station.core_moves.remove(0);
            self.station_follow_core(uid, cx);
        }
        if !self.station.running && std::mem::take(&mut self.station.pending_refresh) {
            self.station_refresh_bot(cx);
        }
        self.station_window_sync(cx);
        if self.station.min_tail_retry_due() {
            self.station_push_min_tail(cx);
        }
        // Last: with the bot's state as now read, its zone may lag the header clock's. After a push
        // itself only when the header clock moved during it: the same zone refused would be sent
        // again at once, in a loop.
        let wanted = moon_core::util::display_time::zone_or_utc(self.header_clock_zone()).name();
        if self.station.kind != Kind::Zone || self.station.zone_pushed.as_deref() != Some(wanted) {
            self.station_zone_sync(cx);
        }
        self.station_groups_sync(cx);
    }

    /// Apply one job event; `true` once the job has ended.
    fn station_event(&mut self, event: job::Event) -> bool {
        let done = match event {
            job::Event::UnmappedCores(count) => {
                self.station.unmapped_cores = count;
                if self.station.kind == Kind::Groups {
                    self.station.groups_sync.translation_seen(count);
                }
                return false;
            }
            job::Event::Returned { returned, cores } => {
                self.station.returned = Some(returned);
                self.station.returned_cores = cores;
                return false;
            }
            job::Event::Line(line) => {
                match self.station.core_move_running {
                    Some(uid) if self.station.kind == Kind::CoreMove => {
                        self.core_move_note(uid, line)
                    }
                    _ => self.station.push_line(line),
                }
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
        if !matches!(kind, Kind::Quiet | Kind::Tail | Kind::Window) {
            self.station.finish_job();
        }
        self.station.running_label = None;
        self.station.cores_job = false;
        if kind == Kind::Recovery {
            let answered = matches!(
                &done,
                job::Done::Ok { bot: Some(_), .. } | job::Done::Installed { bot: Some(_), .. }
            );
            self.station_apply_recovery(done);
            self.station
                .reconnect_after_read(moon_core::util::time::now_unix_ms_i64(), answered);
            return true;
        }
        if kind == Kind::Window {
            self.station.apply_window(done);
            return true;
        }
        if kind == Kind::CoreMove {
            // The job's own lines say what it did; only a failure needs one more.
            if let Some(uid) = self.station.core_move_running.take()
                && let job::Done::Failed { reason, .. } = done
            {
                self.core_move_note(
                    uid,
                    t!("core_move.station.failed", reason = reason).to_string(),
                );
            }
            return true;
        }
        if kind == Kind::Zone || kind == Kind::Groups {
            // Background writes refresh bot state and identities without replacing user outcomes.
            match done {
                job::Done::Ok { bot, .. } | job::Done::Installed { bot, .. } => {
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
        if kind == Kind::Quiet || kind == Kind::Tail {
            // A quiet read refreshes bot state and core identities while preserving the user's
            // outcome; a failed read records its error instead.
            if kind == Kind::Tail
                && matches!(&done, job::Done::Ok { .. } | job::Done::Installed { .. })
            {
                self.station.pending_refresh = true;
            }
            self.station.apply_quiet(done);
            return true;
        }
        if kind == Kind::User {
            self.station.finished = self.station.finished.wrapping_add(1);
        }
        let mut transferred = false;
        let mut station_polls = false;
        let mut said_bot = false;
        let completion = match &done {
            job::Done::Installed {
                configured,
                skipped,
                ..
            } => text::install_result(*configured, skipped),
            _ => t!("telegram.server.done").to_string(),
        };
        match done {
            job::Done::BotOff { returned, cores } => {
                self.station.returned = returned;
                self.station.returned_cores = cores;
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
            }
            | job::Done::Installed {
                transferred: t,
                bot,
                bot_off,
                ..
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
                    self.station.outcome = Some(Ok(completion));
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
            self.station_append_unmapped();
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
        self.station_append_unmapped();
        true
    }

    /// Keep a typed omission count visible beside the action's final result in both Settings tabs.
    fn station_append_unmapped(&mut self) {
        if self.station.unmapped_cores > 0
            && let Some(outcome) = self.station.outcome.as_mut()
        {
            let text = match outcome {
                Ok(text) | Err(text) => text,
            };
            text.push('\n');
            text.push_str(&t!(
                "telegram.server.cores_unmapped",
                n = self.station.unmapped_cores
            ));
        }
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
        let (translated, unmapped) =
            returned_to_terminal(&self.config, &returned, &self.station.returned_cores);
        let restored = match save_returned_bot(
            &mut self.config,
            &mut self.preview,
            &translated,
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
            self.station.unmapped_cores = unmapped;
            self.station.restored = self.station.restored.wrapping_add(1);
        }
        self.station.returned_cores.clear();
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

/// Translate a clone at save time so failed saves retain the raw station grants for a safe retry.
fn returned_to_terminal(
    config: &AppConfig,
    returned: &moon_remote::station::bot::ReturnedBot,
    listing: &[moon_core::station_api::ListedCore],
) -> (moon_remote::station::bot::ReturnedBot, usize) {
    let here = cores_sync::trace_cores(config);
    let mut translation = uid_translation::UidTranslation::new(&here, listing);
    let mut translated = returned.clone();
    translation.access(&mut translated.access, true);
    (translated, translation.unmapped())
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
