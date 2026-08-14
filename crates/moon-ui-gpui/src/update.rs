//! Process-wide update state and the acknowledged Windows replacement helper.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, anyhow, bail};
use gpui::*;
use moon_core::config::paths;
use moon_core::update::{
    AvailableRelease, BuildIdentity, DiscoveryError, DiscoveryRetry, GitHubReleaseClient,
    ReleaseDiscovery, ReleaseVersion, UpdateEligibility,
};
use moon_core::util::time::now_unix_secs;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[cfg(test)]
mod tests;

const MANIFEST_VERSION: u32 = 2;
const HELPER_READY_TIMEOUT: Duration = Duration::from_secs(15);
const HELPER_COMMIT_TIMEOUT: Duration = Duration::from_secs(15);
const STARTED_TIMEOUT: Duration = Duration::from_secs(30);
const HEALTHY_TIMEOUT: Duration = Duration::from_secs(90);
const POLL_WINDOW_SECONDS: u64 = 30 * 60;
const STARTUP_POLL_GAP_SECONDS: u64 = 5 * 60;
const SERVER_DEADLINE_MARGIN_SECONDS: u64 = 60;
const RATE_BACKOFF_SECONDS: [u64; 4] = [60 * 60, 2 * 60 * 60, 4 * 60 * 60, 8 * 60 * 60];
const TRANSIENT_BACKOFF_SECONDS: [u64; 4] = [30 * 60, 60 * 60, 2 * 60 * 60, 4 * 60 * 60];
const PROTOCOL_BACKOFF_SECONDS: u64 = 8 * 60 * 60;

/// Result of parsing the hidden updater process modes before GPUI starts.
pub(crate) enum ProcessDispatch {
    /// Continue normal application startup with an optional update receipt.
    Run(Option<StartupUpdate>),
    /// A helper transaction completed and the process must not create a UI.
    Exit,
}

/// Opaque receipt used for pre-storage update acceptance and a post-boot recovery notice.
#[derive(Clone)]
pub(crate) struct StartupUpdate {
    manifest_path: PathBuf,
    nonce: String,
    recovered: bool,
}

impl StartupUpdate {
    /// Return whether boot should show the one-time recovery notification.
    pub(crate) fn recovered(&self) -> bool {
        self.recovered
    }
}

/// User-visible process-wide update state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum UpdateState {
    /// No trustworthy update is currently available.
    Hidden,
    /// A newer stable release can be installed.
    Available(ReleaseVersion),
    /// The update is downloading and preparing its helper.
    Installing(ReleaseVersion),
    /// The helper acknowledged the parent and normal quit is beginning.
    Restarting(ReleaseVersion),
    /// A clicked installation failed while the current app remains viable.
    Failed {
        version: ReleaseVersion,
        message: String,
    },
}

impl UpdateState {
    /// Return whether the header button should exist.
    pub(crate) fn visible(&self) -> bool {
        !matches!(self, Self::Hidden)
    }

    /// Return whether a click may begin or retry installation.
    pub(crate) fn clickable(&self) -> bool {
        matches!(self, Self::Available(_) | Self::Failed { .. })
    }

    /// Return whether the button should show a busy treatment.
    pub(crate) fn busy(&self) -> bool {
        matches!(self, Self::Installing(_) | Self::Restarting(_))
    }

    /// Return the release version carried by every visible state.
    pub(crate) fn version(&self) -> Option<ReleaseVersion> {
        match self {
            Self::Hidden => None,
            Self::Available(version) | Self::Installing(version) | Self::Restarting(version) => {
                Some(*version)
            }
            Self::Failed { version, .. } => Some(*version),
        }
    }
}

/// Task-local wall-clock and failure state for the one process-wide discovery loop.
struct PollSchedule {
    phase_seconds: u64,
    last_failure: Option<DiscoveryRetry>,
    failure_streak: usize,
}

impl PollSchedule {
    /// Create a schedule whose stable phase spreads processes across each UTC half-hour.
    ///
    /// Args:
    ///     phase_seconds: Process-local offset within the half-hour window.
    ///
    /// Returns:
    ///     A schedule with no recorded failures.
    fn new(phase_seconds: u64) -> Self {
        Self {
            phase_seconds: phase_seconds % POLL_WINDOW_SECONDS,
            last_failure: None,
            failure_streak: 0,
        }
    }

    /// Reset failures and choose the next anchored success deadline.
    ///
    /// Args:
    ///     now_unix: Completion time of the successful scan.
    ///     attempt_unix: Start time used to enforce the startup minimum gap.
    ///     server_not_before: Optional GitHub reset deadline.
    ///
    /// Returns:
    ///     The later safe UTC deadline for the next scan.
    fn after_success(
        &mut self,
        now_unix: u64,
        attempt_unix: u64,
        server_not_before: Option<u64>,
    ) -> u64 {
        self.last_failure = None;
        self.failure_streak = 0;
        later_unix(
            next_regular_poll(now_unix, attempt_unix, self.phase_seconds),
            server_not_before.map(|deadline| self.server_deadline(deadline)),
        )
    }

