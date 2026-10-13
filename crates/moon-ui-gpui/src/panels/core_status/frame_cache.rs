//! Per-render memo for everything Core Status derives from its cached snapshot.
//!
//! GPUI re-renders the whole [`super::CoreStatusView`] on every row hover transition and scroll
//! tick of any list inside it, because a child's notify dirties its ancestors. `render` therefore
//! must not walk the scope, the findings, the update history or the chart rings: each of those
//! projections lives here, keyed by the snapshot generation (`rows_generation`, advanced only by
//! `rebuild_cache`) plus the view state it reads, and a repeat render with the same key returns
//! the stored `Rc` without calling its builder.

use std::collections::{HashMap, HashSet, VecDeque};
use std::net::IpAddr;
use std::rc::Rc;

use moon_core::session::CoreId;
use moon_core::session::core_order::OrderedCores;
use moon_core::session::core_update::{CoreUpdatePhase, CoreUpdateRecord};

use rust_i18n::t;

use super::health::{self, HealthCounts, SegmentCounts, Verdict};
use super::mode::UPDATE_LIST_LIMIT;
use super::model::{CoreStatusRow, ServerKey, ServerStatusGroup};
use super::updates_list::VersionShare;
use super::{CoreStatusView, interactions, problems, servers_view, updates_list};
use crate::Backend;

#[cfg(test)]
mod tests;

/// One memoized projection with its key and hit/build counters.
///
/// The counters exist for the regression test that pins "a repeat render builds nothing"; they
/// cost two integer increments per lookup.
pub(super) struct Memo<K, V> {
    entry: Option<(K, Rc<V>)>,
    /// Lookups answered from the stored value.
    pub(super) hits: u64,
    /// Values stored after a miss, i.e. how many times a builder ran.
    pub(super) builds: u64,
}

impl<K, V> Default for Memo<K, V> {
    fn default() -> Self {
        Self {
            entry: None,
            hits: 0,
            builds: 0,
        }
    }
}

impl<K: PartialEq, V> Memo<K, V> {
    /// Return the stored value when `key` matches it, counting a hit.
    ///
    /// Args:
    ///     key: Key the caller would build under.
    ///
    /// Returns:
    ///     The stored value on a hit, `None` on a miss. A miss counts nothing: [`Self::store`]
    ///     counts the build that follows it.
    pub(super) fn cached(&mut self, key: &K) -> Option<Rc<V>> {
        self.cached_while(key, |_| true)
    }

    /// Like [`Self::cached`], but a stored value under the same key also has to pass `still_valid`.
    ///
    /// Args:
    ///     key: Key the caller would build under.
    ///     still_valid: Extra test on the stored value, for a value that expires with the clock.
    ///
    /// Returns:
    ///     The stored value on a hit, `None` on a miss.
    pub(super) fn cached_while(
        &mut self,
        key: &K,
        still_valid: impl FnOnce(&V) -> bool,
    ) -> Option<Rc<V>> {
        let value = self
            .entry
            .as_ref()
            .filter(|(stored, value)| stored == key && still_valid(value))
            .map(|(_, value)| value.clone())?;
        self.hits += 1;
        Some(value)
    }

    /// Store a freshly built value under `key`, counting a build.
    ///
    /// Args:
    ///     key: Key the value was built under.
    ///     value: The built projection.
    ///
    /// Returns:
    ///     The stored value.
    pub(super) fn store(&mut self, key: K, value: V) -> Rc<V> {
        let value = Rc::new(value);
        self.entry = Some((key, value.clone()));
        self.builds += 1;
        value
    }

    /// Hit-or-build in one call, for builders that need nothing borrowed from the memo's owner.
    ///
    /// Args:
    ///     key: Key to look up and, on a miss, to store under.
    ///     build: Runs only on a miss.
    ///
    /// Returns:
    ///     The stored or freshly built value.
    pub(super) fn get_or_build(&mut self, key: K, build: impl FnOnce() -> V) -> Rc<V> {
        if let Some(value) = self.cached(&key) {
            return value;
        }
        self.store(key, build())
    }
}

