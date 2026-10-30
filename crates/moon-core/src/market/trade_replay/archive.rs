//! Public daily trade archives and their offline CSV decoders.

mod csv;
mod day_cache;
mod transport;

use std::collections::HashMap;
use std::io::{BufReader, Read};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use super::Coverage;
use super::gate::ReplayGate;
use super::venue_caps::{ArchiveFormat, ArchiveRoute};
use crate::feed::types::Tick;
use chrono::NaiveDate;

// Reserve room for tape filing after the compressed day has been extracted.
const EXTRACT_HEADROOM: u64 = 4;
static DISK_GUARD: Mutex<Option<fn() -> Option<u64>>> = Mutex::new(None);

/// Retry conditions for paid body failures and definitive missing daily files.
enum DayLatch {
    Missing,
    Disk { available: u64 },
    Transfer { error: ArchiveError, until: Instant },
}

/// Failed bodies share a per-day allowance until their transfer cooldown expires.
#[derive(Default)]
struct DayFailures {
    attempts: u8,
    latched: Option<DayLatch>,
    /// Deadline after which this resolved path and its preserved retry allowance both expire.
    resolved: Option<Instant>,
}

/// Per-day retry state plus a time-ordered index of resolutions awaiting bounded retention.
#[derive(Default)]
struct FailureBook {
    days: HashMap<PathBuf, DayFailures>,
    resolved: std::collections::BTreeMap<Instant, std::collections::HashSet<PathBuf>>,
}

static DAY_FAILURES: OnceLock<Mutex<FailureBook>> = OnceLock::new();

/// Remove a path's old resolution index without touching its retry allowance.
fn unindex_resolution(book: &mut FailureBook, path: &std::path::Path) {
    let Some(expiry) = book.days.get(path).and_then(|day| day.resolved) else {
        return;
    };
    let remove = book.resolved.get_mut(&expiry).is_some_and(|paths| {
        paths.remove(path);
        paths.is_empty()
    });
    if remove {
        book.resolved.remove(&expiry);
    }
}

/// Return a failure entry with any previous resolution cleared, preserving its attempt count.
fn failure_mut<'a>(book: &'a mut FailureBook, path: &std::path::Path) -> &'a mut DayFailures {
    unindex_resolution(book, path);
    let day = book.days.entry(path.to_owned()).or_default();
    day.resolved = None;
    day
}

/// Expire only resolutions whose 24-hour retry-preservation deadline has passed.
fn expire_resolved(book: &mut FailureBook, now: Instant) {
    while let Some((&expiry, _)) = book.resolved.first_key_value() {
        if expiry > now {
            break;
        }
        let paths = book.resolved.remove(&expiry).unwrap_or_default();
        for path in paths {
            if book
                .days
                .get(&path)
                .is_some_and(|day| day.resolved == Some(expiry))
            {
                book.days.remove(&path);
            }
        }
    }
}

/// Retry failed bodies after a cooldown or after the disk budget has grown.
fn day_refusal(
    path: &std::path::Path,
    guard: &impl Fn() -> Option<u64>,
    now: Instant,
) -> Option<ArchiveError> {
    let mut book = DAY_FAILURES
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    expire_resolved(&mut book, now);
    let day = book.days.get_mut(path)?;
    let (error, retry) = match day.latched.as_ref()? {
        DayLatch::Missing => return Some(ArchiveError::UnknownSymbol),
        DayLatch::Disk { available } => (
            ArchiveError::NoRoom,
            guard().is_some_and(|room| room > *available),
        ),
        DayLatch::Transfer { error, until } => (error.clone(), now >= *until),
    };
    if !retry {
        return Some(error);
    }
    if matches!(day.latched, Some(DayLatch::Transfer { .. })) {
        book.days.remove(path);
    } else {
        // More disk room releases the latch but never renews the failed-body allowance.
        day.latched = None;
    }
    None
}