    /// Advance the exact local backoff class and preserve a later GitHub deadline.
    ///
    /// Args:
    ///     now_unix: Completion time of the failed scan.
    ///     error: Typed discovery failure with optional server authority.
    ///
    /// Returns:
    ///     The later safe retry deadline.
    fn after_failure(&mut self, now_unix: u64, error: &DiscoveryError) -> u64 {
        self.after_failure_hint(now_unix, error.retry(), error.not_before_unix())
    }

    /// Advance a fixed failure hint without moving ahead of the regular UTC cadence.
    ///
    /// Args:
    ///     now_unix: Completion time of the failed scan.
    ///     retry: Local failure class.
    ///     server_not_before: Optional GitHub retry deadline.
    ///
    /// Returns:
    ///     The later regular, local-backoff, or server-authorized deadline.
    fn after_failure_hint(
        &mut self,
        now_unix: u64,
        retry: DiscoveryRetry,
        server_not_before: Option<u64>,
    ) -> u64 {
        if self.last_failure == Some(retry) {
            self.failure_streak = self.failure_streak.saturating_add(1);
        } else {
            self.last_failure = Some(retry);
            self.failure_streak = 1;
        }
        let local = now_unix.saturating_add(failure_backoff(retry, self.failure_streak).as_secs());
        let regular = next_regular_poll(now_unix, now_unix, self.phase_seconds);
        later_unix(
            local.max(regular),
            server_not_before.map(|deadline| self.server_deadline(deadline)),
        )
    }

    /// Add a safety minute and reuse the process phase to spread reset-time wakeups.
    ///
    /// Args:
    ///     deadline: Raw absolute server deadline.
    ///
    /// Returns:
    ///     Deadline plus the safety margin and stable sub-minute spread.
    fn server_deadline(&self, deadline: u64) -> u64 {
        deadline
            .saturating_add(SERVER_DEADLINE_MARGIN_SECONDS)
            .saturating_add(self.phase_seconds % 60)
    }
}

/// Return the next stable UTC half-hour phase after both now and the startup minimum gap.
///
/// Args:
///     now_unix: Current completion time.
///     attempt_unix: Scan start time used for the minimum gap.
///     phase_seconds: Process-local half-hour offset.
///
/// Returns:
///     The first eligible anchored deadline.
fn next_regular_poll(now_unix: u64, attempt_unix: u64, phase_seconds: u64) -> u64 {
    let phase_seconds = phase_seconds % POLL_WINDOW_SECONDS;
    let window = now_unix / POLL_WINDOW_SECONDS;
    let mut deadline = window
        .saturating_mul(POLL_WINDOW_SECONDS)
        .saturating_add(phase_seconds);
    if deadline <= now_unix {
        deadline = deadline.saturating_add(POLL_WINDOW_SECONDS);
    }
    let minimum = attempt_unix.saturating_add(STARTUP_POLL_GAP_SECONDS);
    if deadline < minimum {
        deadline = deadline.saturating_add(POLL_WINDOW_SECONDS);
    }
    deadline
}

/// Return the documented bounded local delay for one failure streak.
///
/// Args:
///     retry: Failure class selecting the policy table.
///     streak: One-based consecutive failure count for that class.
///
/// Returns:
///     The capped local delay.
fn failure_backoff(retry: DiscoveryRetry, streak: usize) -> Duration {
    let index = streak.saturating_sub(1);
    let seconds = match retry {
        DiscoveryRetry::RateLimited => {
            RATE_BACKOFF_SECONDS[index.min(RATE_BACKOFF_SECONDS.len() - 1)]
        }
        DiscoveryRetry::Transient => {
            TRANSIENT_BACKOFF_SECONDS[index.min(TRANSIENT_BACKOFF_SECONDS.len() - 1)]
        }
        DiscoveryRetry::Protocol => PROTOCOL_BACKOFF_SECONDS,
    };
    Duration::from_secs(seconds)
}

/// Claim a one-shot loop start without coupling it to GPUI entity mechanics.
///
/// Args:
///     started: Process-wide loop-start flag.
///
/// Returns:
///     `true` only for the first claim.
fn claim_polling(started: &mut bool) -> bool {
    if *started {
        false
    } else {
        *started = true;
        true
    }
}

/// Keep discovery alive until the controller enters its terminal restart state.
///
/// Args:
///     state: Current process-wide updater state.
///
/// Returns:
///     `false` only after restart has been launched.
fn polling_continues_after(state: &UpdateState) -> bool {
    !matches!(state, UpdateState::Restarting(_))
}

/// Pick one stable process-local phase without adding a runtime dependency.
///
/// Returns:
///     An offset inside one half-hour polling window.
fn process_poll_phase() -> u64 {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    (u64::from(now.subsec_nanos()) ^ u64::from(std::process::id()).wrapping_mul(0x9e37_79b9))
        % POLL_WINDOW_SECONDS
}

