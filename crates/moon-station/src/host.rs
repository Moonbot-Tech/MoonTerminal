//! The station's server and process, for `status` (STATION.md §4.5), kept all the time: processor
//! load and memory from the terminal's own sampler, disk space and the data root's files from a
//! thread of their own.
//!
//! The load comes from `moon_core::metrics` (its own thread, one sysinfo poll a second); the main
//! loop only copies its snapshot once a second and folds it into minutes, numbered from the
//! start, a day of them kept. A window is the minutes of the last hour or day by the clock — a
//! stalled loop leaves a gap, not a longer "hour" — its average the mean of its minutes and its
//! peak the busiest minute: a one-second burst, which a single core shows at 100 % for any
//! start-up, does not pass for load.
//!
//! Walking the data root and asking the filesystem for its space is I/O of unknown length, so
//! neither runs on the main loop: a thread refreshes them every [`DISK_EVERY`] and `status` reads
//! what it found last.

use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use moon_core::metrics::{MetricsSampler, MetricsSnapshot};
use moon_core::station_api::{CpuWindow, DataFile, Disk, Host, Memory};

/// How often the main loop copies the sampler's snapshot: the sampler's own cadence.
const SAMPLE_EVERY: Duration = Duration::from_secs(1);
const MINUTE: Duration = Duration::from_secs(60);
/// The windows `status` reports, in minutes: the last hour and the last day.
const WINDOWS: [u64; 2] = [60, 1440];
/// Minutes kept: the longest window.
const KEEP_MINUTES: usize = 1440;
/// How often the disk thread reads the data root and the filesystem's space.
const DISK_EVERY: Duration = Duration::from_secs(60);
/// Most entries of the data root reported: its own few files and directories, never a frame's
/// worth of names.
const MAX_FILES: usize = 64;
const MIB: f64 = 1024.0 * 1024.0;
/// What a database's companion files end in; each is counted with its database.
const COMPANIONS: [&str; 3] = ["-wal", "-shm", "-journal"];

/// One minute's samples, as percentages of the whole machine.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Minute {
    /// Minutes since the watch started: where the minute sits in time.
    index: u64,
    station_sum: f32,
    machine_sum: f32,
    samples: u32,
}

impl Minute {
    fn add(&mut self, station: f32, machine: f32) {
        self.station_sum += station;
        self.machine_sum += machine;
        self.samples += 1;
    }

    /// The minute's averages: the station's, the machine's.
    fn averages(&self) -> (f32, f32) {
        let n = self.samples.max(1) as f32;
        (self.station_sum / n, self.machine_sum / n)
    }
}

/// What the disk thread found last.
#[derive(Default)]
struct DiskFound {
    disk: Option<Disk>,
    files: Vec<DataFile>,
}

/// The load history, the latest reading, and the disk thread's findings.
pub struct HostWatch {
    /// When the process started, for its uptime.
    process_started: Instant,
    sampler: MetricsSampler,
    last_sample: Instant,
    /// Where minute 0 began.
    first_minute: Instant,
    current: Minute,
    /// Minutes with readings, oldest first.
    minutes: VecDeque<Minute>,
    /// The last snapshot that carried a reading.
    last: Option<MetricsSnapshot>,
    rss_peak_mb: f32,
    disk: Arc<Mutex<DiskFound>>,
}

impl HostWatch {
    /// Start the sampler and the disk thread; the first minute begins now.
    ///
    /// Args:
    ///     process_started: When the process started, taken first thing in `main`.
    ///     data_root: The station's data root, whose files and filesystem are reported.
    pub fn start(process_started: Instant, data_root: PathBuf) -> Self {
        let now = Instant::now();
        Self {
            process_started,
            sampler: moon_core::metrics::spawn_sampler(),
            last_sample: now,
            first_minute: now,
            current: Minute::default(),
            minutes: VecDeque::with_capacity(KEEP_MINUTES),
            last: None,
            rss_peak_mb: 0.0,
            disk: spawn_disk_thread(data_root),
        }
    }

    /// The main loop's call on every pass: a clock compare, and once a second a copy of the
    /// sampler's snapshot folded into the minute.
    pub fn tick(&mut self, now: Instant) {
        if now.duration_since(self.last_sample) < SAMPLE_EVERY {
            return;
        }
        self.last_sample = now;
        let snapshot = self.sampler.snapshot();
        self.observe(now, snapshot);
    }