/// Health strip, verdict and segment counts, plus the instant they stop being exact.
pub(super) struct StripModel {
    pub(super) counts: HealthCounts,
    pub(super) verdict: Verdict,
    pub(super) segments: SegmentCounts,
    /// First Unix millisecond at which `counts.alarms_24h` would change with no new snapshot:
    /// the earliest counted start leaving the 24 h window, or the earliest future start entering
    /// it. `i64::MAX` when neither can happen.
    pub(super) valid_until_ms: i64,
}

/// Build the strip model for one snapshot at `now_ms`.
///
/// Args:
///     rows: Scoped rows of the snapshot.
///     alarm_starts: Episode starts of the snapshot, Unix milliseconds.
///     servers: Server group count of the snapshot.
///     now_ms: Clock reading of the render that missed.
///
/// Returns:
///     The figures `render` used to compute on every frame, and how long they stay exact.
pub(super) fn strip_model(
    rows: &[CoreStatusRow],
    alarm_starts: &[i64],
    servers: usize,
    now_ms: i64,
) -> StripModel {
    let counts = health::health_counts(rows, alarm_starts, now_ms);
    let verdict = health::verdict(rows, &counts, servers);
    let segments = health::segment_counts(&verdict, &counts, servers);
    let oldest_excluded = now_ms - health::DAY_MS;
    // `health_counts` counts `oldest_excluded < start <= now`. A counted start drops out once
    // `now` reaches `start + DAY_MS`; a future start joins once `now` reaches it.
    let valid_until_ms = alarm_starts
        .iter()
        .filter_map(|&start| {
            if start > now_ms {
                Some(start)
            } else if start > oldest_excluded {
                Some(start.saturating_add(health::DAY_MS))
            } else {
                None
            }
        })
        .min()
        .unwrap_or(i64::MAX);
    StripModel {
        counts,
        verdict,
        segments,
        valid_until_ms,
    }
}

/// Scope figures the toolbar reads: the picker's core list and the effective selection.
pub(super) struct ScopeModel {
    /// Group cores the active preset displays, in canonical order.
    pub(super) cores: OrderedCores,
    /// Whether the workspace owns the scope (Auto mode).
    pub(super) workspace_owned: bool,
    /// Effective scope ids as a set.
    pub(super) effective_selection: HashSet<CoreId>,
    /// Trigger label of the pinned scope chip: the Overview or single-core name, else the
    /// selection summary. `None` when the scope is not workspace-owned and the picker shows.
    pub(super) owned_label: Option<String>,
}

/// Everything the Problems arm draws, resolved from the store once per snapshot.
pub(super) struct ProblemsModel {
    pub(super) rows: Rc<Vec<problems::ProblemRow>>,
    pub(super) core_names: Rc<HashMap<CoreId, String>>,
    pub(super) scope: problems::ProblemsScope,
    pub(super) picked: Option<CoreId>,
    pub(super) zone: chrono_tz::Tz,
    /// [`problems::mark_signature`] of `rows`, so the read-mark's early-out costs nothing.
    pub(super) mark_sig: u64,
}

/// One visible core a footer bulk update could enqueue, before the selection narrows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct UpdateCandidate {
    pub(super) core: CoreId,
    /// Server lane the update would run on.
    pub(super) lane: IpAddr,
    /// Whether the core is behind the fleet's newest build.
    pub(super) behind: bool,
}

