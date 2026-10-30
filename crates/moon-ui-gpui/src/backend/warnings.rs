//! Warning sampling, persisted topology captures, and scoped episodes.

use super::warn_slices::PendingWarnSlice;
use super::warn_slices::WARN_PRUNE_INTERVAL_MS;
use super::warn_slices::WARN_SLICE_FWD_MS;
use super::warn_slices::WARN_SLICE_RETENTION_MS;
use super::warn_slices::capture_topology;
use super::warn_slices::finalize_recent_warning_episodes;
use super::warn_slices::push_pending_slice;
use super::warn_slices::ring_slice;
use crate::Backend;
use crate::backend::core_warn::axis_has_series;
use moon_core::session::CoreId;
use std::collections::{HashMap, HashSet};
use std::net::IpAddr;

/// Reconcile last-known server ownership and return the IPs whose cores are still configured.
///
/// Observations update the map even across reconnects; only configured cores remain in it, so an
/// unresolved endpoint cannot preserve a removed core's history or block another core's eviction.
fn reconcile_endpoint_owners(
    owners: &mut HashMap<CoreId, IpAddr>,
    configured: impl IntoIterator<Item = CoreId>,
    observed: impl IntoIterator<Item = (CoreId, IpAddr)>,
) -> HashSet<IpAddr> {
    owners.extend(observed);
    let configured: HashSet<CoreId> = configured.into_iter().collect();
    owners.retain(|core, _| configured.contains(core));
    owners.values().copied().collect()
}

