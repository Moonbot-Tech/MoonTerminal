//! The station updating itself: `[update] auto` in `station.toml`, on unless it says `false`.
//!
//! A thread looks at the latest release shortly after the start and then every few hours, with
//! jitter so a fleet of stations does not ask GitHub at one moment. When a newer release carries
//! this station's binary it files the SAME request the bot's "Update" and the terminal's "Update
//! the service" file ([`crate::release::request_update`]): the root updater does the rest, and
//! its verdict lands in `Status.last_update` as for any update.
//!
//! Only a build of a release tag itself updates itself: a build of a later commit carries the tag
//! as its baseline too ([`crate::release::exact_release`]), and a release must not replace it.
//!
//! The station waits for the terminal: a release newer than the newest terminal that reported to
//! it (`[terminal] version`) is held back, so the station never outgrows the terminal that drives
//! it. Once that terminal is updated and reports again, the next look installs the release. A
//! station no terminal reported to updates freely. The button updates are never held.
//!
//! One automatic attempt per newer version per day: the attempt is kept in the data root
//! ([`ATTEMPT_FILE`]), so an update that failed and restarted the old binary does not file the
//! same request again right after the start.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use moon_core::update::{
    BuildIdentity, ReleaseDiscovery, ReleaseVersion, UpdateEligibility, station_asset_name,
};

/// The last automatic attempt, `<version> <unix seconds>`, in the data root.
const ATTEMPT_FILE: &str = "update.auto";
/// How soon after the start the first look runs, before its jitter.
const FIRST_LOOK: Duration = Duration::from_secs(5 * 60);
/// How often the station looks again, on the UTC clock's multiples of it, before its jitter.
const LOOK_EVERY: Duration = Duration::from_secs(6 * 60 * 60);
/// The most jitter added to the first look.
const FIRST_JITTER: Duration = Duration::from_secs(5 * 60);
/// The most jitter added to every later look.
const LOOK_JITTER: Duration = Duration::from_secs(60 * 60);
/// How long one failed automatic attempt at a version holds the next one back.
const RETRY_AFTER_S: u64 = 24 * 60 * 60;

/// An automatic update attempt: which version was asked for, and when.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Attempt {
    pub version: ReleaseVersion,
    pub at_s: u64,
}

/// What one look decided.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    /// `[update] auto = false`: updates only by the button.
    Off,
    /// A build that is not a release tag's own is never replaced by a release behind the user's back.
    Unversioned,
    /// No release newer than this station carries its binary.
    Current,
    /// This version was asked for less than a day ago; its verdict is `Status.last_update`.
    Tried(ReleaseVersion),
    /// This version is out, but newer than the newest terminal that reported: it waits until
    /// a terminal of that version reports.
    WaitsForTerminal(ReleaseVersion),
    /// File the update request for this version.
    Update(ReleaseVersion),
}

/// Whether an automatic update is due.
///
/// Args:
///     auto: The `[update] auto` switch.
///     own: This station's release version; `None` for a build that is not a release tag's own.
///     latest: The newest release that carries this station's binary for its architecture;
///         `None` when there is none.
///     terminal: The newest terminal release that reported (`[terminal] version`); `None` holds
///         nothing back.
///     last: The last automatic attempt, if any.
///     now_s: Unix seconds now.
///
/// Returns:
///     The decision; [`Decision::Update`] only for a release strictly newer than `own`, not
///     newer than `terminal`, that was not asked for within the last day.
pub fn decide(
    auto: bool,
    own: Option<ReleaseVersion>,
    latest: Option<ReleaseVersion>,
    terminal: Option<ReleaseVersion>,
    last: Option<Attempt>,
    now_s: u64,
) -> Decision {
    if !auto {
        return Decision::Off;
    }
    let Some(own) = own else {
        return Decision::Unversioned;
    };
    let Some(latest) = latest.filter(|latest| *latest > own) else {
        return Decision::Current;
    };
    if terminal.is_some_and(|terminal| latest > terminal) {
        return Decision::WaitsForTerminal(latest);
    }
    match last {
        Some(last) if last.version == latest && now_s.saturating_sub(last.at_s) < RETRY_AFTER_S => {
            Decision::Tried(latest)
        }
        _ => Decision::Update(latest),
    }
}