/// Return the later of a required local deadline and an optional server deadline.
///
/// Args:
///     local: Required local deadline.
///     server: Optional absolute server deadline.
///
/// Returns:
///     The later applicable deadline.
fn later_unix(local: u64, server: Option<u64>) -> u64 {
    server.map_or(local, |server| local.max(server))
}

/// Single process-wide authority for discovery and one installation attempt.
pub(crate) struct UpdateController {
    state: UpdateState,
    candidate: Option<AvailableRelease>,
    polling_started: bool,
    install_generation: u64,
}

impl UpdateController {
    /// Create a hidden controller before the background release check begins.
    pub(crate) fn new() -> Self {
        Self {
            state: UpdateState::Hidden,
            candidate: None,
            polling_started: false,
            install_generation: 0,
        }
    }

    /// Return a snapshot used by every group header.
    pub(crate) fn state(&self) -> UpdateState {
        self.state.clone()
    }

    /// Start the idempotent process-wide release discovery loop.
    ///
    /// Args:
    ///     entity: Sole Backend-owned updater controller.
    ///     cx: GPUI application context used to spawn and publish state.
    pub(crate) fn start_polling(entity: &Entity<Self>, cx: &mut App) {
        let baseline = option_env!("MOONTERMINAL_RELEASE_BASE").unwrap_or("unknown");
        let identity = BuildIdentity::from_release_base(baseline);
        if identity.baseline().is_none() {
            return;
        }
        if !entity.update(cx, |this, _| claim_polling(&mut this.polling_started)) {
            return;
        }
        let controller = entity.clone();
        let executor = cx.background_executor().clone();
        cx.spawn(async move |cx| {
            let mut discovery = ReleaseDiscovery::new(identity);
            let mut schedule = PollSchedule::new(process_poll_phase());
            loop {
                let attempt_unix = now_unix_secs();
                let (returned, result) = executor
                    .spawn(async move {
                        let result = discovery.scan();
                        (discovery, result)
                    })
                    .await;
                discovery = returned;
                let completed_unix = now_unix_secs();
                let next_unix = match &result {
                    Ok(result) => schedule.after_success(
                        completed_unix,
                        attempt_unix,
                        result.defer_until_unix,
                    ),
                    Err(error) => schedule.after_failure(completed_unix, error),
                };
                let keep_polling = cx.update(|cx| {
                    controller.update(cx, |this, cx| {
                        match result {
                            Ok(result) => this.adopt_discovery(result.eligibility, cx),
                            Err(error) => log::warn!("update check failed: {error}"),
                        }
                        polling_continues_after(&this.state)
                    })
                });
                if !keep_polling {
                    break;
                }
                let wait = Duration::from_secs(next_unix.saturating_sub(now_unix_secs()).max(1));
                executor.timer(wait).await;
            }
        })
        .detach();
    }

    /// Adopt only a strictly newer candidate without stealing installation authority.
    ///
    /// Args:
    ///     eligibility: Complete discovery result for the current executable baseline.
    ///     cx: Entity context used to notify observers after a visible state change.
    fn adopt_discovery(&mut self, eligibility: UpdateEligibility, cx: &mut Context<Self>) {
        if self.state.busy() {
            return;
        }
        let UpdateEligibility::Available(release) = eligibility else {
            return;
        };
        let is_newer = self
            .candidate
            .as_ref()
            .is_none_or(|current| release.version() > current.version());
        if !is_newer {
            return;
        }
        self.state = UpdateState::Available(release.version());
        self.candidate = Some(release);
        cx.notify();
    }

    /// Start or retry the single installation attempt from an explicit user click.
    pub(crate) fn start_install(entity: &Entity<Self>, cx: &mut App) {
        let Some((candidate, version, generation)) = entity.update(cx, |this, cx| {
            if !this.state.clickable() {
                return None;
            }
            let candidate = this.candidate.clone()?;
            let version = candidate.version();
            this.install_generation = this.install_generation.wrapping_add(1);
            this.state = UpdateState::Installing(version);
            cx.notify();
            Some((candidate, version, this.install_generation))
        }) else {
            return;
        };
        let controller = entity.clone();
        let executor = cx.background_executor().clone();
        cx.spawn(async move |cx| {
            let result = executor
                .spawn(async move { prepare_install(candidate) })
                .await;
            cx.update(|cx| {
                controller.update(cx, |this, cx| {
                    if this.install_generation != generation {
                        return;
                    }
                    match result {
                        Ok(_) => {
                            this.state = UpdateState::Restarting(version);
                            cx.notify();
                            cx.quit();
                        }
                        Err(error) => {
                            this.state = UpdateState::Failed {
                                version,
                                message: error.to_string(),
                            };
                            cx.notify();
                        }
                    }
                });
            });
        })
        .detach();
    }
}