/// Cache definitive 404s; suppress only failures that actually received body bytes.
fn note_day_result(
    path: PathBuf,
    result: &Result<Coverage, ArchiveError>,
    received: u64,
    available: Option<u64>,
    now: Instant,
) {
    let mut book = DAY_FAILURES
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    expire_resolved(&mut book, now);
    if result.is_ok() {
        // Success resolves the path but does not renew its failed-body allowance.
        unindex_resolution(&mut book, &path);
        let expiry = now + Duration::from_secs(24 * 60 * 60);
        if let Some(day) = book.days.get_mut(&path) {
            day.resolved = Some(expiry);
            book.resolved.entry(expiry).or_default().insert(path);
        }
        return;
    }
    let Err(error) = result else {
        return;
    };
    match error {
        ArchiveError::UnknownSymbol => {
            failure_mut(&mut book, &path).latched = Some(DayLatch::Missing);
        }
        ArchiveError::NoRoom if received > 0 => {
            if let Some(available) = available {
                failure_mut(&mut book, &path).latched = Some(DayLatch::Disk { available });
            }
        }
        ArchiveError::Transient(_) | ArchiveError::Corrupt(_) if received > 0 => {
            let day = failure_mut(&mut book, &path);
            day.attempts = day.attempts.saturating_add(1);
            if day.attempts >= 3 {
                day.latched = Some(DayLatch::Transfer {
                    error: error.clone(),
                    until: now + Duration::from_secs(24 * 60 * 60),
                });
            }
        }
        _ => {}
    }
}

thread_local! { static PACING: std::cell::Cell<Option<fn()>> = const { std::cell::Cell::new(None) }; }

/// Scope pacing to the catch-up thread; terminal archive requests retain their own policy.
pub(crate) fn with_pacing<T>(pace: fn(), run: impl FnOnce() -> T) -> T {
    /// Restore the previous hook even if a decoder unwinds.
    struct Restore(Option<fn()>);
    impl Drop for Restore {
        /// The hook belongs only to this recovery operation.
        fn drop(&mut self) {
            PACING.with(|slot| slot.set(self.0));
        }
    }
    let _restore = Restore(PACING.with(|slot| slot.replace(Some(pace))));
    run()
}

/// Poll the optional caller's pacing between bounded download and decode operations.
pub(super) fn pace() {
    PACING.with(|slot| {
        if let Some(pace) = slot.get() {
            pace();
        }
    });
}
/// Host locks remain reusable across fills and do not serialize distinct archive services.
type DownloadLocks = Mutex<HashMap<&'static str, Arc<Mutex<()>>>>;
static DOWNLOAD: OnceLock<DownloadLocks> = OnceLock::new();

/// Completed days survive later refusals; stopping errors take precedence over skipped days.
#[derive(Clone, Debug)]
pub struct ArchiveFill {
    pub covered: Coverage,
    pub ticks: Vec<Tick>,
    pub deferred: Option<ArchiveError>,
}

/// Serialize downloads per archive host without blocking unrelated venues.
fn download_lock(host: &'static str) -> Arc<Mutex<()>> {
    DOWNLOAD
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .entry(host)
        .or_default()
        .clone()
}

/// Install a spendable-byte snapshot probe; None means the host cannot preserve its reserve.
pub fn set_disk_guard(guard: fn() -> Option<u64>) {
    *DISK_GUARD.lock().unwrap_or_else(|error| error.into_inner()) = Some(guard);
}

/// Locate temporary archive storage beneath the configured data root.
pub fn day_dir() -> PathBuf {
    crate::config::paths::data_dir().join("tape_archive")
}

/// Ask the installed reserve check for extraction and subsequent tape filing headroom.
pub(crate) fn room_for(bytes: u64) -> bool {
    disk_budget().is_some_and(|available| bytes.saturating_mul(EXTRACT_HEADROOM) <= available)
}

/// Snapshot spendable bytes once; an unavailable installed measurement fails closed.
fn disk_budget() -> Option<u64> {
    let guard = *DISK_GUARD.lock().unwrap_or_else(|error| error.into_inner());
    guard.map_or(Some(u64::MAX), |guard| guard())
}

/// Preserve the same reserve as the station on the terminal's configured data filesystem.
pub fn data_disk_budget() -> Option<u64> {
    let root = crate::config::paths::data_dir();
    let path = root.ancestors().find(|path| path.is_dir())?;
    let (free, total) = crate::disk_reserve::disk_space(path)?;
    crate::disk_reserve::spendable(free, total)
}

/// Streaming response boundary; tests provide readers instead of making HTTP requests.
struct DayResponse {
    status: u16,
    retry_after_s: Option<u32>,
    content_length: Option<u64>,
    reader: Box<dyn Read>,
}