/// Parse [`ATTEMPT_FILE`]'s text; `None` for anything else.
fn parse_attempt(text: &str) -> Option<Attempt> {
    let mut words = text.split_whitespace();
    let version = ReleaseVersion::parse(words.next()?)?;
    let at_s = words.next()?.parse().ok()?;
    Some(Attempt { version, at_s })
}

/// The terminal side of a look: the ceiling the main loop sets from `station.toml`, and the
/// release the last look held back under it.
#[derive(Default)]
struct Terminal {
    version: Option<ReleaseVersion>,
    waits_for: Option<ReleaseVersion>,
}

/// The switch the main loop flips on a reload and the status reports.
#[derive(Clone)]
pub struct AutoUpdate {
    on: Arc<AtomicBool>,
    terminal: Arc<Mutex<Terminal>>,
}

impl AutoUpdate {
    /// Start the looking thread with the switch at `on` and the terminal ceiling at `terminal`.
    /// A thread that cannot start is logged: the station runs on and updates by the button.
    pub fn start(on: bool, terminal: Option<ReleaseVersion>, data_root: PathBuf) -> Self {
        let auto = Self {
            on: Arc::new(AtomicBool::new(on)),
            terminal: Arc::new(Mutex::new(Terminal {
                version: terminal,
                waits_for: None,
            })),
        };
        let switch = auto.clone();
        if let Err(e) = std::thread::Builder::new()
            .name("auto-update".into())
            .spawn(move || switch.run(&data_root))
        {
            log::warn!("auto-update: no thread ({e}); the station updates only by the button");
        }
        auto
    }

    /// Whether the station updates itself.
    pub fn on(&self) -> bool {
        self.on.load(Ordering::Relaxed)
    }

    /// Set the switch from a re-read `station.toml`; logged when it changes.
    pub fn set(&self, on: bool) {
        if self.on.swap(on, Ordering::Relaxed) != on {
            log::info!("auto-update: switched {}", if on { "on" } else { "off" });
        }
    }

    /// The terminal's state, readable after a panicked writer: it holds only plain values.
    fn terminal(&self) -> std::sync::MutexGuard<'_, Terminal> {
        self.terminal.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Set the terminal ceiling from a re-read `station.toml`; logged when it changes. The next
    /// look applies it.
    pub fn set_terminal(&self, version: Option<ReleaseVersion>) {
        let mut terminal = self.terminal();
        if terminal.version != version {
            match version {
                Some(version) => log::info!("auto-update: the terminal reported {version}"),
                None => log::info!("auto-update: no terminal version; updates are not held"),
            }
            terminal.version = version;
            // A hold under the old ceiling no longer stands; the next look says what does.
            terminal.waits_for = None;
        }
    }

    /// The newest terminal release that reported, for the status.
    pub fn terminal_version(&self) -> Option<ReleaseVersion> {
        self.terminal().version
    }

    /// The release the last look held back for the terminal, for the status.
    pub fn waits_for(&self) -> Option<ReleaseVersion> {
        self.terminal().waits_for
    }

    /// The looking loop: never returns. Each decision is logged once, until it changes.
    fn run(&self, data_root: &Path) {
        let mut discovery: Option<ReleaseDiscovery> = None;
        let mut logged: Option<Decision> = None;
        std::thread::sleep(FIRST_LOOK + jitter(FIRST_JITTER));
        loop {
            let decision = self.look(data_root, &mut discovery);
            if let Some(decision) = decision {
                self.terminal().waits_for = match decision {
                    Decision::WaitsForTerminal(version) => Some(version),
                    _ => None,
                };
            }
            if decision.is_some() && decision != logged {
                if let Some(decision) = decision {
                    log_decision(decision);
                }
                logged = decision;
            }
            std::thread::sleep(until_next_slot(unix_now_s()) + jitter(LOOK_JITTER));
        }
    }