/// Everything the Updates arm draws apart from the selection-dependent counts.
pub(super) struct UpdatesModel {
    /// Visible cores mapped to whether a bulk update could enqueue them and whether they are
    /// behind: `None` when not offerable. Lets a selection count in O(selected).
    pub(super) visible: HashMap<CoreId, Option<bool>>,
    /// `(all, behind)` footer counts with no selection, over every visible core.
    pub(super) unselected_counts: (usize, usize),
    pub(super) summary: String,
    pub(super) shares: Vec<VersionShare>,
    pub(super) lags: bool,
    pub(super) newest: String,
    pub(super) live_rows: Rc<Vec<CoreStatusRow>>,
    pub(super) history: Rc<Vec<CoreUpdateRecord>>,
    /// Per history row: whether it is the newest record of its core (the one that may retry).
    pub(super) history_newest: Rc<Vec<bool>>,
    pub(super) phases: Rc<HashMap<CoreId, CoreUpdatePhase>>,
    pub(super) server_names: Rc<HashMap<IpAddr, String>>,
    pub(super) zone: chrono_tz::Tz,
}

/// Plan counts for the footer: how many cores either bulk button would enqueue.
///
/// Mirrors `fleet_update_plan` over [`UpdateCandidate`]s: the selection narrows when it is
/// non-empty, the whole visible list counts otherwise.
///
/// Args:
///     candidates: Offerable visible cores from the snapshot.
///     selected: Membership test for the row selection, or `None` when nothing is selected.
///
/// Returns:
///     `(all, behind)`: how many cores "update all" and "update behind" would enqueue.
pub(super) fn plan_counts(
    candidates: &[UpdateCandidate],
    selected: Option<&dyn Fn(CoreId) -> bool>,
) -> (usize, usize) {
    let mut all = 0;
    let mut behind = 0;
    for candidate in candidates {
        if selected.is_some_and(|is_selected| !is_selected(candidate.core)) {
            continue;
        }
        all += 1;
        behind += usize::from(candidate.behind);
    }
    (all, behind)
}

/// Footer figures for the current selection: `(selected visible, all, behind)`.
///
/// Counts the selected cores the presentation draws (not `core_selection.len()`: the footer label
/// must match what its own confirm names) and mirrors `fleet_update_plan`, without walking the
/// visible list: an empty selection reads the precomputed counts, a non-empty one walks only the
/// selected cores.
///
/// Args:
///     model: The snapshot's Updates model.
///     selected: The selected cores, empty when nothing is selected.
///
/// Returns:
///     How many selected cores are visible, and how many each bulk button would enqueue.
pub(super) fn selection_counts<'a>(
    model: &UpdatesModel,
    selected: impl IntoIterator<Item = &'a CoreId>,
) -> (usize, usize, usize) {
    let mut any = false;
    let (mut shown, mut all, mut behind) = (0, 0, 0);
    for core in selected {
        any = true;
        let Some(offer) = model.visible.get(core) else {
            continue;
        };
        shown += 1;
        if let Some(is_behind) = offer {
            all += 1;
            behind += usize::from(*is_behind);
        }
    }
    if !any {
        let (all, behind) = model.unselected_counts;
        return (0, all, behind);
    }
    (shown, all, behind)
}

/// Whether each history record is the newest one of its core; history is newest-first.
pub(super) fn history_newest(history: &[CoreUpdateRecord]) -> Vec<bool> {
    let mut seen = HashSet::new();
    history
        .iter()
        .map(|record| seen.insert(record.core))
        .collect()
}

/// Inputs of the detached window's chart, copied off the backend rings once per snapshot.
pub(super) struct ChartModel {
    pub(super) points: VecDeque<(u8, u8)>,
    pub(super) ping_points: VecDeque<u16>,
    pub(super) exch_points: VecDeque<u16>,
    pub(super) title: String,
}

/// Key of the Flat column projection: the inputs `projected_column_keys` reads besides rows.
#[derive(PartialEq)]
pub(super) struct ColumnsKey {
    pub(super) generation: u64,
    pub(super) hidden: HashSet<String>,
    pub(super) uniform: Vec<&'static str>,
    pub(super) dragged: HashMap<String, f32>,
    pub(super) measured_w_bits: u32,
    pub(super) locale: String,
}