/// Persisted transaction authority shared by the old app, helper, and new app.
#[derive(Debug, Deserialize, Serialize)]
struct UpdateTransaction {
    version: u32,
    nonce: String,
    release_tag: String,
    target: PathBuf,
    staged: PathBuf,
    incoming: PathBuf,
    backup: PathBuf,
    ready: PathBuf,
    commit: PathBuf,
    started: PathBuf,
    healthy: PathBuf,
    expected_size: u64,
    expected_sha256: String,
    original_size: u64,
    original_sha256: String,
    parent_pid: u32,
}

/// Parse a hidden updater mode before any portable storage is opened.
pub(crate) fn dispatch_process_mode() -> anyhow::Result<ProcessDispatch> {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("--moonterminal-update-helper") => {
            let manifest = required_arg(&args, 2, "manifest")?;
            let nonce = required_arg(&args, 3, "nonce")?;
            run_helper(Path::new(manifest), nonce)?;
            Ok(ProcessDispatch::Exit)
        }
        Some("--moonterminal-update-resume") => {
            let manifest = PathBuf::from(required_arg(&args, 2, "manifest")?);
            let nonce = required_arg(&args, 3, "nonce")?.to_owned();
            let transaction = load_validated_manifest(&manifest, &nonce)?;
            write_marker(&transaction.started, &nonce)?;
            Ok(ProcessDispatch::Run(Some(StartupUpdate {
                manifest_path: manifest,
                nonce,
                recovered: false,
            })))
        }
        Some("--moonterminal-update-recovery") => {
            let manifest = PathBuf::from(required_arg(&args, 2, "manifest")?);
            let nonce = required_arg(&args, 3, "nonce")?.to_owned();
            load_validated_manifest(&manifest, &nonce)?;
            Ok(ProcessDispatch::Run(Some(StartupUpdate {
                manifest_path: manifest,
                nonce,
                recovered: true,
            })))
        }
        Some(value) if value.starts_with("--moonterminal-update-") => {
            bail!("unknown update process mode")
        }
        _ => Ok(ProcessDispatch::Run(None)),
    }
}

/// Accept a resumed executable before it may mutate portable storage.
///
/// Errors:
///     Returns manifest-validation or durable-marker failures so startup cannot continue into
///     portable storage while the helper still retains rollback authority.
pub(crate) fn acknowledge_healthy(receipt: Option<&StartupUpdate>) -> anyhow::Result<()> {
    let Some(receipt) = receipt else {
        return Ok(());
    };
    if receipt.recovered {
        return Ok(());
    }
    let transaction = load_validated_manifest(&receipt.manifest_path, &receipt.nonce)?;
    write_marker(&transaction.healthy, &receipt.nonce)?;
    schedule_completed_staging_cleanup(transaction.staged);
    Ok(())
}

/// Download the candidate, persist a strict manifest, and commit one acknowledged helper.
///
/// Args:
///     candidate: Exact immutable release selected by the background check.
///
/// Errors:
///     Returns download, manifest, spawn, acknowledgement, or child-termination failures while
///     keeping the current application alive.
fn prepare_install(candidate: AvailableRelease) -> anyhow::Result<()> {
    let nonce = random_nonce()?;
    let client = GitHubReleaseClient::new();
    let staged = client.download_verified(&candidate, &nonce)?;
    let (transaction, manifest_path) = match build_transaction(&candidate, nonce, &staged) {
        Ok(transaction) => transaction,
        Err(error) => {
            cleanup_staged_download(&staged);
            return Err(error);
        }
    };
    if let Err(error) = write_json_atomic(&manifest_path, &transaction) {
        cleanup_abandoned_transaction(&transaction, &manifest_path);
        return Err(error);
    }
    let mut helper = match Command::new(&transaction.staged)
        .arg("--moonterminal-update-helper")
        .arg(&manifest_path)
        .arg(&transaction.nonce)
        .spawn()
    {
        Ok(helper) => helper,
        Err(error) => {
            cleanup_abandoned_transaction(&transaction, &manifest_path);
            return Err(error).context("start verified update helper");
        }
    };
    let ready = wait_for_marker_or_exit(
        &transaction.ready,
        &transaction.nonce,
        &mut helper,
        HELPER_READY_TIMEOUT,
        "helper ready",
    )
    .and_then(|_| {
        write_marker(&transaction.commit, &transaction.nonce)
            .context("commit acknowledged update helper")
    });
    if let Err(error) = ready {
        terminate_child(&mut helper).context("stop unready update helper")?;
        cleanup_abandoned_transaction(&transaction, &manifest_path);
        return Err(error);
    }
    Ok(())
}