    /// One look: GitHub is asked only while the switch is on and the build is a release's.
    /// `None` when the releases could not be read (logged).
    fn look(&self, data_root: &Path, discovery: &mut Option<ReleaseDiscovery>) -> Option<Decision> {
        let identity = BuildIdentity::from_release_base(crate::release::release_base());
        let own = crate::release::exact_release();
        let now_s = unix_now_s();
        let last = std::fs::read_to_string(data_root.join(ATTEMPT_FILE))
            .ok()
            .and_then(|text| parse_attempt(&text));
        let terminal = self.terminal_version();
        if !self.on() || own.is_none() {
            return Some(decide(self.on(), own, None, terminal, last, now_s));
        }
        let Some(asset) = station_asset_name() else {
            log::warn!(
                "auto-update: no station binary is released for {}",
                std::env::consts::ARCH
            );
            return None;
        };
        let discovery =
            discovery.get_or_insert_with(|| ReleaseDiscovery::for_asset(identity, asset));
        let latest = match discovery.scan() {
            Ok(scan) => match scan.eligibility {
                UpdateEligibility::Available(release) => Some(release.version()),
                UpdateEligibility::Current | UpdateEligibility::Unsupported => None,
            },
            Err(e) => {
                log::warn!("auto-update: the releases could not be read: {e}");
                return None;
            }
        };
        let decision = decide(self.on(), own, latest, terminal, last, now_s);
        if let Decision::Update(version) = decision {
            // Recorded first: an attempt that cannot be recorded is not made, or a failed update
            // would be requested again at every start.
            let line = format!("{version} {now_s}\n");
            if let Err(e) = std::fs::write(data_root.join(ATTEMPT_FILE), line) {
                log::warn!(
                    "auto-update: {version} not requested, the attempt cannot be recorded: {e}"
                );
                return None;
            }
            match crate::release::request_update(data_root) {
                Ok(()) | Err(moon_tg::UpdateRefusal::AlreadyRunning) => {}
                Err(refusal) => {
                    log::warn!("auto-update: {version} not requested: {refusal:?}");
                    return None;
                }
            }
        }
        Some(decision)
    }
}

/// The decision's one English log line.
fn log_decision(decision: Decision) {
    match decision {
        Decision::Off => log::info!("auto-update: off; the station updates only by the button"),
        Decision::Unversioned => {
            log::info!("auto-update: this build is not a release's own; not updating itself")
        }
        Decision::Current => log::info!("auto-update: no newer release"),
        Decision::Tried(version) => log::info!(
            "auto-update: {version} was requested less than a day ago; see the last update's verdict"
        ),
        Decision::WaitsForTerminal(version) => log::info!(
            "auto-update: {version} is out; it waits for a terminal of that version to report"
        ),
        Decision::Update(version) => log::info!("auto-update: {version} is out, update requested"),
    }
}

/// How long from `now_s` to the next multiple of [`LOOK_EVERY`] on the UTC clock: the looks keep
/// their times across restarts and slow scans.
pub(crate) fn until_next_slot(now_s: u64) -> Duration {
    let every = LOOK_EVERY.as_secs();
    Duration::from_secs(every - now_s % every)
}

/// Unix seconds now; 0 before the epoch.
fn unix_now_s() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// A delay up to `max`, from the clock's sub-second part: enough to spread stations apart.
fn jitter(max: Duration) -> Duration {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos()) as u64;
    Duration::from_secs(nanos % max.as_secs().max(1))
}

#[cfg(test)]
mod tests;