/// Every memo the render path reads. One per projection, each with its own key.
#[derive(Default)]
pub(super) struct FrameCache {
    /// Key: generation; a hit also needs `now < valid_until_ms`.
    pub(super) strip: Memo<u64, StripModel>,
    /// Key: generation and locale (the pinned label is translated).
    pub(super) scope: Memo<(u64, String), ScopeModel>,
    /// Key: generation, then the sort and pick the arm reads.
    pub(super) problems: Memo<(u64, Option<(String, bool)>, Option<CoreId>), ProblemsModel>,
    /// Key: generation and locale (the campaign summary is translated).
    pub(super) updates: Memo<(u64, String), UpdatesModel>,
    /// Key: generation, the search and the mask.
    pub(super) servers: Memo<(u64, String, bool), Vec<ServerStatusGroup>>,
    pub(super) cpu_history: Memo<u64, HashMap<IpAddr, Vec<(f32, f32)>>>,
    pub(super) server_names: Memo<u64, HashMap<ServerKey, String>>,
    pub(super) columns: Memo<ColumnsKey, Vec<&'static str>>,
    /// Key: generation, then the chart subject (core, else server).
    pub(super) chart: Memo<(u64, Option<CoreId>, Option<ServerKey>), Option<ChartModel>>,
}

impl FrameCache {
    /// Total hits across every memo.
    #[cfg(test)]
    pub(super) fn hits(&self) -> u64 {
        self.strip.hits
            + self.scope.hits
            + self.problems.hits
            + self.updates.hits
            + self.servers.hits
            + self.cpu_history.hits
            + self.server_names.hits
            + self.columns.hits
            + self.chart.hits
    }

    /// Total builds across every memo.
    #[cfg(test)]
    pub(super) fn builds(&self) -> u64 {
        self.strip.builds
            + self.scope.builds
            + self.problems.builds
            + self.updates.builds
            + self.servers.builds
            + self.cpu_history.builds
            + self.server_names.builds
            + self.columns.builds
            + self.chart.builds
    }
}

impl CoreStatusView {
    /// Strip, verdict and segment figures for this frame, rebuilt only on a new snapshot or when
    /// the 24 h alarm window would change the count.
    ///
    /// Args:
    ///     now_ms: The frame's clock reading.
    ///
    /// Returns:
    ///     The memoized strip model.
    pub(super) fn frame_strip(&self, now_ms: i64) -> Rc<StripModel> {
        let generation = self.rows_generation;
        let mut frame = self.frame.borrow_mut();
        if let Some(model) = frame
            .strip
            .cached_while(&generation, |model| now_ms < model.valid_until_ms)
        {
            return model;
        }
        frame.strip.store(
            generation,
            strip_model(
                &self.cached_rows,
                &self.cached_alarm_starts,
                self.cached_groups.len(),
                now_ms,
            ),
        )
    }

    /// Toolbar scope figures for this snapshot.
    ///
    /// Every input moves `rows_generation`: the workspace and core-filter observers and the
    /// retained selection all end in `rebuild_cache`.
    ///
    /// Args:
    ///     b: Backend snapshot, read only on a miss.
    ///
    /// Returns:
    ///     The memoized scope model.
    pub(super) fn frame_scope(&self, b: &Backend) -> Rc<ScopeModel> {
        let key = (self.rows_generation, rust_i18n::locale().to_string());
        if let Some(model) = self.frame.borrow_mut().scope.cached(&key) {
            return model;
        }
        let cores = self.displayed_scope_cores(b);
        let scope = self.effective_scope(b);
        let pinned_label = match scope.label() {
            crate::workspace::EffectiveScopeLabel::Overview => {
                Some(t!("workspace.overview").to_string())
            }
            crate::workspace::EffectiveScopeLabel::Core(core) => cores
                .iter()
                .find(|(id, _)| *id == core)
                .map(|(_, name)| name.clone()),
            crate::workspace::EffectiveScopeLabel::All
            | crate::workspace::EffectiveScopeLabel::Selection(_) => None,
        };
        let workspace_owned = scope.is_workspace_owned();
        let effective_selection: HashSet<CoreId> = scope.ids().iter().copied().collect();
        let owned_label = workspace_owned.then(|| {
            pinned_label.unwrap_or_else(|| {
                crate::controls::core_selection_summary(
                    &cores,
                    &effective_selection,
                    crate::controls::CoreAllRowMode::ImplicitOrComplete,
                    t!("core_status.all_cores").as_ref(),
                    &|n| t!("core_status.cores_n", n = n).to_string(),
                )
                .label
            })
        });
        let model = ScopeModel {
            workspace_owned,
            effective_selection,
            owned_label,
            cores,
        };
        self.frame.borrow_mut().scope.store(key, model)
    }