impl Backend {
    /// Advance the backend warning engine one tick from the current core telemetry.
    ///
    /// Runs from the coordination loop (backend-always, ~10 Hz); the engine throttles itself to
    /// 1 Hz. Samples every live core's endpoint, status, and telemetry only when a tick is due;
    /// settings, retention pruning, and pending slice draining still run on every call.
    ///
    /// Args:
    ///     now_ms: Current Unix milliseconds.
    ///
    /// Returns:
    ///     Nothing; the engine's tracking, warning state, and episode log advance in place.
    pub(crate) fn tick_core_warnings(&mut self, now_ms: i64) {
        let enabled = self.warn_enabled();
        self.warn.set_enabled(enabled);
        let tuning = self.warn_tuning();
        self.warn.set_tuning(tuning);
        let result = self.warn.tick_with_samples(now_ms, || {
            let store = self.session.store();
            self.session
                .sessions()
                .iter()
                .filter_map(|session| {
                    let core = store.core(session.id)?;
                    Some(crate::backend::core_warn::CoreSample {
                        id: session.id,
                        ip: core.endpoint.map(|endpoint| endpoint.address),
                        status: core.status.clone(),
                        sys: core.sys,
                        // Aged to NOW, exactly as the panel classifies it: judging the raw stored
                        // count here would let a key expire unnoticed on a core that went down, and
                        // would disagree with the number the operator is reading. An unanswered
                        // check and a key with no expiry both arrive as `None` and stay silent.
                        api_days: core
                            .api_expiry
                            .and_then(|expiry| expiry.days_left_at(now_ms)),
                        // Straight off the store: unlike the key's day count there is nothing to
                        // age — the quota is whatever the core last published, and a core that
                        // publishes none stays `None` and silent.
                        api_quota: core.api_quota,
                    })
                })
                .collect()
        });
        // Play each newly-opened axis's alert sound once (independent of chart visibility). The axis
        // is necessarily enabled — a disabled axis opens nothing.
        //
        // Quiet mode silences the SOUND only, and only for the axes it is not told to let through:
        // the episode above is already recorded, so the warning list, the badges and the charts
        // still show every night the operator slept through.
        for axis in &result.opened {
            if !self.quiet_allows_warn(*axis) {
                continue;
            }
            if let Some(name) = self.warn_sound(*axis) {
                crate::media::sound::play(&name);
            }
        }
        // Record this second's raw chart history into the shared rings (backend-always, so the
        // Core Status chart and the upcoming badge slices have data regardless of any open panel).
        let sec = now_ms / 1000;
        for ring in &result.rings {
            match ring.subject {
                crate::backend::core_warn::RingSubject::Server(ip) => {
                    self.core_chart_hist.record(ip, sec, (ring.cpu, ring.mem))
                }
                crate::backend::core_warn::RingSubject::Core(id) => self.core_line_hist.record(
                    id,
                    sec,
                    crate::backend::server_chart::CoreMetrics {
                        cpu: ring.cpu,
                        mem: ring.mem,
                        ping: ring.ping,
                        exch: ring.exch,
                    },
                ),
            }
        }
        // Per-server ping history (client↔core and core→exchange), recorded backend-always like the
        // CPU/memory rings.
        for (ip, link, exch) in &result.pings {
            self.server_ping_hist.record(*ip, sec, *link);
            self.server_exch_hist.record(*ip, sec, *exch);
        }
        let configured = &self.config.servers;
        self.core_line_hist
            .retain(|core| configured.iter().any(|server| server.id == core));
        let store = self.session.store();
        let live_ips = reconcile_endpoint_owners(
            &mut self.core_endpoint_ips,
            configured.iter().map(|server| server.id),
            self.session.sessions().iter().filter_map(|session| {
                Some((session.id, store.core(session.id)?.endpoint?.address))
            }),
        );
        self.core_chart_hist.retain(|ip| live_ips.contains(&ip));
        self.server_ping_hist.retain(|ip| live_ips.contains(&ip));
        self.server_exch_hist.retain(|ip| live_ips.contains(&ip));
        // Retention prune: drop the graph slices of episodes older than 30 days (the episode rows
        // themselves stay forever), so the per-core blobs cannot grow the file without bound. Repeated
        // once a day rather than only at startup, so a session outliving the retention window keeps
        // trimming — `warn_last_prune_ms == 0` fires it on the first tick.
        if now_ms - self.warn_last_prune_ms >= WARN_PRUNE_INTERVAL_MS {
            self.warn_last_prune_ms = now_ms;
            if let Some(store) = self.warn_store.as_ref()
                && let Err(err) = store.prune_slices(now_ms - WARN_SLICE_RETENTION_MS)
            {
                log::warn!("core warning slice prune failed: {err}");
            }
        }
        // Persist each closed episode plus its full topology (collect the follow-up re-captures into a
        // local first, so the per-episode `&self` capture calls don't clash with the queue push).
        let mut pending: Vec<PendingWarnSlice> = Vec::new();
        for episode in &result.closed {
            // An axis turned off mid-episode closes its open warning on this tick; honor
            // "off = not persisted" by dropping it rather than writing it to the log.
            if !enabled.allows(episode.axis) {
                continue;
            }
            let Some(store) = self.warn_store.as_ref() else {
                continue;
            };
            // Structural, NOT `warn_chart`: display toggles must never stop persistence of an axis
            // that has per-second history. Standing API axes keep only their episode row.
            let has_series = axis_has_series(episode.axis);
            let ip = episode.server_ip;
            let roster = if has_series {
                match ip {
                    Some(ip) => self.cores_on_ip(ip),
                    None => episode.core_id.into_iter().collect(),
                }
            } else {
                Vec::new()
            };
            // Ring slicing and encoding stay cheap and synchronous; the episode and all its series
            // commit together on the writer, so even a shutdown sees one atomic partial capture.
            let slices = if has_series {
                self.episode_topology(ip, &roster, episode.start_ms, now_ms)
            } else {
                Vec::new()
            };
            let rowid = match store.insert_episode_with_slices(episode, slices) {
                Ok(rowid) => rowid,
                Err(err) => {
                    log::warn!("core warning persist failed: {err}");
                    continue;
                }
            };
            // If the forward tail has not accrued yet, re-capture the full window later; OR REPLACE
            // overwrites the partial with the complete slice.
            let capture_at_ms = episode.start_ms + WARN_SLICE_FWD_MS;
            if has_series && capture_at_ms > now_ms {
                pending.push(PendingWarnSlice {
                    episode_id: rowid,
                    ip,
                    roster,
                    start_ms: episode.start_ms,
                    capture_at_ms,
                });
            }
        }
        for item in pending {
            push_pending_slice(&mut self.warn_pending_slices, item);
        }
        self.drain_pending_slices(now_ms);
        // The close that bumped the live revision may have been read before its transaction committed.
        // Wake one replacement generation once the writer publishes that commit; the 100 ms loop's
        // ordinary dirty flag carries it through the existing coalesced observer path.
        let persisted_rev = self.warn_store.as_ref().map_or(0, |store| store.revision());
        if self.warn_persisted_rev != persisted_rev {
            self.warn_persisted_rev = persisted_rev;
            self.warn.bump_rev();
            self.backend_dirty_since_notify = true;
        }
    }