/// Fetch a daily body with a dedicated long-lived HTTPS client and no body buffering.
fn fetch_day(url: &str) -> Result<DayResponse, ArchiveError> {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    let agent = AGENT.get_or_init(|| {
        use ureq::unversioned::transport::{Connector, DefaultConnector};
        ureq::Agent::with_parts(
            ureq::Agent::config_builder()
                .timeout_connect(Some(Duration::from_secs(30)))
                .https_only(true)
                .http_status_as_error(false)
                .build(),
            DefaultConnector::default().chain(transport::ReadTimeout),
            ureq::unversioned::resolver::DefaultResolver::default(),
        )
    });
    let response = agent.get(url).call().map_err(transient)?;
    let status = response.status().as_u16();
    let retry_after_s = response
        .headers()
        .get("retry-after")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse().ok());
    let (_, body) = response.into_parts();
    let content_length = body.content_length();
    Ok(DayResponse {
        status,
        retry_after_s,
        content_length,
        reader: Box::new(body.into_reader()),
    })
}

/// Fill published UTC days, retaining only requested ticks and deleting every downloaded file.
pub fn fill(
    route: ArchiveRoute,
    market: &str,
    spans: &Coverage,
    gate: &ReplayGate,
) -> Result<ArchiveFill, ArchiveError> {
    fill_counted(route, market, spans, gate, u64::MAX).0
}

/// Return only this call's received bytes, including bodies that fail before any day completes.
pub(crate) fn fill_counted(
    route: ArchiveRoute,
    market: &str,
    spans: &Coverage,
    gate: &ReplayGate,
    budget: u64,
) -> (Result<ArchiveFill, ArchiveError>, u64) {
    let bytes = Arc::new(AtomicU64::new(0));
    let now_ms = crate::util::time::now_unix_ms_i64();
    let pending = publication_pending(route, spans, now_ms);
    let result = fill_with(
        route,
        market,
        spans,
        gate,
        (
            &day_dir(),
            utc_days_at(spans, now_ms, route.publish_lag_ms()),
        ),
        (disk_budget, budget),
        |url| {
            let mut response = fetch_day(url)?;
            response.reader = Box::new(CountedRead {
                reader: response.reader,
                bytes: bytes.clone(),
            });
            Ok(response)
        },
    );
    let result = result.and_then(|mut fill| {
        if pending.is_some() && fill.deferred.is_none() {
            if fill.covered.is_empty() {
                return Err(ArchiveError::NotPublished);
            }
            fill.deferred = Some(ArchiveError::NotPublished);
        }
        Ok(fill)
    });
    (result, bytes.load(Ordering::Relaxed))
}

/// Earliest future daily publication touched by the uncovered focus, using the route's lag.
pub(crate) fn publication_pending(
    route: ArchiveRoute,
    spans: &Coverage,
    now_ms: i64,
) -> Option<i64> {
    const DAY_MS: i64 = 86_400_000;
    spans
        .spans()
        .iter()
        .filter_map(|&(from, to)| {
            let first = from.div_euclid(DAY_MS);
            let last = to.div_euclid(DAY_MS);
            let first_pending = now_ms
                .saturating_sub(route.publish_lag_ms())
                .div_euclid(DAY_MS);
            let day = first.max(first_pending);
            (day <= last).then(|| {
                day.saturating_add(1)
                    .saturating_mul(DAY_MS)
                    .saturating_add(route.publish_lag_ms())
            })
        })
        .min()
}

/// Retry a late daily file on the next UTC hour, matching recorder catch-up's `next_pass` policy.
pub(crate) fn next_retry_at(now_ms: i64) -> i64 {
    const HOUR_MS: i64 = 3_600_000;
    now_ms
        .div_euclid(HOUR_MS)
        .saturating_add(1)
        .saturating_mul(HOUR_MS)
}

/// Account compressed transport bytes locally without charging another caller's download.
struct CountedRead {
    reader: Box<dyn Read>,
    bytes: Arc<AtomicU64>,
}
impl Read for CountedRead {
    /// Count each successfully received chunk even if extraction or later transport reads fail.
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let received = self.reader.read(buffer)?;
        self.bytes.fetch_add(received as u64, Ordering::Relaxed);
        Ok(received)
    }
}

