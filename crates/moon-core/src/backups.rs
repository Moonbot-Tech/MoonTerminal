//! Shared daily scheduling for settings and strategy backup domains.
//!
//! One wall-clock coordinator owns the 12:00 UTC deadline and wakes independent domain jobs.
//! Settings never wait for strategy synchronization, while a fresh strategy database becomes
//! eligible only after every expected live core commits one complete set.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, LazyLock, Mutex, MutexGuard};
use std::time::Duration;

use crate::config::AppConfig;
use crate::util::now_unix_ms_i64;

/// Milliseconds in one UTC day.
pub(crate) const DAY_MS: i64 = 86_400_000;
/// Number of UTC-noon periods retained by every daily backup domain.
pub(crate) const RETAIN_PERIODS: i64 = 7;
/// Delay before retrying a source or I/O failure inside one domain job.
const RETRY_DELAY: Duration = Duration::from_secs(5 * 60);
/// UTC wall-clock offset of the daily backup deadline.
pub(crate) const NOON_MS: i64 = 12 * 60 * 60 * 1_000;

/// Result of bringing one domain's latest due slot current.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum DueOutcome {
    /// The canonical slot already held a completed domain snapshot.
    Current(PathBuf),
    /// This attempt published the canonical slot.
    Created(PathBuf),
    /// The domain has no source yet and should retry later.
    SourceMissing,
    /// Eligibility changed while the snapshot was assembled; a later wake owns the retry.
    Pending,
}

/// Mutable predicate protected by the coordinator mutex.
#[derive(Default)]
struct ScheduleState {
    /// Whether normal startup authorized persistent backup work.
    requested: bool,
    /// Whether the process-lifetime scheduler thread claimed its launch.
    worker_started: bool,
    /// Revision used to distinguish a real wake from a spurious condition-variable wake.
    wake_revision: u64,
    /// Active strategy-feed cores whose first complete commits are required on a fresh database.
    expected_cores: HashSet<u64>,
    /// Cores that have committed at least one complete set in this process.
    committed_cores: HashSet<u64>,
    /// Whether the database already contained strategy rows before session startup.
    inherited_strategy_data: bool,
    /// Whether the strategy domain may publish its scheduled slot.
    strategy_ready: bool,
    /// Changes whenever runtime topology alters the expected strategy-core set.
    topology_generation: u64,
}

/// Process-global schedule state and its readiness wake.
struct ScheduleCoordinator {
    state: Mutex<ScheduleState>,
    wake: Condvar,
}

impl ScheduleCoordinator {
    /// Create an inert coordinator for production or an isolated unit test.
    fn new() -> Self {
        Self {
            state: Mutex::new(ScheduleState::default()),
            wake: Condvar::new(),
        }
    }

    /// Lock state while recovering a poisoned mutex instead of disabling backups permanently.
    fn lock(&self) -> MutexGuard<'_, ScheduleState> {
        self.state.lock().unwrap_or_else(|error| error.into_inner())
    }

    /// Authorize normal startup, install its expected cores, and claim the scheduler launch.
    fn request(&self, expected: HashSet<u64>, inherited_strategy_data: bool) -> bool {
        let mut state = self.lock();
        state.requested = true;
        state.inherited_strategy_data = inherited_strategy_data;
        update_expected_locked(&mut state, expected);
        let launch = !state.worker_started;
        state.worker_started = true;
        state.wake_revision = state.wake_revision.wrapping_add(1);
        drop(state);
        self.wake.notify_all();
        launch
    }

    /// Reconcile runtime strategy-feed topology and wake a newly eligible strategy job.
    fn update_expected(&self, expected: HashSet<u64>) {
        let mut state = self.lock();
        if state.expected_cores == expected {
            return;
        }
        update_expected_locked(&mut state, expected);
        state.wake_revision = state.wake_revision.wrapping_add(1);
        drop(state);
        self.wake.notify_all();
    }

    /// Record one committed complete set and wake the scheduler when readiness changes.
    fn strategy_commit(&self, core_uid: u64) {
        let mut state = self.lock();
        state.committed_cores.insert(core_uid);
        recompute_strategy_ready(&mut state);
        state.wake_revision = state.wake_revision.wrapping_add(1);
        drop(state);
        self.wake.notify_all();
    }

    /// Return current strategy eligibility and the topology generation it belongs to.
    fn strategy_claim(&self) -> Option<u64> {
        let state = self.lock();
        (state.requested && state.strategy_ready).then_some(state.topology_generation)
    }

    /// Publish one final strategy result only while its ready topology remains current.
    fn with_current_topology<T>(
        &self,
        generation: u64,
        operation: impl FnOnce() -> T,
    ) -> Option<T> {
        let state = self.lock();
        (state.requested && state.strategy_ready && state.topology_generation == generation)
            .then(operation)
    }

    /// Release a failed scheduler-thread launch for a later explicit request.
    fn release_worker(&self) {
        self.lock().worker_started = false;
    }
}

