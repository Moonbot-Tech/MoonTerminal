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
use std::collections::HashSet;
use std::net::IpAddr;

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
            let rowid = match self.warn_store.as_ref().map(|s| s.insert_episode(episode)) {
                Some(Ok(rowid)) => rowid,
                Some(Err(err)) => {
                    log::warn!("core warning persist failed: {err}");
                    continue;
                }
                None => continue,
            };
            // Graph slices exist to fill a chart badge's hover card. An axis with no per-second
            // series would write one blob per core for a card that can never have content, so it
            // keeps the episode row and skips the slices.
            //
            // Structural, NOT `warn_chart`: that folds in the user's "show on chart" checkbox, and
            // gating PERSISTENCE on a display toggle would silently stop recording slices for CPU
            // the moment someone unticks it — and leave those episodes graph-less forever after.
            if !axis_has_series(episode.axis) {
                continue;
            }
            // The roster to record per-core slices for: every core on the server, or the episode's
            // own core when it has no known endpoint.
            let ip = episode.server_ip;
            let roster = match ip {
                Some(ip) => self.cores_on_ip(ip),
                None => episode.core_id.into_iter().collect(),
            };
            // Capture immediately with whatever window exists now, so a shutdown before the forward
            // tail fills still leaves a (partial) graph on disk.
            self.capture_episode_topology(rowid, ip, &roster, episode.start_ms, now_ms);
            // If the forward tail has not accrued yet, re-capture the full window later; OR REPLACE
            // overwrites the partial with the complete slice.
            let capture_at_ms = episode.start_ms + WARN_SLICE_FWD_MS;
            if capture_at_ms > now_ms {
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
    }

    /// Capture one episode's full topology (server graph + every core in `roster`) from the current
    /// rings into `warn_store`. A no-op when persistence is off.
    fn capture_episode_topology(
        &self,
        episode_id: i64,
        ip: Option<IpAddr>,
        roster: &[CoreId],
        start_ms: i64,
        now_ms: i64,
    ) {
        let Some(store) = self.warn_store.as_ref() else {
            return;
        };
        let cores: Vec<_> = roster
            .iter()
            .map(|id| (*id, self.core_line_hist.ring(*id)))
            .collect();
        capture_topology(
            store,
            ip.and_then(|ip| self.core_chart_hist.ring(ip)),
            ip.and_then(|ip| self.server_ping_hist.ring(ip)),
            ip.and_then(|ip| self.server_exch_hist.ring(ip)),
            &cores,
            episode_id,
            start_ms,
            now_ms,
        );
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
            self.capture_episode_topology(
                pending.episode_id,
                pending.ip,
                &pending.roster,
                pending.start_ms,
                now_ms,
            );
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

    /// The persisted server history slice for a closed episode, for a card whose warning has already
    /// rolled out of the live ring. `None` if it was never captured or persistence is off.
    pub(crate) fn warn_series_slice(&self, episode_id: u64) -> Option<Vec<(u8, u8)>> {
        self.warn_store
            .as_ref()?
            .series_for_episode(episode_id as i64, 0, "server")
            .ok()
            .flatten()
    }

    /// The persisted client↔core ping slice for a closed episode, past the live ring. `None` if it
    /// was never captured or persistence is off.
    pub(crate) fn warn_ping_series_slice(&self, episode_id: u64) -> Option<Vec<u16>> {
        self.warn_store
            .as_ref()?
            .ping_series_for_episode(episode_id as i64, 0, "ping")
            .ok()
            .flatten()
    }

    /// The persisted core→exchange ping slice for a closed episode, past the live ring.
    pub(crate) fn warn_exch_series_slice(&self, episode_id: u64) -> Option<Vec<u16>> {
        self.warn_store
            .as_ref()?
            .ping_series_for_episode(episode_id as i64, 0, "exch")
            .ok()
            .flatten()
    }

    /// The engine's axis master switches, projected from the persisted layout toggles.
    fn warn_enabled(&self) -> crate::backend::core_warn::WarnEnabled {
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

    /// Whether one axis draws on charts (separate from whether it is detected/recorded).
    fn warn_chart(&self, axis: crate::backend::core_warn::WarnAxis) -> bool {
        use crate::backend::core_warn::WarnAxis;
        let p = &self.layout.warn_params;
        match axis {
            WarnAxis::SysCpu => p.cpu.chart,
            WarnAxis::MemGrowth => p.mem.chart,
            WarnAxis::Unreachable => p.conn.chart,
            WarnAxis::Ping => p.ping.chart,
            WarnAxis::ExchPing => p.exch.chart,
            // Never on a chart: an expiring key has no per-second history, so its badge would open
            // a card with an empty graph. It lives in Core Status and the Warnings list only.
            WarnAxis::ApiExpiry => false,
            // Never on a chart either: the quota is a standing number the core republishes every
            // few minutes, not a per-second series.
            WarnAxis::ApiQuota => false,
        }
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

    /// Warning episodes for one server within `[from_ms, to_ms]`: persisted (closed) plus still-open.
    ///
    /// The chart draws a badge per episode. Merges the SQLite log with the engine's live open
    /// episodes, so an in-progress warning already shows a badge.
    ///
    /// Args:
    ///     ip: Server endpoint address.
    ///     from_ms: Inclusive lower bound on `start_ms`.
    ///     to_ms: Inclusive upper bound on `start_ms`.
    ///
    /// Returns:
    ///     Matching episodes; empty if persistence is off and nothing is open.
    pub(crate) fn warn_episodes_for_server(
        &self,
        ip: IpAddr,
        from_ms: i64,
        to_ms: i64,
    ) -> Vec<crate::backend::core_warn::WarnEpisode> {
        let mut out = self
            .warn_store
            .as_ref()
            .and_then(|store| store.episodes_for_server(ip, from_ms, to_ms).ok())
            .unwrap_or_default();
        for open in self.warn.open_episodes() {
            if open.server_ip == Some(ip) && open.start_ms >= from_ms && open.start_ms <= to_ms {
                out.push(open);
            }
        }
        // A disabled axis hides its already-recorded history from the chart too; an axis with
        // "show on chart" off is still recorded and listed, just not drawn here.
        let enabled = self.warn_enabled();
        out.retain(|episode| enabled.allows(episode.axis) && self.warn_chart(episode.axis));
        out
    }

    /// Warning episodes for one effective workspace scope, filtered before the persisted limit.
    ///
    /// Open episodes are filtered in memory. Persisted episodes are filtered in SQLite before its
    /// `ORDER BY` and `LIMIT`, so unrelated cores cannot crowd the selected workspace out.
    ///
    /// Args:
    ///     core_ids: Effective core identities accepted by core-specific warning axes.
    ///     server_ips: Effective server identities accepted by server-wide warning axes.
    ///     limit: Maximum rows to return after merging open and persisted episodes.
    ///
    /// Returns:
    ///     Still-open episodes first, then closed ones newest first, capped once to `limit`.
    pub(crate) fn warn_episodes_recent_for_scope(
        &self,
        core_ids: &HashSet<CoreId>,
        server_ips: &HashSet<IpAddr>,
        limit: usize,
    ) -> Vec<crate::backend::core_warn::WarnEpisode> {
        let mut all = self.warn.open_episodes();
        if let Some(store) = &self.warn_store
            && let Ok(closed) = store.recent_episodes_for_scope(core_ids, server_ips, limit)
        {
            all.extend(closed);
        }
        finalize_recent_warning_episodes(all, self.warn_enabled(), core_ids, server_ips, limit)
    }
}
