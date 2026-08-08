//! Application log: (1) a file log of raw core commands/reports in `logs/commands.log`
//! under the application data directory (beside the exe on Windows), used to diagnose
//! report SQL (INSERT/UPDATE) sent by the core; (2) a shared in-memory ring buffer for
//! the bottom dock's Log tab.
//!
//! Two sources populate the buffer: [`command`] (raw core report SQL) and [`TeeLogger`],
//! an `env_logger` wrapper that duplicates each accepted `log::` entry into the buffer
//! before passing it to env_logger for console/file output. Thread-safe for multiple feed
//! threads while the UI thread reads a snapshot.
//!
//! A core is named, never addressed, in the log. Each entry point redacts once — [`TeeLogger::log`]
//! for `log::` records, [`command`], [`parse_file_line`] for history read back from disk, and the
//! core stream at its source in `feed::live` — and [`DatedWriter::write`] scans again on the way to
//! a file, the sink whose contents outlive the process and get shared. [`panic_log`] owns the crash
//! file on the same terms.

pub mod redact;

use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use crate::config::paths;
use crate::util::now_unix_ms_i64 as now_ms_i64;

static LOG: OnceLock<Option<Mutex<std::fs::File>>> = OnceLock::new();

/// Maximum number of lines in the ring buffer; oldest lines are evicted.
const RING_CAP: usize = 5000;

/// Whether to write logs to `logs/<date>_<source>.log`. Defaults to on so early entries
/// are not lost before config loading; the application later applies the configured value.
static FILE_LOG: AtomicBool = AtomicBool::new(true);
/// Log-file retention period in days. 0 keeps everything. See [`purge_old`].
static RETENTION_DAYS: AtomicU32 = AtomicU32::new(14);

/// Applies file-logging settings from config, after loading and after saving settings.
pub fn set_file_logging(enabled: bool, retention_days: u32) {
    FILE_LOG.store(enabled, Ordering::Relaxed);
    RETENTION_DAYS.store(retention_days, Ordering::Relaxed);
}

/// Whether logs are currently written to files.
pub fn file_logging_enabled() -> bool {
    FILE_LOG.load(Ordering::Relaxed)
}

/// One log line for the Log tab.
#[derive(Clone)]
pub struct LogLine {
    /// UTC time in `YYYY-MM-DD HH:MM:SS.mmm` format.
    pub ts: String,
    pub level: log::Level,
    /// Source: the log entry's target, or `core.cmd` for raw core commands.
    /// Empty for core log lines because the selected server identifies the source.
    pub target: String,
    pub msg: String,
}

impl LogLine {
    /// Creates a core log line using unix time from ServerLog. The core has no level, so
    /// it uses Info; target is empty because the selected server identifies the source.
    pub fn core(unix_ms: i64, msg: String) -> Self {
        Self {
            ts: ts_from_unix_ms(unix_ms),
            level: log::Level::Info,
            target: String::new(),
            msg,
        }
    }

    /// Heuristically identifies an error line for the quick errors-only filter.
    /// Matches Warn/Error levels OR error-like words in the text, since level-less core
    /// logs must be filtered by content.
    pub fn is_errorish(&self) -> bool {
        if matches!(self.level, log::Level::Error | log::Level::Warn) {
            return true;
        }
        let m = self.msg.to_lowercase();
        [
            "error",
            "ошиб",
            "fail",
            "warn",
            "panic",
            "exception",
            "critical",
        ]
        .iter()
        .any(|k| m.contains(k))
    }
}

/// The Log tab's shared buffer: the lines it can still show, and how many have ever been added.
///
/// The count lives INSIDE the mutex with the lines it describes, because [`since`] answers "what
/// have I not seen" by subtracting a caller's cursor from it and then taking that many rows off the
/// tail. Were the two readable independently, a line pushed between the two reads would make the
/// count and the contents describe different instants, and the reader would skip the oldest unseen
/// line — silently, and only under load.
struct Ring {
    lines: VecDeque<LogLine>,
    /// Lines ever pushed, including those already evicted.
    pushed: u64,
}