    /// Server display names keyed by server, for the Flat table.
    pub(super) fn frame_server_names(&self) -> Rc<HashMap<ServerKey, String>> {
        let groups = self.cached_groups.clone();
        self.frame
            .borrow_mut()
            .server_names
            .get_or_build(self.rows_generation, || {
                groups
                    .iter()
                    .map(|group| (group.key, group.display_name.clone()))
                    .collect()
            })
    }

    /// Server cards the grid draws for the current search, already in card order.
    pub(super) fn frame_server_cards(&self) -> Rc<Vec<ServerStatusGroup>> {
        let groups = self.cached_groups.clone();
        let query = self.search_query.clone();
        let ip_masked = self.ip_masked;
        let key = (self.rows_generation, query.clone(), ip_masked);
        self.frame.borrow_mut().servers.get_or_build(key, || {
            let shown = servers_view::shown_server_groups(&groups, &query, ip_masked);
            servers_view::card_order(&shown)
                .into_iter()
                .cloned()
                .collect()
        })
    }

    /// CPU series per server address, read off `core_chart_hist` once per snapshot.
    ///
    /// Args:
    ///     b: Backend snapshot, read only on a miss.
    ///
    /// Returns:
    ///     Normalized `(x, cpu)` series keyed by server address.
    pub(super) fn frame_cpu_history(&self, b: &Backend) -> Rc<HashMap<IpAddr, Vec<(f32, f32)>>> {
        let groups = self.cached_groups.clone();
        self.frame
            .borrow_mut()
            .cpu_history
            .get_or_build(self.rows_generation, || {
                let mut history = HashMap::<IpAddr, Vec<(f32, f32)>>::new();
                for group in groups.iter() {
                    let Some(ip) = group.address else {
                        continue;
                    };
                    let Some(ring) = b.core_chart_hist.ring(ip) else {
                        continue;
                    };
                    let denom = ring.len().saturating_sub(1).max(1) as f32;
                    let series = ring
                        .iter()
                        .enumerate()
                        .map(|(i, (cpu, _))| (i as f32 / denom, f32::from(*cpu)))
                        .collect();
                    history.insert(ip, series);
                }
                history
            })
    }