/// Replace the expected set and recompute readiness under the coordinator lock.
fn update_expected_locked(state: &mut ScheduleState, expected: HashSet<u64>) {
    state.expected_cores = expected;
    state.topology_generation = state.topology_generation.wrapping_add(1);
    recompute_strategy_ready(state);
}

/// Derive fresh-database readiness from inherited data or every expected core's first commit.
fn recompute_strategy_ready(state: &mut ScheduleState) {
    state.strategy_ready = state.inherited_strategy_data
        || (!state.expected_cores.is_empty()
            && state.expected_cores.is_subset(&state.committed_cores));
}

/// Global coordinator used only after the UI explicitly authorizes a normal run.
static COORDINATOR: LazyLock<ScheduleCoordinator> = LazyLock::new(ScheduleCoordinator::new);
/// Prevent overlapping settings jobs when a deadline and readiness wake arrive together.
static SETTINGS_JOB_ACTIVE: AtomicBool = AtomicBool::new(false);
/// Prevent overlapping strategy jobs while a large SQLite copy is still running.
static STRATEGY_JOB_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Return the most recent 12:00 UTC slot at or before `now_ms`.
pub(crate) fn due_slot_ms(now_ms: i64) -> i64 {
    let day = now_ms.div_euclid(DAY_MS) * DAY_MS;
    let noon = day + NOON_MS;
    if now_ms >= noon { noon } else { noon - DAY_MS }
}

/// Compute the wall-clock delay from `now_ms` to the next 12:00 UTC occurrence.
fn delay_to_next_noon(now_ms: i64) -> Duration {
    let day = now_ms.div_euclid(DAY_MS) * DAY_MS;
    let today_noon = day + NOON_MS;
    let next = if now_ms < today_noon {
        today_noon
    } else {
        today_noon + DAY_MS
    };
    Duration::from_millis(next.saturating_sub(now_ms) as u64)
}

/// Return whether work crossed into a newer canonical daily slot.
fn daily_slot_advanced(started_ms: i64, finished_ms: i64) -> bool {
    due_slot_ms(started_ms) != due_slot_ms(finished_ms)
}

/// Return the strategy-feed cores expected to populate a complete fresh database.
fn expected_strategy_cores(config: &AppConfig) -> HashSet<u64> {
    config
        .servers
        .iter()
        .filter(|server| {
            server.active
                && !server.synthetic
                && server.feed.strategies
                && config.group(&server.group).active
        })
        .map(|server| server.uid)
        .collect()
}

/// Start the unified daily scheduler for a normal, persistence-enabled application run.
///
/// The caller must invoke this before sessions can initialize a fresh strategy database. FireTest
/// must not call it. Settings become eligible immediately; strategy readiness is derived from a
/// read-only existing-data probe or later complete-set commits.
pub fn start_daily(config: &AppConfig) {
    let expected = expected_strategy_cores(config);
    let inherited = crate::strat_db::backup::source_has_strategy_rows();
    if !COORDINATOR.request(expected, inherited) {
        return;
    }
    if let Err(error) = std::thread::Builder::new()
        .name("backup-schedule".into())
        .spawn(schedule_loop)
    {
        COORDINATOR.release_worker();
        log::error!("failed to start daily backup scheduler: {error}");
    }
}