static RING: OnceLock<Mutex<Ring>> = OnceLock::new();
/// Monotonic insertion counter for cheaply detecting new lines. The host forces a frame
/// while the Log tab is active. See [`revision`].
///
/// Deliberately outside the mutex: this one only has to CHANGE when lines arrive, and keeping it
/// out means a poisoned ring still moves it, so a panel watching it does not go silent as well.
/// Anything that needs the count and the rows to agree reads [`Ring::pushed`] instead.
static REVISION: AtomicU64 = AtomicU64::new(0);

fn ring() -> &'static Mutex<Ring> {
    RING.get_or_init(|| {
        Mutex::new(Ring {
            lines: VecDeque::with_capacity(RING_CAP),
            pushed: 0,
        })
    })
}

/// Global file writer for the local application log (`logs/<date>_app.log`).
static APP_WRITER: OnceLock<Mutex<DatedWriter>> = OnceLock::new();
fn app_writer() -> &'static Mutex<DatedWriter> {
    APP_WRITER.get_or_init(|| Mutex::new(DatedWriter::new("app")))
}

/// Adds a line to the ring buffer, evicting the oldest, and appends it to
/// `logs/<date>_app.log` when file logging is enabled.
///
/// Callers redact before building the line; [`DatedWriter::write`] scans again on the way to disk,
/// so a mistake here cannot reach a file.
fn push(line: LogLine) {
    if file_logging_enabled() {
        if let Ok(mut w) = app_writer().lock() {
            let date = line.ts.get(0..10).unwrap_or("");
            let hms = line.ts.get(11..).unwrap_or(line.ts.as_str());
            w.write(date, hms, level_code(line.level), &line.target, &line.msg);
            w.flush(); // Application logs are low-frequency, so flush immediately for disk visibility.
        }
    }
    if let Ok(mut r) = ring().lock() {
        if r.lines.len() >= RING_CAP {
            r.lines.pop_front();
        }
        r.lines.push_back(line);
        r.pushed = r.pushed.saturating_add(1);
    }
    REVISION.fetch_add(1, Ordering::Relaxed);
}

/// Buffer revision (number of insertions), used to detect new lines.
pub fn revision() -> u64 {
    REVISION.load(Ordering::Relaxed)
}

/// Lines pushed since `cursor`, oldest first, and the cursor to store for the next call.
///
/// The count and the rows are read together under one lock, so the difference is exactly what the
/// caller has not seen. When more lines arrived than the ring capacity, the overflow is already
/// evicted and the survivors come back — the same history loss a full re-read would show, rather
/// than a silent nothing. This is what lets the Log panel append instead of re-reading and
/// re-parsing the ring on every revision.
///
/// Args:
///     cursor: Value returned by the previous call, or 0 to read the whole ring.
///
/// Returns:
///     The unseen lines, oldest first, and the cursor for the next call. A poisoned ring yields no
///     lines and the caller's own cursor, so nothing is counted as delivered that was not.
pub fn since(cursor: u64) -> (Vec<LogLine>, u64) {
    ring()
        .lock()
        .map(|r| {
            let take = unseen(r.pushed, r.lines.len(), cursor);
            (
                r.lines.iter().skip(r.lines.len() - take).cloned().collect(),
                r.pushed,
            )
        })
        .unwrap_or_else(|_| (Vec::new(), cursor))
}

/// How many of the ring's newest lines a reader at `cursor` has not seen.
///
/// Split out so the three cases can be checked without driving the global ring: the ordinary
/// difference, a reader that missed more than the ring holds (the overflow is gone, so it gets the
/// survivors), and a cursor ahead of the counter, which means the ring was reset under it.
fn unseen(pushed: u64, len: usize, cursor: u64) -> usize {
    (pushed.saturating_sub(cursor) as usize).min(len)
}

