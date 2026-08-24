//! Runtime diagnostics switches, collected out of the environment and into `cfg/diagnostics.toml`.
//!
//! Before this module every switch was an environment variable, which on Windows made them nearly
//! unusable: the terminal is a GUI process started from a shortcut, so setting a variable meant a
//! batch file or a system-wide edit and a re-login. Worse, there was no list — the only way to
//! learn what could be traced was to grep the sources. The file is that list for every OBSERVATION
//! channel; switches that change what the program DOES (`MOON_ARCHIVE_PROBE`, the
//! `MOON_RENDER_DIAG_OPEN_*` startup automation, `MOON_ANALYTICS_PROBE`, which opens a window) stay
//! on the environment deliberately, and `tests/diagnostics_contract.rs` holds that line.
//!
//! Three properties the rest of the code depends on:
//!
//! - **Live.** Every switch is an atomic (or, for the log areas, a swappable filter), never a
//!   `OnceLock` read once per process. [`poll`] re-reads the file when it changes on disk, so a
//!   switch can be flipped while the terminal runs — which matters, because the state worth
//!   observing is usually the one a restart would destroy.
//! - **Off is free.** A disabled channel costs one relaxed atomic load at its call site. A disabled
//!   log area leaves `log::max_level` where it was, so the `debug!` sites short-circuit inside the
//!   macro and never build their arguments.
//! - **The environment still wins.** A variable can only turn a switch ON, and the file cannot turn
//!   it off; see [`config::apply_env`]. FireTest and CI keep working unchanged.
//!
//! Initialisation order is load-bearing and is spelled out in [`init`].

pub mod config;
mod filter;
mod template;
mod upgrade;

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock, RwLock};
use std::time::Duration;

pub use config::DiagCfg;
pub use filter::{filter_string, DEFAULT_BASE_FILTER};

use crate::config::paths;

static RENDER: AtomicBool = AtomicBool::new(false);
static DETECT: AtomicBool = AtomicBool::new(false);
static ASSETS: AtomicBool = AtomicBool::new(false);
static MARKETS: AtomicBool = AtomicBool::new(false);
/// Fast path for the order channel: the selector itself sits behind a lock, and reading that lock
/// on every order of every core would be the one diagnostic that costs something while OFF.
static ORDERS_ON: AtomicBool = AtomicBool::new(false);
static ORDERS_SELECTOR: RwLock<String> = RwLock::new(String::new());
static COIN_NAMING_ON: AtomicBool = AtomicBool::new(false);
static COIN_NAMING_SELECTOR: RwLock<String> = RwLock::new(String::new());

static RING_LINES: AtomicUsize = AtomicUsize::new(config::DEFAULT_RING_LINES);
static BALANCE_WINDOW_SEC: AtomicU64 = AtomicU64::new(config::DEFAULT_BALANCE_WINDOW_SEC);
static MARKET_FLOOR_MS: AtomicU64 = AtomicU64::new(config::DEFAULT_MARKET_FLOOR_MS);

/// Mtime and length at the last read, the pair [`poll`] compares to decide the file changed.
///
/// Length is part of the signal because mtime alone is not enough where this file lives: the
/// Windows layout puts it beside the executable, which may be on a FAT/exFAT volume whose
/// timestamps have two-second granularity. Two edits inside one bucket would otherwise be invisible.
static LAST_MTIME_MS: AtomicU64 = AtomicU64::new(0);
static LAST_LEN: AtomicU64 = AtomicU64::new(0);

/// The configuration currently published, so [`poll`] can tell a real edit from a bare `touch`.
static ACTIVE: OnceLock<Mutex<DiagCfg>> = OnceLock::new();

fn active_cell() -> &'static Mutex<DiagCfg> {
    ACTIVE.get_or_init(|| Mutex::new(DiagCfg::default()))
}

/// Render counters and their per-second line (`logs/render_diag.log`).
#[inline]
pub fn render() -> bool {
    RENDER.load(Ordering::Relaxed)
}

/// Detect-pipeline channel (`logs/detect_diag.log`).
#[inline]
pub fn detect() -> bool {
    DETECT.load(Ordering::Relaxed)
}