    /// Capture one episode's server and per-core rings as data for the background writer.
    fn episode_topology(
        &self,
        ip: Option<IpAddr>,
        roster: &[CoreId],
        start_ms: i64,
        now_ms: i64,
    ) -> Vec<crate::backend::core_warn::store::WarnSlice> {
        let cores: Vec<_> = roster
            .iter()
            .map(|id| (*id, self.core_line_hist.ring(*id)))
            .collect();
        capture_topology(
            ip.and_then(|ip| self.core_chart_hist.ring(ip)),
            ip.and_then(|ip| self.server_ping_hist.ring(ip)),
            ip.and_then(|ip| self.server_exch_hist.ring(ip)),
            &cores,
            start_ms,
            now_ms,
        )
    }

    /// Core ids whose configured endpoint resolves to `ip` (the server's roster this second).
    fn cores_on_ip(&self, ip: IpAddr) -> Vec<CoreId> {
        let store = self.session.store();
        self.session
            .sessions()
            .iter()
            .filter(|session| {
                store
                    .core(session.id)
                    .and_then(|core| core.endpoint)
                    .map(|endpoint| endpoint.address)
                    == Some(ip)
            })
            .map(|session| session.id)
            .collect()
    }

    /// Capture and persist the ±1 min history slice of any pending episode whose forward window has
    /// now accumulated in the live ring. A pending capture with no ring data is dropped (its card
    /// simply shows no graph), so the queue never stalls.
    ///
    /// Args:
    ///     now_ms: Current Unix milliseconds.
    ///
    /// Returns:
    ///     Nothing; ready slices are written to `warn_store` and removed from the queue.
    fn drain_pending_slices(&mut self, now_ms: i64) {
        if self.warn_store.is_none() {
            self.warn_pending_slices.clear();
            return;
        }
        // Take the ready captures out of the queue first (in insertion order), THEN capture: the
        // capture borrows `&self`, so it cannot run while the queue is being mutated.
        let mut ready: Vec<PendingWarnSlice> = Vec::new();
        let mut i = 0;
        while i < self.warn_pending_slices.len() {
            if self.warn_pending_slices[i].capture_at_ms > now_ms {
                i += 1;
                continue;
            }
            // `remove` (not `swap_remove`) keeps insertion order so the cap's oldest-first eviction
            // stays meaningful; the queue is tiny, so the shift is cheap.
            ready.push(self.warn_pending_slices.remove(i));
        }
        for pending in ready {
            let slices =
                self.episode_topology(pending.ip, &pending.roster, pending.start_ms, now_ms);
            if let Some(store) = self.warn_store.as_ref()
                && let Err(error) = store.insert_slices(pending.episode_id, slices)
            {
                log::warn!("core warning slice recapture failed: {error}");
            }
        }
    }

    /// The server history slice around a moment, from the live ring: the card's live-path graph.
    pub(crate) fn warn_server_slice(
        &self,
        ip: IpAddr,
        at_ms: i64,
        now_ms: i64,
    ) -> Option<Vec<(u8, u8)>> {
        ring_slice(self.core_chart_hist.ring(ip), at_ms, now_ms)
    }

    /// The server client↔core ping slice around a moment, from the live ring: the card's ping line.
    pub(crate) fn warn_server_ping_slice(
        &self,
        ip: IpAddr,
        at_ms: i64,
        now_ms: i64,
    ) -> Option<Vec<u16>> {
        ring_slice(self.server_ping_hist.ring(ip), at_ms, now_ms)
    }

    /// The server core→exchange ping slice around a moment, from the live ring: the card's exch line.
    pub(crate) fn warn_server_exch_slice(
        &self,
        ip: IpAddr,
        at_ms: i64,
        now_ms: i64,
    ) -> Option<Vec<u16>> {
        ring_slice(self.server_exch_hist.ring(ip), at_ms, now_ms)
    }

    /// The engine's axis master switches, projected from the persisted layout toggles.
    pub(crate) fn warn_enabled(&self) -> crate::backend::core_warn::WarnEnabled {
        let axes = self.layout.warn_axes;
        crate::backend::core_warn::WarnEnabled {
            cpu: axes.cpu,
            mem: axes.mem,
            conn: axes.conn,
            ping: axes.ping,
            exch: axes.exch,
            api: axes.api,
            api_quota: axes.api_quota,
        }
    }