/// Snapshot of the latest `max` lines, ordered oldest→newest, for the Log tab.
pub fn snapshot(max: usize) -> Vec<LogLine> {
    snapshot_with_cursor(max).0
}

/// [`snapshot`] plus the cursor that says everything in it has been seen.
///
/// One lock acquisition for both, which is the whole point: a reader that took the snapshot and
/// then asked for the cursor separately would mark a line pushed in between as already delivered
/// and never show it. Feed threads push here, so that window is real.
///
/// Args:
///     max: Most lines to return, newest kept.
///
/// Returns:
///     The lines oldest first, and the cursor to hand [`since`] next.
pub fn snapshot_with_cursor(max: usize) -> (Vec<LogLine>, u64) {
    ring()
        .lock()
        .map(|r| {
            let start = r.lines.len().saturating_sub(max);
            (r.lines.iter().skip(start).cloned().collect(), r.pushed)
        })
        .unwrap_or_else(|_| (Vec::new(), 0))
}

fn handle() -> Option<&'static Mutex<std::fs::File>> {
    LOG.get_or_init(|| {
        let dir = paths::logs_dir();
        if let Err(e) = std::fs::create_dir_all(&dir) {
            eprintln!("applog: не удалось создать {}: {e}", dir.display());
            return None;
        }
        let path = dir.join("commands.log");
        match OpenOptions::new().create(true).append(true).open(&path) {
            Ok(f) => {
                eprintln!("applog: команды пишутся в {}", path.display());
                Some(Mutex::new(f))
            }
            Err(e) => {
                eprintln!("applog: не удалось открыть {}: {e}", path.display());
                None
            }
        }
    })
    .as_ref()
}

fn ts() -> String {
    ts_from_unix_ms(now_ms_i64())
}

/// Converts unix ms to UTC `(YYYY-MM-DD, HH:MM:SS.mmm)` for the filename date and line time.
pub fn split_unix_ms(ms: i64) -> (String, String) {
    let secs = ms.div_euclid(1000);
    let frac = ms.rem_euclid(1000);
    let full = crate::db::fmt_unix_secs(secs); // "YYYY-MM-DD HH:MM:SS"
    let date = full.get(0..10).unwrap_or("").to_string();
    let time = full.get(11..).unwrap_or("");
    (date, format!("{time}.{frac:03}"))
}

/// Converts unix ms to `YYYY-MM-DD HH:MM:SS.mmm` in UTC.
pub fn ts_from_unix_ms(ms: i64) -> String {
    let (d, t) = split_unix_ms(ms);
    format!("{d} {t}")
}

/// Level code used in TSV files.
fn level_code(l: log::Level) -> &'static str {
    match l {
        log::Level::Error => "ERR",
        log::Level::Warn => "WARN",
        log::Level::Info => "INFO",
        log::Level::Debug => "DBG",
        log::Level::Trace => "TRC",
    }
}

fn level_from_code(s: &str) -> log::Level {
    match s {
        "ERR" => log::Level::Error,
        "WARN" => log::Level::Warn,
        "DBG" => log::Level::Debug,
        "TRC" => log::Level::Trace,
        _ => log::Level::Info,
    }
}

/// Converts a source label to a safe filename using letters, digits, hyphens, and underscores.
pub fn sanitize_label(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if s.is_empty() {
        "core".to_string()
    } else {
        s
    }
}

/// Daily-rotating file writer for one log source. Writes to `logs/<date>_<label>.log`
/// and reopens the file when the date changes. Line format is TSV:
/// `time \t level \t source \t message`. Used by both the local application log and
/// each core thread, with a separate instance per thread.
pub struct DatedWriter {
    label: String,
    date: String,
    file: Option<BufWriter<File>>,
}

impl DatedWriter {
    pub fn new(label: &str) -> Self {
        Self {
            label: sanitize_label(label),
            date: String::new(),
            file: None,
        }
    }