    /// Chart inputs of the detached window: a picked core's own rings, else the followed (or
    /// first addressed) server's aggregate rings.
    ///
    /// Args:
    ///     b: Backend snapshot, read only on a miss.
    ///
    /// Returns:
    ///     The chart inputs, or `None` when nothing can be charted.
    pub(super) fn frame_chart(&self, b: &Backend) -> Rc<Option<ChartModel>> {
        let groups = self.cached_groups.clone();
        let (chart_core, chart_server) = (self.chart_core, self.chart_server);
        let key = (self.rows_generation, chart_core, chart_server);
        self.frame.borrow_mut().chart.get_or_build(key, || {
            // A selected core, if it is still in scope, charts ITS OWN samples — CPU/RAM plus its
            // own client↔core and core→exchange pings, not the server-wide worst.
            if let Some(core) = chart_core.and_then(|core_id| {
                groups
                    .iter()
                    .flat_map(|group| group.cores.iter())
                    .find(|core| core.id == core_id)
            }) {
                let (points, ping_points, exch_points) = b
                    .core_line_hist
                    .ring(core.id)
                    .map(|r| {
                        (
                            r.iter().map(|m| (m.cpu, m.mem)).collect(),
                            r.iter().map(|m| m.ping).collect(),
                            r.iter().map(|m| m.exch).collect(),
                        )
                    })
                    .unwrap_or_default();
                return Some(ChartModel {
                    points,
                    ping_points,
                    exch_points,
                    title: core.name.clone(),
                });
            }
            // An address-less (unknown-endpoint) server has no history ring to chart, so it must
            // fall through rather than blank the chart.
            let target = chart_server
                .and_then(|key| groups.iter().find(|group| group.key == key))
                .filter(|group| group.address.is_some())
                .or_else(|| groups.iter().find(|group| group.address.is_some()))?;
            let ip = target.address?;
            Some(ChartModel {
                points: b.core_chart_hist.ring(ip).cloned().unwrap_or_default(),
                ping_points: b.server_ping_hist.ring(ip).cloned().unwrap_or_default(),
                exch_points: b.server_exch_hist.ring(ip).cloned().unwrap_or_default(),
                title: target.display_name.clone(),
            })
        })
    }

    /// The Problems arm's rows, names and action gate, resolved once per snapshot, sort and pick.
    ///
    /// Args:
    ///     b: Backend snapshot, read only on a miss.
    ///
    /// Returns:
    ///     The memoized Problems model.
    pub(super) fn frame_problems(&self, b: &Backend) -> Rc<ProblemsModel> {
        let key = (
            self.rows_generation,
            self.problems_sort.clone(),
            self.problems_picked,
        );
        if let Some(model) = self.frame.borrow_mut().problems.cached(&key) {
            return model;
        }
        let model = self.build_problems(b);
        self.frame.borrow_mut().problems.store(key, model)
    }

