//! Shared per-server CPU / memory chart history.
//!
//! Lives in the Backend (not in a panel) so it accumulates continuously and survives a Core Status
//! window opening and closing — opening a window shows the existing history immediately instead of
//! starting from scratch. One `(cpu %, occupied memory %)` sample per second, one hour retained as
//! a ring, deduplicated per server so several panels feeding it never double-count a second.

use std::collections::{HashMap, VecDeque};
use std::net::IpAddr;

/// Points kept per server: one hour at 1 Hz. At 2 bytes/point that is ~7 KB per server.
const CAP: usize = 3600;

/// Rolling per-server CPU/occupied-memory history keyed by endpoint IP.
#[derive(Default)]
pub(crate) struct ServerChartHistory {
    /// `ip -> (ring of (cpu %, mem %), last recorded second)`.
    rings: HashMap<IpAddr, (VecDeque<(u8, u8)>, i64)>,
}

impl ServerChartHistory {
    /// Append one sample for a server, at most once per second.
    ///
    /// Args:
    ///     ip: Server endpoint address.
    ///     sec: Current Unix second; a sample for an already-recorded second is ignored.
    ///     cpu: Machine CPU percent (raw).
    ///     mem: Occupied memory percent.
    ///
    /// Returns:
    ///     Nothing; the server's ring grows by at most one and is capped to one hour.
    pub(crate) fn record(&mut self, ip: IpAddr, sec: i64, cpu: u8, mem: u8) {
        let entry = self
            .rings
            .entry(ip)
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

    /// Return a server's history ring, if any has been collected.
    ///
    /// Args:
    ///     ip: Server endpoint address.
    ///
    /// Returns:
    ///     The `(cpu %, mem %)` samples oldest-first, or `None` when the server is unseen.
    pub(crate) fn ring(&self, ip: IpAddr) -> Option<&VecDeque<(u8, u8)>> {
        self.rings.get(&ip).map(|(ring, _)| ring)
    }
}