/// Raw balance rows per core, before the Assets panel's dust filter.
#[inline]
pub fn assets() -> bool {
    ASSETS.load(Ordering::Relaxed)
}

/// Market roles, order-book pull and archive reach.
#[inline]
pub fn markets() -> bool {
    MARKETS.load(Ordering::Relaxed)
}

/// Whether the order channel is following anything at all.
#[inline]
pub fn orders() -> bool {
    ORDERS_ON.load(Ordering::Relaxed)
}

/// Run `f` on the order selector, or return `None` when the channel is off.
///
/// The selector is borrowed rather than cloned: this runs per order per core, and handing out a
/// `String` would allocate on a path whose whole point is to stay cheap enough to leave on. `f`
/// must not log — it is called under a read lock this module also takes for writing.
pub fn with_orders_selector<R>(f: impl FnOnce(&str) -> R) -> Option<R> {
    if !orders() {
        return None;
    }
    let guard = ORDERS_SELECTOR
        .read()
        .unwrap_or_else(|e| e.into_inner());
    Some(f(guard.as_str()))
}

/// Whether the coin-spelling channel is following anything at all.
#[inline]
pub fn coin_naming_on() -> bool {
    COIN_NAMING_ON.load(Ordering::Relaxed)
}

/// Run `f` on the coin-spelling selector, or return `None` when the channel is off.
///
/// Borrowed rather than cloned for the same reason the order selector is: this is asked once per
/// market per sweep, and the off case must cost one atomic load. `f` must not log — it runs under
/// a read lock this module also takes for writing.
pub fn with_coin_naming_selector<R>(f: impl FnOnce(&str) -> R) -> Option<R> {
    if !coin_naming_on() {
        return None;
    }
    let guard = COIN_NAMING_SELECTOR
        .read()
        .unwrap_or_else(|e| e.into_inner());
    Some(f(guard.as_str()))
}

/// Lines the Log panel keeps in memory.
#[inline]
pub fn ring_lines() -> usize {
    RING_LINES.load(Ordering::Relaxed)
}

/// Window within which repeated balance-repair requests for one core collapse into a single line.
pub fn balance_repeat_window() -> Duration {
    Duration::from_secs(BALANCE_WINDOW_SEC.load(Ordering::Relaxed))
}

/// Minimum gap between two market-trace lines about the same subject.
pub fn market_trace_min_interval() -> Duration {
    Duration::from_millis(MARKET_FLOOR_MS.load(Ordering::Relaxed))
}

/// Directory holding the channel files. The PATH is cached; whether it exists is not.
///
/// Caching a creation FAILURE would be worse than the per-line `create_dir_all` it replaced: one
/// transient error — an antivirus holding the folder, a sync client mid-rename — would kill every
/// channel file for the rest of the process, where the old code simply succeeded on the next line.
fn channel_dir() -> &'static std::path::PathBuf {
    static DIR: OnceLock<std::path::PathBuf> = OnceLock::new();
    // The NON-logging resolver. No channel writes from inside a log record today, but this is the
    // same shape as the `DatedWriter::open` self-deadlock, and the creation this would have done
    // for us already happens in `channel_line`'s retry.
    DIR.get_or_init(paths::logs_dir_no_create)
}

/// Append one line to a channel's own file under `logs/`.
///
/// Every channel writes through here so the location is decided once. Historically each opened its
/// file by RELATIVE path, which put it in the process working directory — the folder the terminal
/// was started from, not the one holding the executable. The two differ whenever the app is
/// launched from a terminal, and reading the stale copy from the other folder has cost real
/// debugging time. Now they sit beside the application's own logs.
///
/// Note that `applog::purge_old` does NOT sweep these: it ages files by a leading `YYYY-MM-DD` in
/// the name, which channel files do not carry. They grow until deleted by hand, exactly as they did
/// in the working directory — being in `logs/` makes them findable, not self-limiting.
pub fn channel_line(file: &str, line: &str) {
    channel_lines(file, std::slice::from_ref(&line.to_string()));
}

