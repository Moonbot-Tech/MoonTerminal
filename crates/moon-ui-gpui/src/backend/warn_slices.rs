//! Warning episode filtering and bounded persisted history captures.

use moon_core::session::CoreId;
use std::collections::HashSet;
use std::net::IpAddr;

/// Milliseconds of history kept on each side of a warning start for its persisted graphs (±30 s, a
/// 60 s window — enough context for analysis without bloating the per-core slices).
const WARN_SLICE_BACK_MS: i64 = 30_000;
pub(super) const WARN_SLICE_FWD_MS: i64 = 30_000;
/// Persisted warning-episode SLICES (not the episode rows) are pruned once they are older than this,
/// so the per-core graph blobs — which scale with the core count — cannot grow the file forever.
pub(super) const WARN_SLICE_RETENTION_MS: i64 = 30 * 24 * 3600 * 1000;
/// How often the retention prune re-runs (once a day), so a session that outlives the 30-day
/// retention window keeps trimming instead of pruning only at startup.
pub(super) const WARN_PRUNE_INTERVAL_MS: i64 = 24 * 3600 * 1000;
/// Cap on episodes queued for slice capture, so a burst of warnings cannot grow the queue without
/// bound; the oldest pending capture is dropped past it.
const WARN_PENDING_CAP: usize = 1024;

/// Filter, prioritize, and cap warning episodes for one effective workspace list.
///
/// Args:
///     all: Open and persisted warning episodes to reconcile.
///     enabled: Current warning-axis switches.
///     core_ids: Effective core identities accepted by core-specific warning axes.
///     server_ips: Effective server identities accepted by server-wide warning axes.
///     limit: Maximum rows to publish after filtering and ordering.
///
/// Returns:
///     Enabled in-scope episodes with open warnings first, then newest, capped only after scope
///     membership is applied.
pub(super) fn finalize_recent_warning_episodes(
    mut all: Vec<crate::backend::core_warn::WarnEpisode>,
    enabled: crate::backend::core_warn::WarnEnabled,
    core_ids: &HashSet<CoreId>,
    server_ips: &HashSet<IpAddr>,
    limit: usize,
) -> Vec<crate::backend::core_warn::WarnEpisode> {
    all.retain(|episode| {
        enabled.allows(episode.axis)
            && (episode.core_id.is_some_and(|core| core_ids.contains(&core))
                || (episode.core_id.is_none()
                    && episode.server_ip.is_some_and(|ip| server_ips.contains(&ip))))
    });
    // Still-open episodes lead, then newest first. Without the pin, a warning that has been
    // ongoing for weeks -- an expiring API key is exactly that -- has the oldest start time and is
    // the first row the limit drops, hiding the one warning that is still true.
    all.sort_by(|a, b| {
        b.end_ms
            .is_none()
            .cmp(&a.end_ms.is_none())
            .then(b.start_ms.cmp(&a.start_ms))
    });
    all.truncate(limit);
    all
}

/// A closed, persisted episode whose ±1 min history slice is captured later — once its forward tail
/// (`start_ms + WARN_SLICE_FWD_MS`) has accumulated in the live ring — and written to `warn_store`.
pub(crate) struct PendingWarnSlice {
    /// The `core_warnings` row id the slice is keyed to.
    pub(crate) episode_id: i64,
    /// Server whose history ring the slice is read from, or `None` for an unknown-endpoint episode.
    pub(crate) ip: Option<IpAddr>,
    /// The cores to capture per-core slices for, frozen at close time (the roster on the IP, or the
    /// episode's own core when there is no IP).
    pub(crate) roster: Vec<CoreId>,
    /// The episode start; the window is `[start - BACK, start + FWD]`.
    pub(crate) start_ms: i64,
    /// Unix ms at which the forward tail is expected to be present.
    pub(crate) capture_at_ms: i64,
}