/// Build the complete same-directory transaction manifest from trusted constructors.
///
/// Args:
///     candidate: Immutable release whose exact tag and digest authorize the transaction.
///     nonce: Random validated transaction identifier.
///     staged: Verified helper path retained by the caller for failure cleanup.
///
/// Returns:
///     The serialized transaction value and its canonical manifest path.
///
/// Errors:
///     Returns path, staging-authority, or installed-executable hashing failures.
fn build_transaction(
    candidate: &AvailableRelease,
    nonce: String,
    staged: &Path,
) -> anyhow::Result<(UpdateTransaction, PathBuf)> {
    let canonical = paths::update_transaction_paths(&nonce)?;
    if staged != canonical.staged {
        bail!("downloaded update is outside the canonical transaction path");
    }
    let (original_size, original_sha256) = hash_file(&canonical.target)?;
    let transaction = UpdateTransaction {
        version: MANIFEST_VERSION,
        release_tag: candidate.release_tag().to_owned(),
        incoming: canonical.incoming,
        backup: canonical.backup,
        ready: canonical.ready,
        commit: canonical.commit,
        started: canonical.started,
        healthy: canonical.healthy,
        expected_size: candidate.asset_size(),
        expected_sha256: encode_hex(&candidate.asset_sha256()),
        original_size,
        original_sha256: encode_hex(&original_sha256),
        parent_pid: std::process::id(),
        target: canonical.target,
        staged: staged.to_path_buf(),
        nonce,
    };
    Ok((transaction, canonical.manifest))
}

/// Execute the replacement lifecycle from the verified downloaded executable.
///
/// Args:
///     manifest_path: Canonical transaction manifest supplied by the old application.
///     nonce: Exact transaction nonce supplied independently on the command line.
///
/// Errors:
///     Returns validation, commit-timeout, replacement, startup, or recovery failures.
fn run_helper(manifest_path: &Path, nonce: &str) -> anyhow::Result<()> {
    let transaction = load_validated_manifest(manifest_path, nonce)?;
    validate_helper_identity(&transaction)?;
    validate_transaction_tree(&transaction)?;
    let parent = open_parent(transaction.parent_pid, &transaction.target)?;
    if let Err(error) = write_marker(&transaction.ready, nonce) {
        close_parent(parent);
        return Err(error);
    }
    if let Err(error) = wait_for_marker(
        &transaction.commit,
        nonce,
        HELPER_COMMIT_TIMEOUT,
        "install commit",
    ) {
        close_parent(parent);
        return Err(error);
    }
    wait_parent(parent)?;
    let result = install_after_parent_exit(&transaction, manifest_path, nonce);
    if result.is_ok() {
        return Ok(());
    }
    let update_error = result.unwrap_err();
    recover_viable_target(&transaction, manifest_path)
        .with_context(|| format!("update failed ({update_error}); recovery also failed"))?;
    Err(update_error)
}

/// Replace and start the new executable after the old process has definitely exited.
fn install_after_parent_exit(
    transaction: &UpdateTransaction,
    manifest_path: &Path,
    nonce: &str,
) -> anyhow::Result<()> {
    copy_exclusive(&transaction.staged, &transaction.incoming)?;
    verify_file(
        &transaction.incoming,
        transaction.expected_size,
        &transaction.expected_sha256,
    )?;
    replace_target(
        &transaction.target,
        &transaction.incoming,
        &transaction.backup,
    )?;
    verify_file(
        &transaction.target,
        transaction.expected_size,
        &transaction.expected_sha256,
    )?;
    let mut child = launch_target(transaction, manifest_path, "--moonterminal-update-resume")?;
    let started = wait_for_marker_or_exit(
        &transaction.started,
        nonce,
        &mut child,
        STARTED_TIMEOUT,
        "new app started",
    );
    let healthy = started.and_then(|_| {
        wait_for_marker_or_exit(
            &transaction.healthy,
            nonce,
            &mut child,
            HEALTHY_TIMEOUT,
            "new app healthy",
        )
    });
    if healthy.is_ok() {
        cleanup_accepted_transaction(transaction, manifest_path);
        return Ok(());
    }
    let error = healthy.unwrap_err();
    terminate_child(&mut child).context("stop unhealthy updated application")?;
    Err(error)
}

/// Restore or relaunch a verified viable old target after any post-quit failure.
fn recover_viable_target(
    transaction: &UpdateTransaction,
    manifest_path: &Path,
) -> anyhow::Result<()> {
    if transaction.backup.exists() {
        restore_backup(&transaction.target, &transaction.backup)?;
    }
    verify_file(
        &transaction.target,
        transaction.original_size,
        &transaction.original_sha256,
    )?;
    launch_target(transaction, manifest_path, "--moonterminal-update-recovery")?;
    Ok(())
}

/// Prove the executing helper is the immutable staged asset named by the manifest.
fn validate_helper_identity(transaction: &UpdateTransaction) -> anyhow::Result<()> {
    let current = paths::executable_path()?;
    if !same_file_path(&current, &transaction.staged) {
        bail!("update helper is not the staged executable");
    }
    verify_file(
        &current,
        transaction.expected_size,
        &transaction.expected_sha256,
    )
}