/// Append several lines to a channel's file under ONE open, reporting whether they landed.
///
/// A channel that writes a table rather than an event uses this: one open per row put thousands of
/// blocking syscalls on the main thread the moment a switch was saved. The boolean exists for the
/// same kind of channel — one that must know whether to try again, rather than mark the subject
/// written and never return to it.
pub fn channel_lines(file: &str, lines: &[String]) -> bool {
    use std::io::Write;
    let path = channel_dir().join(file);
    let open = || {
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
    };
    // The ordinary path is one open. Creating the directory is the RECOVERY, attempted only when
    // that fails, so a channel left on all day does not pay a directory syscall per line.
    let handle = open().or_else(|_| {
        let _ = std::fs::create_dir_all(channel_dir());
        open()
    });
    let Ok(mut f) = handle else {
        return false;
    };
    let mut body = String::new();
    for line in lines {
        body.push_str(line);
        body.push('\n');
    }
    f.write_all(body.as_bytes()).is_ok()
}

/// Read the file and the environment, publish the result, and return it.
///
/// Call this FIRST, before the logger is installed: the log areas decide the logger's filter, so
/// they have to be known before it is built. Reading is safe that early because it neither creates
/// directories nor logs — a missing file simply yields defaults, which is the correct state for a
/// first launch. Creating the file is a separate, later step ([`ensure_file`]), because writing it
/// wants a logger that can report a failure.
///
/// A problem with the file cannot be reported here for that same reason, so it is returned for the
/// caller to log once the logger is up.
pub fn init() -> (DiagCfg, Option<String>) {
    let (cfg, err) = load();
    apply(&cfg);
    (cfg, err)
}

/// What the file on disk turned out to be. The three cases are kept apart because they call for
/// three different actions, and collapsing them is how a user's configuration gets destroyed.
enum FileState {
    /// No file: the ordinary first launch. Write the documented template.
    Absent,
    /// Read and parsed. Add any keys a newer build introduced.
    Parsed(DiagCfg),
    /// Present but unusable — unreadable, not UTF-8, or invalid TOML. Touch NOTHING.
    Unusable(String),
}