/// Execute the archive pipeline with injectable transport, clock, root and disk guard.
fn fill_with(
    route: ArchiveRoute,
    market: &str,
    spans: &Coverage,
    gate: &ReplayGate,
    storage: (&std::path::Path, Vec<NaiveDate>),
    limits: (impl Fn() -> Option<u64>, u64),
    fetch: impl Fn(&str) -> Result<DayResponse, ArchiveError>,
) -> Result<ArchiveFill, ArchiveError> {
    fill_at(
        route,
        market,
        spans,
        gate,
        (storage.0, storage.1, Instant::now),
        limits,
        fetch,
    )
}

/// Inject the pass clock so failure expiry can be checked without sleeping.
fn fill_at(
    route: ArchiveRoute,
    market: &str,
    spans: &Coverage,
    gate: &ReplayGate,
    storage: (&std::path::Path, Vec<NaiveDate>, impl Fn() -> Instant),
    limits: (impl Fn() -> Option<u64>, u64),
    fetch: impl Fn(&str) -> Result<DayResponse, ArchiveError>,
) -> Result<ArchiveFill, ArchiveError> {
    let mut covered = Coverage::none();
    let mut ticks = Vec::new();
    let mut deferred = None;
    let requested = storage.1.len();
    let mut unknown = 0;
    let (guard, mut budget) = limits;
    let (root, days, now) = storage;
    day_cache::sweep_once(root).map_err(transient)?;
    for day in days {
        let before = ticks.len();
        let mut latched = false;
        let mut available = None;
        let day_bytes = Arc::new(AtomicU64::new(0));
        let paths = day_cache::paths(root, route, market, day)?;
        let day_path = paths.file.clone();
        let key = day_cache::in_flight(&day_path);
        let _key = key.lock();
        let result = (|| {
            if let Some(error) = day_refusal(&paths.file, &guard, now()) {
                latched = true;
                return Err(error);
            }
            let cleanup = day_cache::Cleanup::new(paths);
            // Preserve the host reserve before HTTP without guessing the day's size.
            let snapshot = guard().ok_or(ArchiveError::NoRoom)?;
            available = Some(snapshot);
            let fits = |bytes: u64| bytes.saturating_mul(EXTRACT_HEADROOM) <= snapshot;
            pace();
            {
                let download = download_lock(route.host());
                let _download = download.lock().unwrap_or_else(|error| error.into_inner());
                let asked_at = gate.pace_at(route.host(), route.page_interval());
                gate.claim(route.host(), Instant::now())
                    .map_err(|seconds| ArchiveError::Throttled {
                        retry_after_s: Some(seconds),
                    })?;
                let response = fetch(&route.day_url(market, day)).inspect_err(|_| {
                    gate.refuse(route.host(), Instant::now());
                })?;
                match response.status {
                    404 => {
                        return Err(missing_day(
                            route,
                            day,
                            crate::util::time::now_unix_ms_i64(),
                        ));
                    }
                    418 | 429 => {
                        match response.retry_after_s {
                            Some(seconds) => {
                                gate.honour_retry_after(route.host(), Instant::now(), seconds)
                            }
                            None => gate.refuse(route.host(), Instant::now()),
                        }
                        return Err(ArchiveError::Throttled {
                            retry_after_s: response.retry_after_s,
                        });
                    }
                    200..=299 => gate.clear(route.host(), asked_at),
                    status => {
                        gate.refuse(route.host(), Instant::now());
                        return Err(ArchiveError::Http(status));
                    }
                }
                if response
                    .content_length
                    .is_some_and(|length| length > budget)
                {
                    return Err(ArchiveError::BudgetDeferred);
                }
                if response.content_length.is_some_and(|length| !fits(length)) {
                    return Err(ArchiveError::NoRoom);
                }
                std::fs::create_dir_all(cleanup.file().parent().expect("route directory"))
                    .map_err(transient)?;
                let mut file = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(cleanup.part())
                    .map_err(transient)?;
                let expected = response.content_length;
                let copied = copy_bounded(
                    CountedRead {
                        reader: response.reader,
                        bytes: day_bytes.clone(),
                    },
                    &mut file,
                    budget,
                    expected,
                    fits,
                );
                budget = budget.saturating_sub(day_bytes.load(Ordering::Relaxed));
                let received = match copied {
                    Ok(received) => received,
                    Err(ArchiveError::NoRoom) => return Err(ArchiveError::NoRoom),
                    Err(error) => {
                        gate.refuse(route.host(), Instant::now());
                        return Err(error);
                    }
                };
                if expected.is_some_and(|length| length != received) {
                    gate.refuse(route.host(), Instant::now());
                    return Err(ArchiveError::Transient("short archive body".into()));
                }
                drop(file);
                std::fs::rename(cleanup.part(), cleanup.file()).map_err(transient)?;
            }
            let start = day
                .and_hms_opt(0, 0, 0)
                .expect("UTC midnight")
                .and_utc()
                .timestamp_millis();
            let inside = spans.clip(&Coverage::one((start, start + 86_400_000 - 1)));
            extract(
                route,
                std::fs::File::open(cleanup.file()).map_err(transient)?,
                &inside,
                &mut ticks,
            )?;
            Ok(inside)
        })();
        if !latched {
            note_day_result(
                day_path,
                &result,
                day_bytes.load(Ordering::Relaxed),
                available,
                now(),
            );
        }
        match result {
            Ok(inside) => {
                for &span in inside.spans() {
                    covered.add(span);
                }
            }
            Err(error) => {
                ticks.truncate(before);
                let skip = latched
                    || matches!(
                        error,
                        ArchiveError::NotPublished
                            | ArchiveError::UnknownSymbol
                            | ArchiveError::Corrupt(_)
                    );
                if error == ArchiveError::UnknownSymbol {
                    unknown += 1;
                }
                if deferred.is_none() || !skip {
                    deferred = Some(error);
                }
                if !skip {
                    break;
                }
            }
        }
    }
    ticks.sort_by(|a, b| a.time_ms.total_cmp(&b.time_ms));
    // UnknownSymbol describes a whole request only when every requested daily file was missing.
    if unknown > 0 && unknown != requested && deferred == Some(ArchiveError::UnknownSymbol) {
        deferred = Some(ArchiveError::NotPublished);
    }
    if covered.is_empty() {
        if let Some(error) = deferred {
            return Err(error);
        }
    }
    Ok(ArchiveFill {
        covered,
        ticks,
        deferred,
    })
}