    /// Walk the scope's findings: sort, cap, name the silent cores and gate the actions.
    fn build_problems(&self, b: &Backend) -> ProblemsModel {
        let effective = self.effective_scope(b);
        // Scope order, then each core's own listing, is only the tie-break. The active column
        // sort — newest confirmation first, when nothing was saved — runs on the whole list. The
        // cap is applied after that sort, so a new confirmation on a late core is not dropped in
        // favour of an old one on an early core.
        let scope_ids = effective.ids();
        let core_names: HashMap<CoreId, String> = b
            .config
            .servers
            .iter()
            .map(|server| (server.id, server.name.clone()))
            .collect();
        let (sort_key, sort_ascending) = problems::shown_sort(self.problems_sort.as_ref());
        let zone = moon_core::util::display_time::zone_or_utc(b.header_clock_zone());
        let mut refs: Vec<problems::ProblemRef<'_>> = Vec::new();
        let mut silent: Vec<String> = Vec::new();
        // `matched` counts the cores in scope whatever their connection, `targets` only the
        // reachable ones: the command channel outlives a disconnect, so a command queued for a
        // core that is down waits there and fires on reconnect — for the reset, against findings
        // gathered during the outage that nobody ever saw. The difference is what lets a shut gate
        // say "the core you picked is down" rather than "pick one".
        let mut matched = 0usize;
        let mut matched_only = None;
        let mut targets = 0usize;
        // The same dash the table uses for an unnamed core, so one core cannot appear under two
        // different spellings on the one surface.
        let name_of = |core: CoreId| problems::core_label(&core_names, core);
        for core in scope_ids.iter().copied() {
            // Counted for EVERY core in scope, before anything can skip the rest of the body. A
            // core with no store entry is still a core this panel covers, and counting only the
            // ones that have connected would let a twenty-six-core scope report "exactly one core"
            // and open the test on a core nobody chose.
            matched += 1;
            matched_only = (matched == 1).then_some(core);
            let Some(data) = b.session.store().core(core) else {
                // No retained state at all is the same "not known" case as an answer without
                // support: named, never read as clean.
                silent.push(name_of(core));
                continue;
            };
            // A core that has never delivered a list is still one all three actions must reach —
            // see `ProblemsScope::targets` for why `supported` does not narrow this.
            if moon_core::session::CoreRunState::from_core(data).online {
                targets += 1;
            }
            if !data.problems.supported {
                silent.push(name_of(core));
                continue;
            }
            problems::extend_supported(&mut refs, core, &data.problems);
        }
        // Sort first, then cut. The notice states a cut list rather than shortening it in
        // silence, for the same reason a silent core is named. Only the rows that survive the cut
        // are cloned.
        let truncated =
            problems::order_and_cap(&mut refs, &sort_key, sort_ascending, &core_names, zone);
        let rows: Vec<problems::ProblemRow> = refs
            .into_iter()
            .map(|row| problems::ProblemRow {
                core: row.core,
                problem: row.problem.clone(),
            })
            .collect();
        // A CLICKED FINDING narrows all three actions to its core, and that pick is the operator's
        // own — set by the click, held as a CORE, never re-derived from a row index. No pick keeps
        // the whole scope, which is this panel's own rule for a bulk command
        // (`selected_or_visible`).
        //
        // Dropped as soon as the surface can no longer SHOW it: a pick whose findings have all
        // gone would sit there narrowing an irreversible action with nothing on screen saying so.
        // Checked against the DRAWN rows, so the cap cannot keep a pick alive that the operator
        // cannot reach either.
        let picked = self
            .problems_picked
            .filter(|core| rows.iter().any(|row| row.core == *core));
        let (matched, matched_only, targets) = match picked {
            None => (matched, matched_only, targets),
            Some(core) => {
                let online = b
                    .session
                    .store()
                    .core(core)
                    .is_some_and(|d| moon_core::session::CoreRunState::from_core(d).online);
                (1, Some(core), usize::from(online))
            }
        };
        let scope = problems::ProblemsScope {
            cores: scope_ids.len(),
            silent,
            truncated,
            // The test needs ONE core and says which reason it lacks: nothing narrowed to one, or
            // the one it has is down.
            actions: match (matched, matched_only, targets) {
                (1, Some(core), 1) => problems::ActionGate::Ready(core),
                (1, Some(_), _) => problems::ActionGate::NotConnected,
                _ => problems::ActionGate::NoSingleChoice,
            },
            // A live pick answers for ITSELF, so a scope that still holds connected cores must not
            // be reported as offline just because the picked one is.
            picked: picked.is_some(),
            targets,
        };
        let mark_sig = problems::mark_signature(&rows);
        ProblemsModel {
            rows: Rc::new(rows),
            core_names: Rc::new(core_names),
            scope,
            picked,
            zone,
            mark_sig,
        }
    }

    /// The Updates arm's lists, version bar and footer candidates, resolved once per snapshot.
    ///
    /// Args:
    ///     b: Backend snapshot, read only on a miss.
    ///
    /// Returns:
    ///     The memoized Updates model.
    pub(super) fn frame_updates(&self, b: &Backend) -> Rc<UpdatesModel> {
        let key = (self.rows_generation, rust_i18n::locale().to_string());
        if let Some(model) = self.frame.borrow_mut().updates.cached(&key) {
            return model;
        }
        let model = self.build_updates(b);
        self.frame.borrow_mut().updates.store(key, model)
    }