    fn observe(&mut self, now: Instant, snapshot: MetricsSnapshot) {
        let index = now.duration_since(self.first_minute).as_secs() / MINUTE.as_secs();
        if index != self.current.index {
            // A minute without a reading (the sampler not up yet, a stalled loop) is a gap, not
            // a minute of zero load.
            if self.current.samples > 0 {
                if self.minutes.len() == KEEP_MINUTES {
                    self.minutes.pop_front();
                }
                self.minutes.push_back(self.current);
            }
            self.current = Minute {
                index,
                ..Minute::default()
            };
        }
        // All zeros until the sampler's first poll, and zeros for the process when sysinfo did
        // not see it that second: no reading, not a reading of nothing.
        let readable = snapshot.mem_total_mb > 0.0
            && snapshot.mem_mb > 0.0
            && snapshot.cpu_process.is_finite()
            && snapshot.cpu_system.is_finite();
        if readable {
            self.rss_peak_mb = self.rss_peak_mb.max(snapshot.mem_mb);
            self.current.add(snapshot.cpu_process, snapshot.cpu_system);
            self.last = Some(snapshot);
        }
    }

    /// The minutes that are whole at minute `now_index`: the kept ones, and the current one once
    /// its minute has passed — the next tick would fold it, a status asked before that counts it.
    fn whole_minutes(&self, now_index: u64) -> impl Iterator<Item = &Minute> + '_ {
        let ended =
            (self.current.index < now_index && self.current.samples > 0).then_some(&self.current);
        self.minutes.iter().chain(ended)
    }

    /// The station's server and process now.
    pub fn host(&self) -> Host {
        let now_index = self.first_minute.elapsed().as_secs() / MINUTE.as_secs();
        let found = self.disk.lock().unwrap_or_else(PoisonError::into_inner);
        Host {
            uptime_s: self.process_started.elapsed().as_secs(),
            cpu: WINDOWS
                .iter()
                .filter_map(|&minutes| window(self.whole_minutes(now_index), now_index, minutes))
                // Two windows over the same few minutes say the same thing once.
                .fold(Vec::new(), |mut out: Vec<CpuWindow>, w| {
                    if out.last().is_none_or(|prev| prev.minutes != w.minutes) {
                        out.push(w);
                    }
                    out
                }),
            memory: self.last.map(|s| Memory {
                rss_bytes: mib_to_bytes(s.mem_mb),
                rss_peak_bytes: mib_to_bytes(self.rss_peak_mb),
                available_bytes: mib_to_bytes(s.mem_available_mb),
                total_bytes: mib_to_bytes(s.mem_total_mb),
            }),
            disk: found.disk,
            files: found.files.clone(),
        }
    }
}

/// The whole minutes of the last `minutes` before minute `now_index` (the current one is not
/// whole yet): the mean of their averages and the busiest one, and how many minutes the window
/// covers — fewer while the process is younger. `None` without a minute in it.
fn window<'a>(
    history: impl Iterator<Item = &'a Minute>,
    now_index: u64,
    minutes: u64,
) -> Option<CpuWindow> {
    let from = now_index.saturating_sub(minutes);
    let (mut taken, mut station_sum, mut machine_sum) = (0u32, 0.0f32, 0.0f32);
    let (mut station_peak, mut machine_peak) = (0.0f32, 0.0f32);
    for minute in history.filter(|m| m.index >= from && m.index < now_index) {
        let (station, machine) = minute.averages();
        taken += 1;
        station_sum += station;
        machine_sum += machine;
        station_peak = station_peak.max(station);
        machine_peak = machine_peak.max(machine);
    }
    if taken == 0 {
        return None;
    }
    let n = taken as f32;
    Some(CpuWindow {
        minutes: now_index.min(minutes) as u32,
        station_avg_permille: permille(station_sum / n),
        station_peak_permille: permille(station_peak),
        machine_avg_permille: permille(machine_sum / n),
        machine_peak_permille: permille(machine_peak),
    })
}

/// A percentage of the machine in tenths of a percent, within 0..=1000.
fn permille(percent: f32) -> u16 {
    if !percent.is_finite() {
        return 0;
    }
    (percent * 10.0).round().clamp(0.0, 1000.0) as u16
}