    /// Appends a line. If file logging is disabled, writes nothing and closes any open file.
    /// `date`=YYYY-MM-DD for the filename; `hms`=HH:MM:SS.mmm for the line.
    ///
    /// Network addresses are redacted here, so every file this writer owns — the application log
    /// and each core's log — is address-free regardless of what the caller passed.
    pub fn write(&mut self, date: &str, hms: &str, level: &str, target: &str, msg: &str) {
        if !file_logging_enabled() {
            self.file = None;
            return;
        }
        if self.file.is_none() || self.date != date {
            self.open(date);
        }
        if let Some(f) = self.file.as_mut() {
            // Remove line breaks from msg so one entry occupies one file line.
            let msg = redact::addresses(msg).replace(['\n', '\r'], " ");
            let _ = writeln!(f, "{hms}\t{level}\t{target}\t{msg}");
        }
    }

    fn open(&mut self, date: &str) {
        let dir = paths::logs_dir();
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join(format!("{date}_{}.log", self.label));
        self.file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .ok()
            .map(BufWriter::new);
        self.date = date.to_string();
    }

    pub fn flush(&mut self) {
        if let Some(f) = self.file.as_mut() {
            let _ = f.flush();
        }
    }
}

impl Drop for DatedWriter {
    fn drop(&mut self) {
        self.flush();
    }
}

/// Removes log files older than the retention period. For `<YYYY-MM-DD>_<label>.log`,
/// age comes from the filename date, which is more reliable than mtime. 0 days removes
/// nothing. Files without a leading date, such as `commands.log`, are left untouched.
pub fn purge_old() {
    let days = RETENTION_DAYS.load(Ordering::Relaxed);
    if days == 0 {
        return;
    }
    let cutoff = now_ms_i64().div_euclid(1000) - (days as i64) * 86_400;
    let dir = paths::logs_dir();
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return;
    };
    let mut removed = 0u32;
    for entry in rd.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.ends_with(".log") {
            continue;
        }
        let Some(date) = name.get(0..10) else {
            continue;
        };
        let Some(file_secs) = crate::db::parse_ymd(date) else {
            continue; // No leading date means this is not one of our rotated files.
        };
        if file_secs < cutoff && std::fs::remove_file(entry.path()).is_ok() {
            removed += 1;
        }
    }
    if removed > 0 {
        log::info!("лог: удалено старых файлов: {removed} (хранение {days} дн.)");
    }
}

/// Lists log files for a source label, newest first. The label is "app" for the local
/// log and the core name for a core log.
pub fn list_files(label: &str) -> Vec<String> {
    let suffix = format!("_{}.log", sanitize_label(label));
    let dir = paths::logs_dir();
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut v: Vec<String> = rd
        .flatten()
        .filter_map(|e| {
            let n = e.file_name().to_string_lossy().into_owned();
            n.ends_with(&suffix).then_some(n)
        })
        .collect();
    v.sort(); // A leading date makes lexicographic order equivalent to ascending date order.
    v.reverse(); // Newest first.
    v
}

/// Reads the latest `max` lines of history from a log file. Parses TSV; foreign or malformed
/// lines are stored whole in msg.
pub fn read_file(filename: &str, max: usize) -> Vec<LogLine> {
    let path = paths::logs_dir().join(filename);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    let date = filename.get(0..10).unwrap_or("");
    // Skip to the last `max` lines BEFORE parsing: a day-sized core log holds far more lines than
    // the view keeps, and parsing (with its redaction scan) the discarded ones runs on the UI thread.
    let total = text.lines().count();
    text.lines()
        .skip(total.saturating_sub(max))
        .map(|l| parse_file_line(date, l))
        .collect()
}

/// Name of the rotated log file a source writes on one UTC date, as [`DatedWriter::open`] spells it.
///
/// Args:
///     date: UTC day as `YYYY-MM-DD`, from [`split_unix_ms`].
///     label: Source label, `"app"` for the local log or the core name for a core log.
///
/// Returns:
///     The bare file name inside `logs/`, without a directory.
pub fn file_name(date: &str, label: &str) -> String {
    format!("{date}_{}.log", sanitize_label(label))
}