/// Read the file, without applying the environment or publishing anything.
fn read_file() -> FileState {
    let path = paths::diagnostics_path_no_create();
    match std::fs::read_to_string(&path) {
        Ok(text) => match config::parse(&text) {
            Ok(cfg) => FileState::Parsed(cfg),
            Err(e) => FileState::Unusable(format!(
                "diagnostics.toml: {e}; используются значения по умолчанию, файл не изменяется"
            )),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => FileState::Absent,
        // Present but unreadable: locked by an editor, denied by permissions, or saved as UTF-16 by
        // Notepad. Emphatically NOT the same as absent — treating it so would let `ensure_file`
        // write the template over a file full of the user's settings and comments.
        Err(e) => FileState::Unusable(format!(
            "diagnostics.toml не прочитан ({e}); используются значения по умолчанию, файл не изменяется"
        )),
    }
}

/// Read and parse, applying the environment on top. The second value is a problem to report.
fn load() -> (DiagCfg, Option<String>) {
    let path = paths::diagnostics_path_no_create();
    // Stat BEFORE reading. The other order records the new mtime against the old text whenever a
    // write lands between the two, and that edit is then invisible until the next one. This way the
    // worst case is re-reading the same content once.
    let (mtime, len) = stamp(&path);
    let (mut cfg, err) = match read_file() {
        FileState::Parsed(cfg) => (cfg, None),
        FileState::Absent => (DiagCfg::default(), None),
        FileState::Unusable(msg) => (DiagCfg::default(), Some(msg)),
    };
    config::apply_env(&mut cfg, config::env_lookup);
    LAST_MTIME_MS.store(mtime, Ordering::Relaxed);
    LAST_LEN.store(len, Ordering::Relaxed);
    (cfg, err)
}

/// Publish a configuration into the atomics every call site reads.
///
/// The log filter is NOT applied here: `init` runs before a logger exists, so swapping a filter
/// then would have nothing to swap. [`crate::applog::install`] takes the filter string instead, and
/// [`poll`] applies later changes.
fn apply(cfg: &DiagCfg) {
    RENDER.store(cfg.channels.render, Ordering::Relaxed);
    DETECT.store(cfg.channels.detect, Ordering::Relaxed);
    ASSETS.store(cfg.channels.assets, Ordering::Relaxed);
    MARKETS.store(cfg.channels.markets, Ordering::Relaxed);
    RING_LINES.store(cfg.limits.log_ring_lines, Ordering::Relaxed);
    BALANCE_WINDOW_SEC.store(cfg.limits.balance_repeat_window_sec, Ordering::Relaxed);
    MARKET_FLOOR_MS.store(cfg.limits.market_trace_min_interval_ms, Ordering::Relaxed);

    let selector = cfg.channels.orders.trim().to_string();
    let on = !selector.is_empty();
    // The flag drops BEFORE the selector is cleared and rises AFTER it is set, because an empty
    // selector means "follow everything" to `order_diag::follows_setting`. Clearing the string
    // first would leave a wide window in which the channel is still on and matching every market
    // on every core — the opposite of what switching it off asked for.
    //
    // This NARROWS that window rather than closing it: a reader that loads the flag and then takes
    // the lock is still two separate operations. What makes the residue harmless is that
    // `order_diag::line` re-checks `enabled()` before writing, so the worst case is a `follows()`
    // that answers `true` for a line which is then not written.
    if !on {
        ORDERS_ON.store(false, Ordering::Relaxed);
    }
    // Poison is recovered rather than skipped. Skipping the store and then raising the flag anyway
    // would leave the channel on with the PREVIOUS selector — and an empty one, the initial value,
    // means "follow everything on every core" to `order_diag::follows_setting`.
    {
        let mut guard = ORDERS_SELECTOR.write().unwrap_or_else(|e| e.into_inner());
        *guard = selector;
    }
    if on {
        ORDERS_ON.store(true, Ordering::Relaxed);
    }

    // Same order as the selector above, and for the same reason: the flag falls before the value
    // is cleared and rises after it is set, so the channel is never on with a selector that does
    // not belong to it. `record` re-checks the flag before writing, which is what makes the
    // remaining window harmless.
    let coins = cfg.channels.coin_naming.trim().to_string();
    let coins_on = !coins.is_empty();
    if !coins_on {
        COIN_NAMING_ON.store(false, Ordering::Relaxed);
    }
    {
        let mut guard = COIN_NAMING_SELECTOR
            .write()
            .unwrap_or_else(|e| e.into_inner());
        *guard = coins;
    }
    if coins_on {
        COIN_NAMING_ON.store(true, Ordering::Relaxed);
    }

    // Poison recovered, like the selector above: a skipped store here would leave `active()`
    // describing a configuration that is no longer in force, and `announce` reads it.
    {
        let mut guard = active_cell().lock().unwrap_or_else(|e| e.into_inner());
        *guard = cfg.clone();
    }
}

/// Mtime in unix milliseconds and length in bytes; zeroes when the file cannot be stated.
fn stamp(path: &std::path::Path) -> (u64, u64) {
    let Ok(meta) = std::fs::metadata(path) else {
        return (0, 0);
    };
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    (mtime, meta.len())
}

/// What, if anything, to write for a given file state — the whole decision, free of the filesystem
/// so every branch is testable. `None` means write nothing.
///
/// The `Unusable` branch is the one that matters: treating an unreadable file as absent is how a
/// user's settings and comments get replaced by a fresh template.
fn plan_write(state: &FileState, raw: Option<&str>) -> Option<(String, String)> {
    match state {
        FileState::Absent => Some((template::render(), "создан".to_string())),
        FileState::Parsed(_) => {
            let (merged, added) = upgrade::merge_missing(raw?)?;
            Some((merged, format!("добавлены новые параметры: {}", added.join(", "))))
        }
        // Never write over a file we could not read: whatever is in it is the user's, and a
        // template written on top would destroy their settings along with their comments.
        FileState::Unusable(_) => None,
    }
}

/// Create the documented file when absent, or add switches a newer build introduced.
///
/// Runs after the logger is installed, so its own failure is reportable.
///
/// An existing file is only ever ADDED to, never rewritten in place: values, ordering, spacing and
/// the user's own comments all survive, because the merge goes through `toml_edit` rather than a
/// serde round-trip. A file that is present but unusable — unreadable, not UTF-8, or invalid TOML —
/// is left completely alone; that case is the reason [`FileState`] exists.
pub fn ensure_file() {
    let raw = std::fs::read_to_string(paths::diagnostics_path_no_create()).ok();
    let Some((text, note)) = plan_write(&read_file(), raw.as_deref()) else {
        return;
    };
    // `diagnostics_path` rather than the no-create variant: by now creating `cfg/` is correct, and
    // its own warning can actually be logged.
    let path = paths::diagnostics_path();
    match crate::config::toml_io::write_atomic(&path, text.as_bytes(), "diagnostics.toml") {
        Ok(()) => {
            log::info!("{}: {note}", path.display());
            // Re-read what was just written and republish. Only the stamp matters in the ordinary
            // case (the added keys carry their defaults), but a file that became visible between
            // `init` and here would otherwise have its values ignored for the whole run.
            let (cfg, _) = load();
            apply(&cfg);
            crate::applog::set_filter(&filter_string(&cfg));
        }
        // Deliberately no stamp update on failure: adopting what is on disk as "already seen" would
        // hide a file another process changed in the meantime from every later poll.
        Err(e) => log::warn!("не удалось записать {}: {e:#}", path.display()),
    }
}

/// Re-read the file if it changed on disk, and apply the result.
///
/// Cheap enough to call once a second: one `stat` unless the file actually moved. Returns the new
/// configuration only when it DIFFERS from what is already published, so that a `touch`, or an
/// editor writing the same bytes back, does not announce a change that did not happen.
pub fn poll() -> Option<DiagCfg> {
    let path = paths::diagnostics_path_no_create();
    let (mtime, len) = stamp(&path);
    if mtime == LAST_MTIME_MS.load(Ordering::Relaxed) && len == LAST_LEN.load(Ordering::Relaxed) {
        return None;
    }
    let (cfg, err) = load();
    // Reported BEFORE the equality check, never after. A syntax error typed while everything is
    // off yields the defaults, which equal what is already active — so returning early first would
    // swallow the one message that explains why the edit did nothing, and the file's own header
    // promises it. The file changed on disk, so this is once per edit, not once per second.
    if let Some(e) = err {
        log::warn!("{e}");
    }
    let unchanged = active_cell()
        .lock()
        .map(|active| *active == cfg)
        .unwrap_or(false);
    if unchanged {
        return None;
    }
    apply(&cfg);
    crate::applog::set_filter(&filter_string(&cfg));
    Some(cfg)
}

/// The configuration currently in force, for a caller that needs the state AFTER startup settled.
///
/// Poison is recovered rather than defaulted. Returning `DiagCfg::default()` would report
/// "diagnostics off" to `announce` while channels were running — precisely the silence the
/// announcement exists to break.
pub fn active() -> DiagCfg {
    active_cell()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

/// Log which switches are active, or say that all are off.
///
/// Called at startup and after every change. Diagnostics have a cost, and a switch left on is
/// otherwise invisible — the file is not on screen, and the symptom (a huge log, a slower terminal)
/// looks like a defect rather than a setting. `warn` so it survives the default filter; the
/// all-off case is `info` rather than silence, because "I turned it off" deserves the same
/// confirmation as turning it on.
pub fn announce(cfg: &DiagCfg) {
    let path = paths::diagnostics_path_no_create();
    match cfg.active_summary() {
        Some(list) => log::warn!("диагностика включена: {list} (см. {})", path.display()),
        None => log::info!("диагностика выключена"),
    }
    // Said out loud, because the alternative is a switch that visibly reads as set and does
    // nothing — the one failure mode a settings file must never have. The reason comes from the
    // same function that made the decision, so the message cannot name a cause the user never hit.
    if let Some(reason) = filter::filter_rejection(&cfg.log.filter) {
        log::warn!(
            "log.filter «{}» игнорируется: {reason}",
            cfg.log.filter.trim()
        );
    }
}

#[cfg(test)]
mod tests;
