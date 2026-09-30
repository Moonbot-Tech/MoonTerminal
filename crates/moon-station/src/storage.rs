//! Storage by the room on the disk (STATION.md §1 п. 7, §4.9 п. 4): the station keeps everything
//! while the server has space and, when it runs short, gives back the oldest tape — only the tape.
//! The reports, the order traces and the valuation cache are never touched; the logs keep their
//! own 14 days.
//!
//! The rule, decided 2026-09-30: keep free the larger of 2 GiB and a tenth of the filesystem. The
//! disk thread (`host.rs`, every minute) measures the space; whatever it finds missing is asked of
//! the tape recorder, which evicts that much of its oldest prints and shrinks its file by it. The
//! eviction runs on the recorder's own thread, so a measure taken before it landed would ask for
//! the same bytes again: after asking, the keeper waits [`SETTLE`] for the room to show before it
//! asks again.

use std::time::{Duration, Instant};

use moon_core::station_api::Disk;

/// The free space always kept, at the least: 2 GiB.
const MIN_FREE_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// The free space always kept, as a share of the filesystem: a tenth.
const MIN_FREE_SHARE: u64 = 10;
/// How often a lasting shortfall is warned about in the journal.
const WARN_EVERY: Duration = Duration::from_secs(30 * 60);
/// How long after asking for room the keeper looks again: long enough for the eviction and the
/// file's shrink to land, so one shortfall is not evicted twice.
const SETTLE: Duration = Duration::from_secs(5 * 60);

/// The free space the station keeps on a filesystem of `total_bytes`.
pub fn reserve(total_bytes: u64) -> u64 {
    MIN_FREE_BYTES.max(total_bytes / MIN_FREE_SHARE)
}

/// How many bytes the disk is short of its reserve; `None` while it has enough, or where the
/// filesystem reports no size at all (some container mounts) — nothing to reckon with.
pub fn shortfall(disk: Disk) -> Option<u64> {
    if disk.total_bytes == 0 {
        return None;
    }
    reserve(disk.total_bytes)
        .checked_sub(disk.free_bytes)
        .filter(|&short| short > 0)
}

/// What the disk thread does with each measure: ask the tape for the room that is missing.
#[derive(Default)]
pub struct Keeper {
    warned_at: Option<Instant>,
    asked_at: Option<Instant>,
}

impl Keeper {
    /// Look at one measure of the data root's filesystem and ask for what it is short of.
    pub fn look(&mut self, disk: Disk) {
        let Some(short) = shortfall(disk) else {
            self.warned_at = None;
            self.asked_at = None;
            return;
        };
        if self.asked_at.is_some_and(|at| at.elapsed() < SETTLE) {
            return;
        }
        self.asked_at = Some(Instant::now());
        if self.warned_at.is_none_or(|at| at.elapsed() >= WARN_EVERY) {
            log::warn!(
                "disk: {} MB free of {} MB, {} MB kept free: evicting the oldest tape",
                disk.free_bytes / 1_000_000,
                disk.total_bytes / 1_000_000,
                reserve(disk.total_bytes) / 1_000_000
            );
            self.warned_at = Some(Instant::now());
        }
        moon_core::market::tape_recorder::free_disk(i64::try_from(short).unwrap_or(i64::MAX));
    }
}

#[cfg(test)]
mod tests;