/// Marker a core writes on every line of one trading task: its number in parentheses.
///
/// This spelling is the only link between a stored trade and its log — the wire protocol carries no
/// identifier on a log line — so it lives here, next to the file format it is searched in.
pub fn task_marker(task_id: i64) -> String {
    format!("({task_id})")
}

/// Result of scanning one rotated log file for lines containing a marker.
pub struct ScanResult {
    /// Matching lines in file order, oldest first, at most the requested cap.
    pub lines: Vec<LogLine>,
    /// Whether the cap stopped the scan before the end of the file, so later matches are missing.
    pub truncated: bool,
}

/// Collects lines of one rotated log file whose raw text contains `needle`, oldest first.
///
/// Reads incrementally rather than through [`read_file`]: a day of one core's log reaches tens of
/// megabytes, and a caller looking for one trade wants a handful of lines out of it. Parsing (with
/// its redaction scan) runs only for the lines that matched. A missing or unreadable file yields no
/// lines, which is the ordinary case for a day the core was not running or whose file retention
/// already removed.
///
/// Args:
///     filename: Bare file name inside `logs/`, from [`file_name`] or [`list_files`].
///     needle: Raw substring to look for, matched against the stored line before parsing.
///     max: Maximum number of matches to return.
///
/// Returns:
///     The matches in file order plus whether `max` cut the scan short.
pub fn scan_file(filename: &str, needle: &str, max: usize) -> ScanResult {
    let path = paths::logs_dir().join(filename);
    let file = match File::open(&path) {
        Ok(file) => file,
        Err(e) => {
            // A day the core did not run, or one retention already removed, is the ordinary case and
            // says nothing. Anything else — a permission or sharing error — is a fact about this
            // machine that the empty result cannot express, so it goes to the log.
            if e.kind() != std::io::ErrorKind::NotFound {
                log::warn!("лог: не удалось открыть {}: {e}", path.display());
            }
            return ScanResult {
                lines: Vec::new(),
                truncated: false,
            };
        }
    };
    let date = filename.get(0..10).unwrap_or("");
    scan_reader(std::io::BufReader::new(file), date, needle, max)
}

/// Selects and parses matching lines from an already opened log reader.
///
/// Split from [`scan_file`] so the selection rule is testable without touching the filesystem.
/// A read or decoding error ends the scan and marks the result truncated: the writer only ever
/// emits UTF-8, so a failure means the file is damaged or not one of ours, and what was collected
/// so far is a partial answer — reporting it as complete would be a lie the caller cannot detect.
fn scan_reader<R: std::io::BufRead>(reader: R, date: &str, needle: &str, max: usize) -> ScanResult {
    let mut lines = Vec::new();
    for line in reader.lines() {
        let Ok(line) = line else {
            return ScanResult {
                lines,
                truncated: true,
            };
        };
        if !line.contains(needle) {
            continue;
        }
        if lines.len() == max {
            return ScanResult {
                lines,
                truncated: true,
            };
        }
        lines.push(parse_file_line(date, &line));
    }
    ScanResult {
        lines,
        truncated: false,
    }
}

/// Parses one stored line. Files written by earlier builds still hold addresses, and the Log tab
/// reads them back into the same view (and clipboard) as live lines, so redact on the way in.
fn parse_file_line(date: &str, l: &str) -> LogLine {
    let mut it = l.splitn(4, '\t');
    match (it.next(), it.next(), it.next(), it.next()) {
        (Some(hms), Some(lv), Some(tg), Some(m)) => LogLine {
            ts: format!("{date} {hms}"),
            level: level_from_code(lv),
            target: tg.to_string(),
            msg: redact::addresses(m).into_owned(),
        },
        _ => LogLine {
            ts: date.to_string(),
            level: log::Level::Info,
            target: String::new(),
            msg: redact::addresses(l).into_owned(),
        },
    }
}

