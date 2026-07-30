//! Shared per-key CPU / memory chart history.
//!
//! Lives in the Backend (not in a panel) so it accumulates continuously and survives a Core Status
//! window opening and closing — opening a window shows the existing history immediately instead of
//! starting from scratch. One `(cpu %, mem %)` sample per second, one hour retained as a ring,
//! deduplicated per key so several panels feeding it never double-count a second.
//!
//! Two aliases share the same generic ring, keyed and valued differently:
//! - [`ServerChartHistory`] — per machine IP: `(system CPU %, occupied memory %)`.
//! - [`CoreChartHistory`] — per core: [`CoreMetrics`] (process CPU %, memory share %, and both pings).
//!
//! The Core Status detached chart overlays the server pair and, when a core is selected, its pair on
//! the same 0..100 % axis; the per-core pings feed the persisted warning-episode slices.

use std::collections::{HashMap, VecDeque};
use std::hash::Hash;
use std::net::IpAddr;

use moon_core::session::CoreId;

/// Points kept per key: one hour at 1 Hz. ~7 KB/key for a server, ~22 KB/key for a core.
const CAP: usize = 3600;

/// One second of a core's own telemetry, kept per core for the detached chart and, crucially, for
/// the per-core warning-episode slices (so an episode's stored graph is the WARNED core's, not the
/// server's worst). Pings are `u16` (a round-trip exceeds a `u8`); a `0` ping means "no live reading
/// this second" (the core was not Ready).
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct CoreMetrics {
    /// Process CPU percent.
    pub(crate) cpu: u8,
    /// Process memory share of the reconstructed machine total, percent.
    pub(crate) mem: u8,
    /// Client↔core round-trip, ms (0 = not Ready this second).
    pub(crate) ping: u16,
    /// Core→exchange order-API latency, ms (0 = not Ready this second).
    pub(crate) exch: u16,
}

/// Whole-machine history keyed by endpoint IP: `(system CPU %, occupied memory %)`.
pub(crate) type ServerChartHistory = ChartHistory<IpAddr, (u8, u8)>;

/// Per-core process history keyed by core id: [`CoreMetrics`].
pub(crate) type CoreChartHistory = ChartHistory<CoreId, CoreMetrics>;

/// Rolling per-key history: `key -> (ring of samples, last recorded second)`, deduplicated per second.
pub(crate) struct ChartHistory<K: Eq + Hash + Copy, V: Copy> {
    /// One capped ring plus the last recorded second per key, for per-second deduplication.
    rings: HashMap<K, (VecDeque<V>, i64)>,
}

// Manual `Default` so the key/value types need not be `Default` (an empty map is always valid).
impl<K: Eq + Hash + Copy, V: Copy> Default for ChartHistory<K, V> {
    fn default() -> Self {
        Self {
            rings: HashMap::new(),
        }
    }
}

impl<K: Eq + Hash + Copy, V: Copy> ChartHistory<K, V> {
    /// Append one sample for a key, at most once per second.
    ///
    /// Args:
    ///     key: Subject of the sample (a server IP or a core id).
    ///     sec: Current Unix second; a sample for an already-recorded second is ignored.
    ///     value: The sample to append.
    ///
    /// Returns:
    ///     Nothing; the key's ring grows by at most one and is capped to one hour.
    pub(crate) fn record(&mut self, key: K, sec: i64, value: V) {
        let entry = self
            .rings
            .entry(key)
            .or_insert_with(|| (VecDeque::new(), i64::MIN));
        if sec <= entry.1 {
            return;
        }
        entry.1 = sec;
        entry.0.push_back(value);
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
    ///     The samples oldest-first, or `None` when the key is unseen.
    pub(crate) fn ring(&self, key: K) -> Option<&VecDeque<V>> {
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