    /// Project the snapshot and the session's update history into the Updates arm's model.
    fn build_updates(&self, b: &Backend) -> UpdatesModel {
        let rows = &self.cached_rows;
        let groups = &self.cached_groups;
        // Updates draws every scoped row, so its on-screen order is the snapshot's own
        // (`interactions::updates_scope_ids`).
        let visible: Vec<CoreId> = rows.iter().map(|row| row.id).collect();
        let behind: HashSet<CoreId> = b.session.cores_behind().into_iter().collect();
        let candidates: Vec<UpdateCandidate> = visible
            .iter()
            .filter_map(|&core| {
                interactions::update_lane(b, core).map(|lane| UpdateCandidate {
                    core,
                    lane,
                    behind: behind.contains(&core),
                })
            })
            .collect();
        let unselected_counts = plan_counts(&candidates, None);
        let mut visible: HashMap<CoreId, Option<bool>> =
            visible.into_iter().map(|core| (core, None)).collect();
        for candidate in &candidates {
            visible.insert(candidate.core, Some(candidate.behind));
        }
        let fleet = b.session.core_update_summary();
        let summary = super::render::campaign_summary(fleet.updating, fleet.queued, fleet.failed);
        let shares = updates_list::version_distribution(rows);
        let lags = interactions::behind_in_scope(rows) > 0;
        let newest = shares
            .iter()
            .find(|share| share.newest)
            .map(|share| super::presentation::build_label(&share.parts))
            .unwrap_or_else(|| "—".to_string());
        let scope = self.effective_scope(b);
        let core_ids: HashSet<CoreId> = scope.ids().iter().copied().collect();
        // Live rows come straight off `rows`: `CoreStatusRow.update` is already this core's
        // current phase. `Done`/`None` are excluded -- a finished attempt is already a history
        // record, and a core the queue has never touched has nothing to show.
        let live_rows: Vec<CoreStatusRow> = rows
            .iter()
            .filter(|row| {
                matches!(
                    row.update,
                    Some(CoreUpdatePhase::Queued { .. })
                        | Some(CoreUpdatePhase::Sent { .. })
                        | Some(CoreUpdatePhase::Waiting { .. })
                        | Some(CoreUpdatePhase::Verifying { .. })
                )
            })
            .cloned()
            .collect();
        // Admit a record when EITHER its core is still configured and in `core_ids` -- so a
        // still-configured core the user did not select never shows through on a shared IP -- OR
        // the core has been removed from configuration entirely and its lane ran on an address
        // this panel's scope still covers, so `reconcile_vanished_updates`'s `Failed(Gone)` row
        // stays reachable after the core leaves the config.
        let configured_core_ids: HashSet<CoreId> =
            b.config.servers.iter().map(|server| server.id).collect();
        let scope_server_addrs: HashSet<IpAddr> =
            groups.iter().filter_map(|group| group.address).collect();
        let phases: HashMap<CoreId, CoreUpdatePhase> = rows
            .iter()
            .filter_map(|row| row.update.clone().map(|phase| (row.id, phase)))
            .collect();
        let history: Vec<CoreUpdateRecord> = b
            .session
            .core_update_history()
            .iter()
            .rev()
            .filter(|record| {
                core_ids.contains(&record.core)
                    || (!configured_core_ids.contains(&record.core)
                        && scope_server_addrs.contains(&record.lane_addr))
            })
            .take(UPDATE_LIST_LIMIT)
            .cloned()
            .collect();
        let update_ips: HashSet<IpAddr> = live_rows
            .iter()
            .filter_map(|row| row.endpoint.map(|ep| ep.address))
            .chain(history.iter().map(|record| record.lane_addr))
            .collect();
        let server_names: HashMap<IpAddr, String> = update_ips
            .into_iter()
            .map(|ip| {
                let name = groups
                    .iter()
                    .find(|group| group.address == Some(ip))
                    .map(|group| group.display_name.clone())
                    .or_else(|| b.layout.core_server_names.get(&ip.to_string()).cloned())
                    .unwrap_or_else(|| "—".to_string());
                (ip, name)
            })
            .collect();
        UpdatesModel {
            visible,
            unselected_counts,
            summary,
            shares,
            lags,
            newest,
            live_rows: Rc::new(live_rows),
            history_newest: Rc::new(history_newest(&history)),
            history: Rc::new(history),
            phases: Rc::new(phases),
            server_names: Rc::new(server_names),
            zone: moon_core::util::display_time::zone_or_utc(b.header_clock_zone()),
        }
    }
}