/// Classify a daily 404 by publication age before caching it for this process.
fn missing_day(route: ArchiveRoute, day: NaiveDate, now_ms: i64) -> ArchiveError {
    let end = day
        .and_hms_opt(0, 0, 0)
        .expect("UTC midnight")
        .and_utc()
        .timestamp_millis()
        .saturating_add(86_400_000);
    if now_ms.saturating_sub(end) > route.publish_lag_ms() + 7 * 86_400_000 {
        ArchiveError::UnknownSymbol
    } else {
        ArchiveError::NotPublished
    }
}

/// Check the captured headroom every 64 MiB and at EOF without probing free space per chunk.
fn copy_bounded(
    mut reader: impl Read,
    writer: &mut impl std::io::Write,
    budget: u64,
    expected: Option<u64>,
    guard: impl Fn(u64) -> bool,
) -> Result<u64, ArchiveError> {
    let mut total = 0_u64;
    let limit = budget;
    let mut buffer = [0_u8; 64 * 1024];
    const CHECK_EVERY: u64 = 64 * 1024 * 1024;
    let mut next_check = CHECK_EVERY;
    if !guard(0) {
        return Err(ArchiveError::NoRoom);
    }
    loop {
        if total == limit {
            return if expected == Some(total) {
                Ok(total)
            } else {
                Err(ArchiveError::BudgetDeferred)
            };
        }
        let available = (limit - total).min(buffer.len() as u64) as usize;
        let count = reader.read(&mut buffer[..available]).map_err(transient)?;
        if count == 0 {
            return if guard(total) {
                Ok(total)
            } else {
                Err(ArchiveError::NoRoom)
            };
        }
        total += count as u64;
        if total >= next_check {
            if !guard(total) {
                return Err(ArchiveError::NoRoom);
            }
            next_check = total.saturating_add(CHECK_EVERY);
        }
        writer.write_all(&buffer[..count]).map_err(transient)?;
    }
}