    /// The engine's numeric detection thresholds, projected from the persisted per-axis params.
    /// Latency percents (`yellow`/`red` as +N %) become the ratio ×100 the engine consumes.
    fn warn_tuning(&self) -> crate::backend::core_warn::WarnTuning {
        let p = &self.layout.warn_params;
        // Baseline multiplier ×100 (config stores ×N, e.g. 2 → ×2 → 200). Clamp yellow to red so a
        // mis-set yellow > red can't make the yellow band unreachable (severity checks red first).
        let ping_red = u32::from(p.ping.red) * 100;
        let exch_red = u32::from(p.exch.red) * 100;
        crate::backend::core_warn::WarnTuning {
            cpu_pct: u32::from(p.cpu.pct),
            // Hold clamped to ≥1: a hand-edited 0 would otherwise make `next >= hold` true even on a
            // non-critical second (counter resets to 0), warning permanently.
            cpu_hold: u32::from(p.cpu.hold).max(1),
            mem_pct: u32::from(p.mem.pct),
            mem_window: i64::from(p.mem.window),
            ping_yellow_num: (u32::from(p.ping.yellow) * 100).min(ping_red),
            ping_red_num: ping_red,
            ping_window: i64::from(p.ping.window),
            ping_hold: u32::from(p.ping.hold).max(1),
            exch_yellow_num: (u32::from(p.exch.yellow) * 100).min(exch_red),
            exch_red_num: exch_red,
            exch_window: i64::from(p.exch.window),
            exch_hold: u32::from(p.exch.hold).max(1),
            // A `days` of 0 is meaningful here (warn only on the key's last day), so it is NOT
            // floored like the sustain counters above. The ceiling is the popup's own range: a
            // hand-edited `layout.toml` asking for 60 000 days would warn on every dated key.
            api_days: i32::from(p.api.days.min(moon_core::config::layout::API_WARN_MAX_DAYS)),
            // No clamp, unlike the days above: `ApiQuotaWarn::min` is a `u16`, so the TYPE already
            // holds it at `API_QUOTA_WARN_MAX` and a hand-edited `layout.toml` asking for more
            // fails to parse instead of reaching here.
            api_quota_min: u64::from(p.api_quota.min),
        }
    }

    /// The alert sound stem configured for one axis, or `None` when silent.
    fn warn_sound(&self, axis: crate::backend::core_warn::WarnAxis) -> Option<String> {
        use crate::backend::core_warn::WarnAxis;
        let p = &self.layout.warn_params;
        let sound = match axis {
            WarnAxis::SysCpu => &p.cpu.sound,
            WarnAxis::MemGrowth => &p.mem.sound,
            WarnAxis::Unreachable => &p.conn.sound,
            WarnAxis::Ping => &p.ping.sound,
            WarnAxis::ExchPing => &p.exch.sound,
            WarnAxis::ApiExpiry => &p.api.sound,
            WarnAxis::ApiQuota => &p.api_quota.sound,
        };
        sound.clone().filter(|s| !s.trim().is_empty())
    }

    /// The persisted per-axis warning params (chart visibility, sound, thresholds).
    pub(crate) fn warn_params(&self) -> moon_core::config::layout::WarnParams {
        self.layout.warn_params.clone()
    }

    /// Replace the per-axis warning params, marking the layout dirty and invalidating chart marks.
    pub(crate) fn set_warn_params(&mut self, params: moon_core::config::layout::WarnParams) {
        if self.layout.warn_params != params {
            self.layout.warn_params = params;
            self.layout_dirty = true;
            self.warn.bump_rev();
        }
    }

    /// Merge live open episodes with the latest persisted read for one effective workspace scope.
    ///
    /// Args:
    ///     core_ids: Effective core identities accepted by core-specific warning axes.
    ///     server_ips: Effective server identities accepted by server-wide warning axes.
    ///     persisted: Closed episodes loaded independently of the UI thread.
    ///     limit: Maximum rows to return after merging open and persisted episodes.
    ///
    /// Returns:
    ///     Still-open episodes first, then closed ones newest first, capped once to `limit`.
    pub(crate) fn warn_episodes_recent_with_persisted(
        &self,
        core_ids: &HashSet<CoreId>,
        server_ips: &HashSet<IpAddr>,
        persisted: &[crate::backend::core_warn::WarnEpisode],
        limit: usize,
    ) -> Vec<crate::backend::core_warn::WarnEpisode> {
        let mut all = self.warn.open_episodes();
        all.extend(persisted.iter().cloned());
        finalize_recent_warning_episodes(all, self.warn_enabled(), core_ids, server_ips, limit)
    }
}

#[cfg(test)]
mod tests;