/// Update strategy readiness after a Settings save changes active session topology.
pub fn update_daily_sources(config: &AppConfig) {
    COORDINATOR.update_expected(expected_strategy_cores(config));
}

/// Notify the coordinator after one core's complete strategy set commits successfully.
pub(crate) fn notify_strategy_commit(core_uid: u64) {
    COORDINATOR.strategy_commit(core_uid);
}

/// Execute the final strategy publication while topology updates are excluded.
pub(crate) fn with_current_strategy_topology<T>(
    generation: u64,
    operation: impl FnOnce() -> T,
) -> Option<T> {
    COORDINATOR.with_current_topology(generation, operation)
}

/// Run wall-clock deadlines and readiness wakes without performing domain I/O under the mutex.
fn schedule_loop() {
    loop {
        launch_settings_job();
        if let Some(generation) = COORDINATOR.strategy_claim() {
            launch_strategy_job(generation);
        }

        let mut state = COORDINATOR.lock();
        let observed = state.wake_revision;
        while state.wake_revision == observed {
            let delay = delay_to_next_noon(now_unix_ms_i64());
            let (next, timeout) = COORDINATOR
                .wake
                .wait_timeout(state, delay)
                .unwrap_or_else(|error| error.into_inner());
            state = next;
            if timeout.timed_out() {
                break;
            }
        }
    }
}

/// Launch one settings job unless another deadline is still servicing that domain.
fn launch_settings_job() {
    launch_domain_job(
        "settings-backup",
        "settings",
        &SETTINGS_JOB_ACTIVE,
        false,
        |now_ms| crate::config::backup_due_at(now_ms),
    );
}

/// Launch one strategy job tied to the topology generation that made it eligible.
fn launch_strategy_job(generation: u64) {
    launch_domain_job(
        "strategy-backup",
        "strategy",
        &STRATEGY_JOB_ACTIVE,
        true,
        move |now_ms| crate::strat_db::backup::backup_due_at(now_ms, generation),
    );
}

/// Spawn an independent domain job that retries source and I/O failures without blocking peers.
fn launch_domain_job(
    thread_name: &'static str,
    domain: &'static str,
    active: &'static AtomicBool,
    wake_on_pending: bool,
    operation: impl Fn(i64) -> anyhow::Result<DueOutcome> + Send + 'static,
) {
    if active
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name(thread_name.into())
        .spawn(move || {
            let mut pending = false;
            let started_ms = now_unix_ms_i64();
            let mut attempt_ms = started_ms;
            loop {
                match operation(attempt_ms) {
                    Ok(DueOutcome::Created(path)) => {
                        log::info!("{domain} backup created: {}", path.display());
                        break;
                    }
                    Ok(DueOutcome::Current(_)) => break,
                    Ok(DueOutcome::Pending) => {
                        pending = true;
                        break;
                    }
                    Ok(DueOutcome::SourceMissing) => std::thread::sleep(RETRY_DELAY),
                    Err(error) => {
                        log::warn!("scheduled {domain} backup failed: {error:#}");
                        std::thread::sleep(RETRY_DELAY);
                    }
                }
                attempt_ms = now_unix_ms_i64();
            }
            active.store(false, Ordering::Release);
            if (wake_on_pending && pending) || daily_slot_advanced(started_ms, now_unix_ms_i64()) {
                let mut state = COORDINATOR.lock();
                state.wake_revision = state.wake_revision.wrapping_add(1);
                drop(state);
                COORDINATOR.wake.notify_all();
            }
        });
    if let Err(error) = spawned {
        active.store(false, Ordering::Release);
        log::error!("failed to start {domain} backup job: {error}");
    }
}

#[cfg(test)]
mod tests;
