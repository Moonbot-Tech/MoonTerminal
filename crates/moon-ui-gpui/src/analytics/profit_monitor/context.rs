//! Profit Monitor context.

use super::*;

/// Diff one snapshot against the trades already on screen.
///
/// A core counts as having traded when its newest close date MOVED or its trade count GREW. The
/// count is not redundant: close dates carry whole seconds, so a second trade inside one second
/// moves nothing else. Growth only — retention trimming a period lowers the count, and that is not
/// a trade.
///
/// Args:
///     seen: Previous `(newest close date, trade count)` per core, or `None` for no baseline.
///     cores: Per-core aggregates from the snapshot just applied.
///
/// Returns:
///     The replacement memory, and the cores that traded since the previous snapshot. With no
///     baseline nothing arrives — that is the first snapshot after a query change, where every
///     value is new at once. With a baseline, a core APPEARING is an arrival: it is that core's
///     first trade of the period, which is exactly what the highlight is for. A core absent from
///     `cores` is dropped, keeping the memory bounded by the report's own core count.
pub(super) fn arrivals(
    seen: Option<&HashMap<CoreId, (i64, i64)>>,
    cores: &[ProfitMonitorCore],
) -> (HashMap<CoreId, (i64, i64)>, Vec<CoreId>) {
    let mut next = HashMap::with_capacity(cores.len());
    let mut arrived = Vec::new();
    for core in cores {
        if let Some(seen) = seen {
            let traded = match seen.get(&core.core_uid) {
                Some((close, trades)) => core.last_close > *close || core.trades > *trades,
                None => true,
            };
            if traded {
                arrived.push(core.core_uid);
            }
        }
        next.insert(core.core_uid, (core.last_close, core.trades));
    }
    (next, arrived)
}

/// Return the current logical width for any window-bounds state.
///
/// Args:
///     window: Window whose responsive width is required.
///
/// Returns:
///     Logical width the window currently occupies, in every window state.
pub(super) fn window_width(window: &Window) -> f32 {
    crate::window::windowing::responsive_width(window)
}

/// Read live labels and canonical core order without touching report SQLite.
///
/// Args:
///     backend: Current shared terminal state.
///
/// Returns:
///     Context sufficient to regroup cached per-core aggregates.
pub(super) fn capture_live_context(backend: &Backend) -> LiveContext {
    let config = &backend.config;
    // The Profit Monitor is an Analytics surface that lists cores and folds them into money, so
    // it is `Singleton`: it inherits the last focused group's preset (Auto or Classic) like
    // Strategies, showing everything and marking nothing when no group is focused.
    let preset = backend.display_preset(crate::workspace::DisplayOwner::Singleton);
    let configured_total = config.servers.len();
    let core_names = config
        .servers
        .iter()
        .filter(|server| backend.core_displayed(preset, server.id))
        .map(|server| (server.id, server.name.clone()))
        .collect();
    // The header's own FLEET run cell commands the whole table, not one row — scoping its
    // authority through `core_displayed` the way `active` below legitimately is would narrow a
    // COMMAND path behind the display preset.
    //
    // The session's own admission rule, from `session::lifecycle`, bounds BOTH: a core inside a
    // server group that is switched off never connects, however active its own checkbox is.
    // Reading the checkbox alone would give such a core a zero row in a window that can never
    // fill it.
    //
    // NOTE the axis this excludes, because it is not the display one and the docstrings elsewhere
    // only discuss that: a core the user DEACTIVATED keeps its own row and its own row button when
    // it traded in the period, but is not part of the fleet cell's authority. That is deliberate —
    // it has no live session for a Trading or Auto toggle to reach.
    //
    // `group_ref`, not `group`: the latter clones a whole `GroupConfig` per server, and this runs
    // for every configured core on every five-second sample. An unlisted group defaults to active
    // (`GroupConfig::new`), which is what `is_none_or` states here.
    let action_core_ids: Vec<CoreId> = config
        .servers
        .iter()
        .filter(|server| {
            server.active
                && config
                    .group_ref(&server.group)
                    .is_none_or(|group| group.active)
        })
        .map(|server| server.id)
        .collect();
    // Derived from the list above rather than re-walking `config.servers` with the same predicate:
    // `active` IS `action_core_ids` narrowed by the display preset, which is the one difference
    // between them.
    let active = action_core_ids
        .iter()
        .copied()
        .filter(|core| backend.core_displayed(preset, *core))
        .collect();
    let order = CoreOrder::new(config);
    let mut core_order = config
        .servers
        .iter()
        .filter(|server| backend.core_displayed(preset, server.id))
        .map(|server| server.id)
        .collect::<Vec<_>>();
    order.sort_by(&mut core_order, |core| *core);
    let configured_core_ids = config.servers.iter().map(|server| server.id).collect();
    LiveContext {
        venues: backend.session.core_venues().clone(),
        core_names,
        core_order,
        active,
        core_groups: config.core_groups.clone(),
        preset,
        configured_total,
        configured_core_ids,
        action_core_ids,
    }
}

/// Combine the two monotonic generations that make projected report values stale.
///
/// Args:
///     report: Report-writer generation, when replication is enabled.
///     valuation: Historical/current valuation generation, when available.
///
/// Returns:
///     Wrapping sum used only as a change token.
pub(super) fn combined_generation(
    report: &Option<Arc<AtomicU64>>,
    valuation: &Option<Arc<AtomicU64>>,
) -> u64 {
    report
        .as_ref()
        .map(|generation| generation.load(Ordering::Relaxed))
        .unwrap_or(0)
        .wrapping_add(
            valuation
                .as_ref()
                .map(|generation| generation.load(Ordering::Relaxed))
                .unwrap_or(0),
        )
}