/// Reject reparse points, links, and pre-planted mutation destinations in the transaction tree.
fn validate_transaction_tree(transaction: &UpdateTransaction) -> anyhow::Result<()> {
    let directory = transaction
        .staged
        .parent()
        .ok_or_else(|| anyhow!("staged helper has no transaction directory"))?;
    let root = directory
        .parent()
        .ok_or_else(|| anyhow!("transaction has no update root"))?;
    for path in [root, directory] {
        let metadata = fs::symlink_metadata(path)?;
        if !paths::is_plain_directory(&metadata) {
            bail!("update transaction contains a linked directory");
        }
    }
    let staged = fs::symlink_metadata(&transaction.staged)?;
    if !staged.is_file() || staged.file_type().is_symlink() {
        bail!("staged helper is not a plain file");
    }
    for path in [&transaction.incoming, &transaction.backup] {
        if fs::symlink_metadata(path).is_ok() {
            bail!("update mutation destination already exists");
        }
    }
    Ok(())
}

/// Copy into an exclusive plain file instead of following a planted destination link.
fn copy_exclusive(source: &Path, destination: &Path) -> anyhow::Result<()> {
    let mut source = File::open(source).context("open staged helper for replacement copy")?;
    let mut destination = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .context("create exclusive replacement input")?;
    std::io::copy(&mut source, &mut destination).context("copy replacement input")?;
    destination.sync_all().context("sync replacement input")
}

/// Validate that an untrusted manifest names only the canonical transaction paths and target.
fn load_validated_manifest(path: &Path, nonce: &str) -> anyhow::Result<UpdateTransaction> {
    let canonical = paths::update_helper_paths(path, nonce)?;
    let bytes = fs::read(path).context("read update manifest")?;
    if bytes.len() > 64 * 1024 {
        bail!("update manifest is too large");
    }
    let transaction: UpdateTransaction =
        serde_json::from_slice(&bytes).context("parse update manifest")?;
    if transaction.version != MANIFEST_VERSION
        || transaction.nonce != nonce
        || !same_installed_target(&transaction.target, &canonical.target)
        || transaction.staged != canonical.staged
        || transaction.incoming != canonical.incoming
        || transaction.backup != canonical.backup
        || transaction.ready != canonical.ready
        || transaction.commit != canonical.commit
        || transaction.started != canonical.started
        || transaction.healthy != canonical.healthy
        || transaction.expected_size == 0
        || parse_digest(&transaction.expected_sha256).is_none()
        || transaction.original_size == 0
        || parse_digest(&transaction.original_sha256).is_none()
        || ReleaseVersion::parse(&transaction.release_tag).is_none()
    {
        bail!("update manifest failed canonical validation");
    }
    Ok(transaction)
}

/// Persist a small JSON authority through an exclusive temporary file and rename.
fn write_json_atomic(path: &Path, value: &impl Serialize) -> anyhow::Result<()> {
    let bytes = serde_json::to_vec(value).context("serialize update manifest")?;
    let temporary = path.with_extension("json.incoming");
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    drop(file);
    fs::rename(&temporary, path).context("publish update manifest")
}

/// Write one nonce-bound acknowledgement without overwriting an existing file.
fn write_marker(path: &Path, nonce: &str) -> anyhow::Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(nonce.as_bytes())?;
    file.sync_all()?;
    Ok(())
}