/// Stream the compression envelope into the venue parser without materializing the whole day.
fn extract(
    route: ArchiveRoute,
    file: impl Read + std::io::Seek,
    spans: &Coverage,
    out: &mut Vec<Tick>,
) -> Result<(), ArchiveError> {
    pace();
    match route.format() {
        ArchiveFormat::ZipCsv => {
            let mut archive = zip::ZipArchive::new(file).map_err(corrupt)?;
            let csv = PacedRead::new(archive.by_index(0).map_err(corrupt)?);
            csv::parse_binance_agg(BufReader::new(csv), spans, out).map_err(decode_error)?;
        }
        ArchiveFormat::GzCsv => {
            csv::parse_bybit_trading(
                BufReader::new(PacedRead::new(flate2::read::MultiGzDecoder::new(file))),
                spans,
                out,
            )
            .map_err(decode_error)?;
        }
    }
    Ok(())
}

/// Pace decompression and parsing by decoded bytes without holding a download host lock.
struct PacedRead<R> {
    reader: R,
    bytes: usize,
}

impl<R> PacedRead<R> {
    /// Start a fresh decode budget for each daily archive.
    fn new(reader: R) -> Self {
        Self { reader, bytes: 0 }
    }
}

impl<R: Read> Read for PacedRead<R> {
    /// Bound CPU work to roughly one megabyte of decoded CSV between pacing samples.
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        if self.bytes >= 1_048_576 {
            pace();
            self.bytes = 0;
        }
        let len = buffer.len().min(64 * 1024);
        let received = self.reader.read(&mut buffer[..len])?;
        self.bytes += received;
        Ok(received)
    }
}

/// Compression reader failures describe a damaged published file, not HTTP transport.
fn decode_error(error: ArchiveError) -> ArchiveError {
    match error {
        ArchiveError::Transient(message) => ArchiveError::Corrupt(message),
        other => other,
    }
}

/// Preserve compression diagnostics while classifying damaged content permanently.
fn corrupt(error: impl std::fmt::Display) -> ArchiveError {
    ArchiveError::Corrupt(error.to_string())
}

/// Preserve transport and filesystem diagnostics for a later retry.
fn transient(error: impl std::fmt::Display) -> ArchiveError {
    ArchiveError::Transient(error.to_string())
}

/// Enumerate published days using the selected archive's publication lag.
pub(crate) fn utc_days(route: ArchiveRoute, spans: &Coverage) -> Vec<NaiveDate> {
    utc_days_at(
        spans,
        crate::util::time::now_unix_ms_i64(),
        route.publish_lag_ms(),
    )
}

/// Enumerate unique UTC dates touched by spans, excluding days whose publication lag remains.
fn utc_days_at(spans: &Coverage, now_ms: i64, lag_ms: i64) -> Vec<NaiveDate> {
    let mut days = std::collections::BTreeSet::new();
    for &(from, to) in spans.spans() {
        let Some(mut day) =
            chrono::DateTime::from_timestamp_millis(from).map(|time| time.date_naive())
        else {
            continue;
        };
        let Some(last) = chrono::DateTime::from_timestamp_millis(to).map(|time| time.date_naive())
        else {
            continue;
        };
        while day <= last {
            let end = day
                .and_hms_opt(0, 0, 0)
                .expect("UTC midnight")
                .and_utc()
                .timestamp_millis()
                .saturating_add(86_400_000);
            if end.saturating_add(lag_ms) > now_ms {
                break;
            }
            days.insert(day);
            let Some(next) = day.succ_opt() else {
                break;
            };
            day = next;
        }
    }
    days.into_iter().collect()
}

/// Why a public archive cannot currently fill the requested tape window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ArchiveError {
    /// The daily archive has not been published yet.
    NotPublished,
    /// The venue does not recognize this market.
    UnknownSymbol,
    /// The host refused requests until its header wait or shared backoff expires.
    Throttled { retry_after_s: Option<u32> },
    /// Downloading or filing the archive would cross the host's disk reserve.
    NoRoom,
    /// The current pass cannot admit this response body; replay callers have no pass budget.
    BudgetDeferred,
    /// The archive host answered with this unexpected HTTP status; retried like a transport
    /// failure, kept apart only so the station can say the venue answered.
    Http(u16),
    /// Transport or local filesystem access requires a later retry.
    Transient(String),
    /// Published content is malformed and must not be retried as a transport failure.
    Corrupt(String),
}

#[cfg(test)]
mod tests;
