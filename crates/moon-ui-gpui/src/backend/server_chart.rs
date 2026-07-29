//! Shared per-key CPU / memory chart history.
//!
//! Lives in the Backend (not in a panel) so it accumulates continuously and survives a Core Status
//! window opening and closing — opening a window shows the existing history immediately instead of
//! starting from scratch. One `(cpu %, mem %)` sample per second, one hour retained as a ring,
//! deduplicated per key so several panels feeding it never double-count a second.
//!
//! Two aliases share the same ring, keyed differently:
//! - [`ServerChartHistory`] — per machine IP: `(system CPU %, occupied memory %)`.
//! - [`CoreChartHistory`] — per core: `(process CPU %, process memory share %)`.
//!
//! The Core Status detached chart overlays the server pair and, when a server runs several cores,
//! each core's pair on the same 0..100 % axis.

use std::collections::{HashMap, VecDeque};
use std::hash::Hash;
use std::net::IpAddr;

use moon_core::session::CoreId;

/// Points kept per key: one hour at 1 Hz. At 2 bytes/point that is ~7 KB per key.
const CAP: usize = 3600;

/// Whole-machine history keyed by endpoint IP: `(system CPU %, occupied memory %)`.
pub(crate) type ServerChartHistory = ChartHistory<IpAddr>;

/// Per-core process history keyed by core id: `(process CPU %, process memory share %)`.
pub(crate) type CoreChartHistory = ChartHistory<CoreId>;

/// Rolling per-key CPU/memory history: `key -> (ring of (cpu %, mem %), last recorded second)`.
pub(crate) struct ChartHistory<K: Eq + Hash + Copy> {
    /// One capped ring plus the last recorded second per key, for per-second deduplication.
    rings: HashMap<K, (VecDeque<(u8, u8)>, i64)>,
}

// Manual `Default` so the key type need not be `Default` (an empty map is always valid).
impl<K: Eq + Hash + Copy> Default for ChartHistory<K> {
    fn default() -> Self {
        Self {
            rings: HashMap::new(),
        }
    }
}

impl<K: Eq + Hash + Copy> ChartHistory<K> {
    /// Append one sample for a key, at most once per second.
    ///
    /// Args:
    ///     key: Subject of the sample (a server IP or a core id).
    ///     sec: Current Unix second; a sample for an already-recorded second is ignored.
    ///     cpu: CPU percent for the subject.
    ///     mem: Memory percent for the subject.
    ///
    /// Returns:
    ///     Nothing; the key's ring grows by at most one and is capped to one hour.
    pub(crate) fn record(&mut self, key: K, sec: i64, cpu: u8, mem: u8) {
        let entry = self
            .rings
            .entry(key)
            .or_insert_with(|| (VecDeque::new(), i64::MIN));
        if sec <= entry.1 {
            return;
        }
        entry.1 = sec;
        entry.0.push_back((cpu, mem));
        while entry.0.len() > CAP {
            entry.0.pop_front();
        }
    }

    /// Return a key's history ring, if any has been collected.
    ///
    /// Args:
    ///     key: Subject whose ring is requested.
    ///
    /// Returns:
    ///     The `(cpu %, mem %)` samples oldest-first, or `None` when the key is unseen.
    pub(crate) fn ring(&self, key: K) -> Option<&VecDeque<(u8, u8)>> {
        self.rings.get(&key).map(|(ring, _)| ring)
    }
}

/// Rolling per-SERVER round-trip history: one hour at 1 Hz, deduplicated per second, like
/// [`ChartHistory`] but a single `u16` ms value per sample (a round-trip can exceed a `u8`). Keyed
/// by endpoint IP and holding the server's WORST core round-trip that second, so the chart draws one
/// ping line per server — matching how the server CPU/memory lines aggregate the machine.
#[derive(Default)]
pub(crate) struct ServerPingHistory {
    /// One capped ring of round-trip ms plus the last recorded second, per server.
    rings: HashMap<IpAddr, (VecDeque<u16>, i64)>,
}

impl ServerPingHistory {
    /// Append one server's round-trip sample, at most once per second.
    ///
    /// Args:
    ///     ip: Server the sample belongs to.
    ///     sec: Current Unix second; a sample for an already-recorded second is ignored.
    ///     ms: Round-trip time in milliseconds (the server's worst core this second).
    ///
    /// Returns:
    ///     Nothing; the server's ring grows by at most one and is capped to one hour.
    pub(crate) fn record(&mut self, ip: IpAddr, sec: i64, ms: u16) {
        let entry = self
            .rings
            .entry(ip)
            .or_insert_with(|| (VecDeque::new(), i64::MIN));
        if sec <= entry.1 {
            return;
        }
        entry.1 = sec;
        entry.0.push_back(ms);
        while entry.0.len() > CAP {
            entry.0.pop_front();
        }
    }

    /// Return a server's round-trip ring, if any has been collected.
    pub(crate) fn ring(&self, ip: IpAddr) -> Option<&VecDeque<u16>> {
        self.rings.get(&ip).map(|(ring, _)| ring)
    }
}

#[cfg(test)]
mod tests;