/// Wait for an exact marker while proving the child remains alive.
fn wait_for_marker_or_exit(
    marker: &Path,
    nonce: &str,
    child: &mut Child,
    timeout: Duration,
    label: &str,
) -> anyhow::Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        if fs::read_to_string(marker).ok().as_deref() == Some(nonce) {
            return Ok(());
        }
        if let Some(status) = child.try_wait()? {
            bail!("{label} process exited with {status}");
        }
        if Instant::now() >= deadline {
            bail!("timed out waiting for {label}");
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Wait a bounded interval for an exact nonce-bound acknowledgement.
///
/// Args:
///     marker: Canonical marker file to read.
///     nonce: Exact expected marker contents.
///     timeout: Maximum wait from this process phase.
///     label: ASCII phase label used in errors.
///
/// Errors:
///     Returns an error when the marker does not arrive before the deadline.
fn wait_for_marker(
    marker: &Path,
    nonce: &str,
    timeout: Duration,
    label: &str,
) -> anyhow::Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        if fs::read_to_string(marker).ok().as_deref() == Some(nonce) {
            return Ok(());
        }
        if Instant::now() >= deadline {
            bail!("timed out waiting for {label}");
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Launch the installed target with one nonce-bound startup receipt.
fn launch_target(
    transaction: &UpdateTransaction,
    manifest_path: &Path,
    mode: &str,
) -> anyhow::Result<Child> {
    Command::new(&transaction.target)
        .arg(mode)
        .arg(manifest_path)
        .arg(&transaction.nonce)
        .spawn()
        .context("launch installed MoonTerminal")
}

/// Accept the installed executable's real casing while binding it to the derived install root.
fn same_installed_target(actual: &Path, canonical: &Path) -> bool {
    actual.parent() == canonical.parent()
        && actual
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.eq_ignore_ascii_case("MoonTerminal.exe"))
}

/// Compare two existing Windows paths through canonical filesystem identities.
fn same_file_path(left: &Path, right: &Path) -> bool {
    match (fs::canonicalize(left), fs::canonicalize(right)) {
        (Ok(left), Ok(right)) => left
            .to_string_lossy()
            .eq_ignore_ascii_case(&right.to_string_lossy()),
        _ => false,
    }
}

/// Remove rollback authority only after this helper consumed the healthy acknowledgement.
fn cleanup_accepted_transaction(transaction: &UpdateTransaction, manifest_path: &Path) {
    for path in [
        &transaction.backup,
        &transaction.incoming,
        &transaction.ready,
        &transaction.commit,
        &transaction.started,
        &transaction.healthy,
        manifest_path,
    ] {
        remove_plain_file(path);
    }
}

/// Remove a pre-quit transaction after its helper is known not to be running.
///
/// Args:
///     transaction: Canonical paths owned by the abandoned transaction.
///     manifest_path: Exact manifest file created for that transaction.
fn cleanup_abandoned_transaction(transaction: &UpdateTransaction, manifest_path: &Path) {
    cleanup_accepted_transaction(transaction, manifest_path);
    cleanup_staged_download(&transaction.staged);
}

/// Remove a downloaded helper and its empty transaction directory before helper ownership.
///
/// Args:
///     staged: Exact verified helper path whose transaction never gained process ownership.
fn cleanup_staged_download(staged: &Path) {
    let staged_removed = remove_plain_file(staged);
    if staged_removed && let Some(directory) = staged.parent() {
        let _ = fs::remove_dir(directory);
    }
}

/// Remove the staged helper after Windows releases its running executable handle.
fn schedule_completed_staging_cleanup(staged: PathBuf) {
    std::thread::spawn(move || {
        let Some(directory) = staged.parent().map(Path::to_path_buf) else {
            return;
        };
        for _ in 0..120 {
            std::thread::sleep(Duration::from_millis(250));
            let staged_removed = remove_plain_file(&staged);
            if staged_removed && fs::remove_dir(&directory).is_ok() {
                return;
            }
        }
        log::warn!(
            "completed update staging cleanup deferred: {}",
            directory.display()
        );
    });
}

/// Remove one plain file without following links and treat absence as success.
///
/// Args:
///     path: Exact transaction file eligible for removal.
///
/// Returns:
///     `true` when the file is absent after the call, otherwise `false`.
fn remove_plain_file(path: &Path) -> bool {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
            fs::remove_file(path).is_ok()
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => true,
        _ => false,
    }
}

/// Terminate and reap only the exact child handle retained by the helper.
///
/// Args:
///     child: Exact process handle created by this transaction phase.
///
/// Errors:
///     Returns process-query, termination, or wait failures without authorizing cleanup.
fn terminate_child(child: &mut Child) -> anyhow::Result<()> {
    if child.try_wait()?.is_some() {
        return Ok(());
    }
    child.kill().context("terminate child process")?;
    child.wait().context("reap terminated child process")?;
    Ok(())
}

/// Generate a lowercase hexadecimal transaction nonce.
fn random_nonce() -> anyhow::Result<String> {
    let mut bytes = [0u8; 16];
    getrandom::getrandom(&mut bytes).context("generate update transaction nonce")?;
    Ok(encode_hex(&bytes))
}

/// Hash and verify one exact executable file.
fn verify_file(path: &Path, expected_size: u64, expected_digest: &str) -> anyhow::Result<()> {
    let expected =
        parse_digest(expected_digest).ok_or_else(|| anyhow!("invalid expected digest"))?;
    let (size, actual) = hash_file(path)?;
    if size != expected_size || actual != expected {
        bail!("update executable verification failed");
    }
    Ok(())
}

/// Hash one executable before the helper-ready point so rollback can prove its identity.
fn hash_file(path: &Path) -> anyhow::Result<(u64, [u8; 32])> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let size = std::io::copy(&mut file, &mut hasher)?;
    Ok((size, hasher.finalize().into()))
}

/// Parse a 64-character lowercase hexadecimal SHA-256 value.
fn parse_digest(value: &str) -> Option<[u8; 32]> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return None;
    }
    let mut output = [0u8; 32];
    for (index, byte) in output.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16).ok()?;
    }
    Some(output)
}

/// Encode bytes as lowercase hexadecimal text.
fn encode_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Read a required positional CLI argument.
fn required_arg<'a>(args: &'a [String], index: usize, label: &str) -> anyhow::Result<&'a str> {
    args.get(index)
        .map(String::as_str)
        .ok_or_else(|| anyhow!("missing update {label} argument"))
}