/// Writes a command line with UTC time to `logs/commands.log` and to the Log tab's
/// in-memory buffer.
pub fn command(line: &str) {
    let ts = ts();
    let line = redact::addresses(line);
    if file_logging_enabled() {
        if let Some(m) = handle() {
            if let Ok(mut f) = m.lock() {
                let _ = writeln!(f, "[{ts}] {line}");
            }
        }
    }
    push(LogLine {
        ts,
        level: log::Level::Info,
        target: "core.cmd".to_string(),
        msg: line.into_owned(),
    });
}

/// Installs `env` as the global logger, wrapped in [`TeeLogger`].
///
/// The wrapper is what redacts and what feeds the Log tab, so nothing may install the inner logger
/// directly. Returns the error from `log::set_boxed_logger` when a logger is already installed.
pub fn install(env: env_logger::Logger) -> Result<(), log::SetLoggerError> {
    log::set_max_level(env.filter());
    log::set_boxed_logger(Box::new(TeeLogger::new(env)))
}

/// Appends a crash report to `panic.log`, redacting addresses first.
///
/// The panic hook and the native crash handler both write here; owning the sink in one place keeps
/// the file's path and its redaction from being restated at each call site.
pub fn panic_log(line: &str) {
    let line = redact::addresses(line);
    if let Ok(mut f) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(paths::panic_log())
    {
        let _ = writeln!(f, "{line}");
    }
}

/// Runs `f` on a copy of `src` whose message is `msg`.
///
/// `format_args!` produces a temporary that cannot outlive the statement building the record, so a
/// rebuilt record can only be handed to a callback, never returned or stored. The fields are copied
/// by hand because `Record::to_builder` needs log's `kv` feature, which this tree does not enable.
fn with_redacted_record<R>(msg: &str, src: &log::Record, f: impl FnOnce(&log::Record) -> R) -> R {
    f(&log::Record::builder()
        .metadata(src.metadata().clone())
        .args(format_args!("{msg}"))
        .module_path(src.module_path())
        .file(src.file())
        .line(src.line())
        .build())
}

/// Logger wrapper that duplicates emitted entries into the ring buffer and delegates
/// formatting/output to the inner `env_logger`. Installed globally in `main`.
pub struct TeeLogger {
    inner: env_logger::Logger,
}

impl TeeLogger {
    pub fn new(inner: env_logger::Logger) -> Self {
        Self { inner }
    }
}

impl log::Log for TeeLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        self.inner.enabled(metadata)
    }

    fn log(&self, record: &log::Record) {
        // Level/module gate first: formatting the message for a record env_logger will drop is the
        // dependency trace noise this wrapper exists to keep out of the buffer.
        if !self.inner.enabled(record.metadata()) {
            return;
        }
        // Format ONCE. Dependencies such as moonproto format addresses into their own records, so
        // the console and file copy is rebuilt from this text rather than forwarding the original:
        // re-formatting it would run the arguments' `Display` a second time, and an argument whose
        // value moved in between (a core renamed mid-line) would print differently in each copy.
        let raw = record.args().to_string();
        // The `Cow` borrows `raw`, so reduce it before reusing `raw` itself on the clean path —
        // that keeps a line without an address at exactly one allocation.
        let masked = match redact::addresses(&raw) {
            std::borrow::Cow::Borrowed(_) => None,
            std::borrow::Cow::Owned(text) => Some(text),
        };
        let msg = masked.unwrap_or(raw);
        // Both copies are decided on the SAME text: env_logger's filter can match on the message
        // body, so testing one text and emitting another would let the two disagree.
        let buffered = with_redacted_record(&msg, record, |rebuilt| {
            let matched = self.inner.matches(rebuilt);
            self.inner.log(rebuilt);
            matched
        });
        if buffered {
            push(LogLine {
                ts: ts(),
                level: record.level(),
                target: record.target().to_string(),
                msg,
            });
        }
    }

    fn flush(&self) {
        self.inner.flush();
    }
}

#[cfg(test)]
/// Tests for log-file scanning and stored-line parsing.
mod tests;