fn mib_to_bytes(mib: f32) -> u64 {
    if !mib.is_finite() || mib <= 0.0 {
        return 0;
    }
    (f64::from(mib) * MIB).round() as u64
}

/// The thread that reads the data root and its filesystem, now and every [`DISK_EVERY`]. It
/// holds a weak handle: once the watch is gone it ends at its next wake.
fn spawn_disk_thread(data_root: PathBuf) -> Arc<Mutex<DiskFound>> {
    let found = Arc::new(Mutex::new(DiskFound::default()));
    let published = Arc::downgrade(&found);
    let started = std::thread::Builder::new()
        .name("station-disk".into())
        .spawn(move || {
            // The same measure keeps the disk's reserve: the tape gives back what it lacks.
            let mut keeper = crate::storage::Keeper::default();
            loop {
                let fresh = DiskFound {
                    disk: disk_space(&data_root),
                    files: data_files(&data_root),
                };
                if let Some(disk) = fresh.disk {
                    keeper.look(disk);
                }
                let Some(slot) = published.upgrade() else {
                    return;
                };
                *slot.lock().unwrap_or_else(PoisonError::into_inner) = fresh;
                drop(slot);
                std::thread::sleep(DISK_EVERY);
            }
        });
    if let Err(e) = started {
        log::warn!("status: no thread for the disk figures ({e}); they stay empty");
    }
    found
}

/// Everything in `root`, largest first and at most [`MAX_FILES`]: a file with its companions as
/// one entry, a directory as the sum of the files under it (links not followed). What cannot be
/// read counts as nothing.
fn data_files(root: &Path) -> Vec<DataFile> {
    let mut sizes: BTreeMap<String, u64> = BTreeMap::new();
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    for entry in entries.flatten() {
        let Ok(meta) = entry.path().symlink_metadata() else {
            continue;
        };
        let name = entry.file_name().to_string_lossy().into_owned();
        let (name, bytes) = if meta.is_dir() {
            (format!("{name}/"), tree_size(&entry.path()))
        } else {
            (database_of(&name).to_owned(), meta.len())
        };
        *sizes.entry(name).or_default() += bytes;
    }
    let mut files: Vec<DataFile> = sizes
        .into_iter()
        .map(|(name, bytes)| DataFile { name, bytes })
        .collect();
    files.sort_by(|a, b| b.bytes.cmp(&a.bytes).then_with(|| a.name.cmp(&b.name)));
    files.truncate(MAX_FILES);
    files
}

/// The database a companion file belongs to (`reports.sqlite-wal` → `reports.sqlite`), or the
/// name itself.
fn database_of(name: &str) -> &str {
    COMPANIONS
        .iter()
        .find_map(|suffix| name.strip_suffix(suffix))
        .filter(|base| !base.is_empty())
        .unwrap_or(name)
}

/// The bytes of the files under `dir`, links not followed.
fn tree_size(dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let meta = entry.path().symlink_metadata().ok()?;
            Some(match meta.is_dir() {
                true => tree_size(&entry.path()),
                false => meta.len(),
            })
        })
        .sum()
}

/// Free and total space of the filesystem `path` is on.
#[cfg(unix)]
fn disk_space(path: &Path) -> Option<Disk> {
    use std::os::unix::ffi::OsStrExt;
    let path = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut stat = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: `path` is a NUL-terminated string that outlives the call, and `stat` points to
    // space for one `statvfs`, which the call fills when it returns 0.
    let stat = unsafe {
        if libc::statvfs(path.as_ptr(), stat.as_mut_ptr()) != 0 {
            return None;
        }
        stat.assume_init()
    };
    // The field types are platform aliases (`c_ulong`, `fsblkcnt_t`), 64 bits on the servers
    // the station runs on. The block counts are in fragments; a filesystem that reports none
    // counts in its block size.
    let block = match stat.f_frsize as u64 {
        0 => stat.f_bsize as u64,
        fragment => fragment,
    };
    Some(Disk {
        free_bytes: (stat.f_bavail as u64).saturating_mul(block),
        total_bytes: (stat.f_blocks as u64).saturating_mul(block),
    })
}

/// Off Unix — the station's Windows build is for tests only.
#[cfg(not(unix))]
fn disk_space(_path: &Path) -> Option<Disk> {
    None
}

#[cfg(test)]
mod tests;