#[cfg(windows)]
type ParentHandle = windows::Win32::Foundation::HANDLE;

#[cfg(not(windows))]
type ParentHandle = ();

/// Open the exact parent process for synchronization before acknowledging readiness.
#[cfg(windows)]
fn open_parent(pid: u32, target: &Path) -> anyhow::Result<ParentHandle> {
    use windows::Win32::System::Threading::{
        OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
        QueryFullProcessImageNameW,
    };
    use windows::core::PWSTR;
    let handle = unsafe {
        OpenProcess(
            PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION,
            false,
            pid,
        )
    }
    .context("open update parent process")?;
    let mut buffer = vec![0u16; 32_768];
    let mut length = buffer.len() as u32;
    if let Err(error) = unsafe {
        QueryFullProcessImageNameW(
            handle,
            Default::default(),
            PWSTR(buffer.as_mut_ptr()),
            &mut length,
        )
    } {
        let _ = unsafe { windows::Win32::Foundation::CloseHandle(handle) };
        return Err(error).context("query update parent image");
    }
    let image = PathBuf::from(String::from_utf16_lossy(&buffer[..length as usize]));
    if !same_file_path(&image, target) {
        let _ = unsafe { windows::Win32::Foundation::CloseHandle(handle) };
        bail!("update parent is not the installed executable");
    }
    Ok(handle)
}

/// Reject helper execution on unsupported platforms.
#[cfg(not(windows))]
fn open_parent(_pid: u32, _target: &Path) -> anyhow::Result<ParentHandle> {
    bail!("self-update is supported only on Windows")
}

/// Close an opened parent handle when the helper transaction is not committed.
///
/// Args:
///     handle: Exact synchronization handle opened before publishing readiness.
#[cfg(windows)]
fn close_parent(handle: ParentHandle) {
    let _ = unsafe { windows::Win32::Foundation::CloseHandle(handle) };
}

/// Discard the unsupported-platform parent placeholder.
///
/// Args:
///     _handle: Unsupported-platform placeholder value.
#[cfg(not(windows))]
fn close_parent(_handle: ParentHandle) {}

/// Wait for the parent to exit normally and close its synchronization handle.
#[cfg(windows)]
fn wait_parent(handle: ParentHandle) -> anyhow::Result<()> {
    use windows::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
    use windows::Win32::System::Threading::WaitForSingleObject;
    // After publishing ready, the helper owns availability. Waiting indefinitely keeps the old
    // executable viable even when persistence or shutdown takes longer than expected.
    let result = unsafe { WaitForSingleObject(handle, u32::MAX) };
    let _ = unsafe { CloseHandle(handle) };
    if result == WAIT_OBJECT_0 {
        Ok(())
    } else {
        bail!("failed while waiting for update parent exit")
    }
}

/// Unreachable non-Windows parent wait retained for cross-platform compilation.
#[cfg(not(windows))]
fn wait_parent(_handle: ParentHandle) -> anyhow::Result<()> {
    bail!("self-update is supported only on Windows")
}

/// Atomically replace the installed executable while retaining a backup.
#[cfg(windows)]
fn replace_target(target: &Path, incoming: &Path, backup: &Path) -> anyhow::Result<()> {
    replace_file(target, incoming, Some(backup)).context("replace installed executable")
}

/// Reject replacement on unsupported platforms.
#[cfg(not(windows))]
fn replace_target(_target: &Path, _incoming: &Path, _backup: &Path) -> anyhow::Result<()> {
    bail!("self-update is supported only on Windows")
}

/// Restore the verified backup over a failed replacement.
#[cfg(windows)]
fn restore_backup(target: &Path, backup: &Path) -> anyhow::Result<()> {
    if target.exists() {
        replace_file(target, backup, None).context("restore previous executable")
    } else {
        fs::rename(backup, target).context("restore missing previous executable")
    }
}

/// Reject rollback on unsupported platforms.
#[cfg(not(windows))]
fn restore_backup(_target: &Path, _backup: &Path) -> anyhow::Result<()> {
    bail!("self-update is supported only on Windows")
}

/// Call `ReplaceFileW` with stable UTF-16 buffers.
#[cfg(windows)]
fn replace_file(target: &Path, incoming: &Path, backup: Option<&Path>) -> anyhow::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Storage::FileSystem::{REPLACE_FILE_FLAGS, ReplaceFileW};
    use windows::core::PCWSTR;
    let wide = |path: &Path| {
        path.as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect::<Vec<_>>()
    };
    let target = wide(target);
    let incoming = wide(incoming);
    let backup = backup.map(wide);
    unsafe {
        ReplaceFileW(
            PCWSTR(target.as_ptr()),
            PCWSTR(incoming.as_ptr()),
            backup
                .as_ref()
                .map_or(PCWSTR::null(), |value| PCWSTR(value.as_ptr())),
            REPLACE_FILE_FLAGS(0),
            None,
            None,
        )
    }
    .map_err(Into::into)
}