/// Slice a history ring to the ±1 min window around `at_ms`, positionally at 1 Hz (the same read the
/// live card uses). `None` when the ring is absent, too short, or does not reach the window.
pub(super) fn ring_slice<T: Copy>(
    ring: Option<&std::collections::VecDeque<T>>,
    at_ms: i64,
    now_ms: i64,
) -> Option<Vec<T>> {
    let ring = ring?;
    let len = ring.len();
    if len < 2 {
        return None;
    }
    let now_sec = now_ms / 1000;
    let at_sec = at_ms / 1000;
    // Window edges in seconds, derived from the same constants that set `base_ms`/`capture_at_ms`.
    let back = WARN_SLICE_BACK_MS / 1000;
    let fwd = WARN_SLICE_FWD_MS / 1000;
    // Seconds back from now to each window edge; the newer edge (at+fwd) may still be the future.
    let k_lo = (now_sec - (at_sec - back)).max(0) as usize;
    let k_hi = (now_sec - (at_sec + fwd)).max(0) as usize;
    let lo = len.saturating_sub(1 + k_lo);
    let hi = len.saturating_sub(1 + k_hi);
    if hi.saturating_sub(lo) < 1 {
        return None;
    }
    Some(ring.iter().skip(lo).take(hi - lo + 1).copied().collect())
}

/// Capture one episode's FULL ±30 s topology as encoded series for one transactional write.
/// A ring with no data in the window contributes nothing. The server graph (`badge 0`) plus every
/// core (`badge = core id`) retain their existing CPU/memory and ping formats.
pub(super) fn capture_topology(
    server: Option<&std::collections::VecDeque<(u8, u8)>>,
    ping: Option<&std::collections::VecDeque<u16>>,
    exch: Option<&std::collections::VecDeque<u16>>,
    cores: &[(
        CoreId,
        Option<&std::collections::VecDeque<crate::backend::server_chart::CoreMetrics>>,
    )],
    start_ms: i64,
    now_ms: i64,
) -> Vec<crate::backend::core_warn::store::WarnSlice> {
    // NOTE for the future timestamped reader: `base_ms` assumes the slice reaches the full back edge.
    // A subject whose ring is shorter than the back window yields a slice whose first sample is NEWER
    // than `base_ms`, so times must be derived from the slice (aligned to `start_ms` and its length),
    // not from `base_ms`. No current reader uses `base_ms`, so this is latent.
    let base_ms = start_ms - WARN_SLICE_BACK_MS;
    let mut out = Vec::new();
    if let Some(samples) = ring_slice(server, start_ms, now_ms) {
        out.push(crate::backend::core_warn::store::WarnSlice::pairs(
            0, "server", base_ms, &samples,
        ));
    }
    if let Some(samples) = ring_slice(ping, start_ms, now_ms) {
        out.push(crate::backend::core_warn::store::WarnSlice::ping(
            0, "ping", base_ms, &samples,
        ));
    }
    if let Some(samples) = ring_slice(exch, start_ms, now_ms) {
        out.push(crate::backend::core_warn::store::WarnSlice::ping(
            0, "exch", base_ms, &samples,
        ));
    }
    for (id, ring) in cores {
        let Some(slice) = ring_slice(*ring, start_ms, now_ms) else {
            continue;
        };
        let badge = *id as i64;
        let cm: Vec<(u8, u8)> = slice.iter().map(|m| (m.cpu, m.mem)).collect();
        let pings: Vec<u16> = slice.iter().map(|m| m.ping).collect();
        let exchs: Vec<u16> = slice.iter().map(|m| m.exch).collect();
        out.push(crate::backend::core_warn::store::WarnSlice::pairs(
            badge, "core", base_ms, &cm,
        ));
        out.push(crate::backend::core_warn::store::WarnSlice::ping(
            badge,
            "core_ping",
            base_ms,
            &pings,
        ));
        out.push(crate::backend::core_warn::store::WarnSlice::ping(
            badge,
            "core_exch",
            base_ms,
            &exchs,
        ));
    }
    out
}

/// Push a pending capture, evicting the oldest if the queue is at its cap.
pub(super) fn push_pending_slice(queue: &mut Vec<PendingWarnSlice>, item: PendingWarnSlice) {
    if queue.len() >= WARN_PENDING_CAP {
        queue.remove(0);
    }
    queue.push(item);
}

#[cfg(test)]
mod tests;
