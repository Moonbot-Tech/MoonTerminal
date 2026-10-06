//! Responsive rail geometry, flattened items, and deferred topology guards.

use super::*;

/// Minimum body width reserved for the active Auto dock tab while the rail is dragged.
const AUTO_DOCK_MIN_WIDTH: f32 = 420.0;

/// Fit one global rail preference into a particular window without changing the preference.
///
/// Args:
///     preferred: Persisted global logical width.
///     chrome_width: Current viewport width in rendered pixels.
///     scale: Current MoonUI scale used to convert between logical and rendered widths.
///
/// Returns:
///     The window-local logical width that preserves the minimum dock content area.
pub(super) fn fitted_auto_rail_width(preferred: f32, chrome_width: f32, scale: f32) -> f32 {
    let max_width = (chrome_width / scale.max(0.01) - AUTO_DOCK_MIN_WIDTH)
        .clamp(AUTO_WORKSPACE_RAIL_WIDTH_MIN, AUTO_WORKSPACE_RAIL_WIDTH_MAX);
    preferred.clamp(AUTO_WORKSPACE_RAIL_WIDTH_MIN, max_width)
}

/// Format the bounded Icon-density summary while the full localized status remains in its tooltip.
///
/// Args:
///     configured: Number of configured cores represented by the rail.
///
/// Returns:
///     Decimal configured-core count without the wider ready/problem fractions.
pub(super) fn icon_workspace_summary(configured: usize) -> String {
    configured.to_string()
}

/// One flattened virtual-list item in the all-core rail.
// `Exchange` holds the logo and the core list for one heading. The rail rebuilds these by value.
#[derive(Clone)]
#[allow(clippy::large_enum_variant)]
pub(super) enum RailItem {
    /// Current group aggregate scope.
    Overview { selected: bool },
    /// Venue heading and its logo, both resolved before the virtual row closure.
    Exchange {
        venue: Option<CoreVenue>,
        logo: Option<Arc<RenderImage>>,
        /// Position of this heading among the drawn sections — the run cell's stable identity.
        section: usize,
        /// Every core drawn under this heading, which its run cell commands.
        cores: Rc<[CoreId]>,
    },
    /// Configured core row and whether its branch stem ends at this leaf.
    Core {
        row: WorkspaceRosterRow,
        is_last_in_section: bool,
        /// The one core this row commands, built once when the rail is flattened — the item builder
        /// runs per visible row on every frame and must not allocate.
        cores: Rc<[CoreId]>,
    },
}

/// What every rail line needs to draw its run cell, resolved once per rail render.
#[derive(Clone, Copy)]
pub(super) struct RailRun {
    /// Slots every line of the rail reserves at the current density.
    pub(super) slots: RunSlots,
    /// Whether an exchange heading also fills them for all its cores.
    pub(super) exchange_controls: bool,
}

/// Density-specific horizontal budget for one indented core leaf.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct CoreRailMetrics {
    pub(super) horizontal_padding: f32,
    pub(super) gap: f32,
    pub(super) connector_width: f32,
    pub(super) connector_elbow_width: f32,
    pub(super) dot_size: f32,
}

/// Return the horizontal geometry for a core leaf at one responsive rail density.
///
/// Args:
///     density: Current Full, Compact, or Icon rail rung.
///
/// Returns:
///     Padding, gap, connector, elbow, and status-dot sizes in logical pixels.
pub(super) fn core_rail_metrics(density: WorkspaceRailDensity) -> CoreRailMetrics {
    match density {
        WorkspaceRailDensity::Icon => CoreRailMetrics {
            horizontal_padding: 4.0,
            gap: 3.0,
            connector_width: 8.0,
            connector_elbow_width: 6.0,
            dot_size: 6.0,
        },
        WorkspaceRailDensity::Full | WorkspaceRailDensity::Compact => CoreRailMetrics {
            horizontal_padding: 9.0,
            gap: 7.0,
            connector_width: 13.0,
            connector_elbow_width: 9.0,
            dot_size: 7.0,
        },
    }
}

/// Append one exchange section while marking exactly its final core as the branch endpoint.
///
/// Args:
///     items: Flattened rail destination.
///     rows: Ordered configured cores belonging to one exchange heading.
///
/// Returns:
///     Nothing; rows retain their order and only the last receives the terminal shape.
pub(super) fn append_core_section_items(items: &mut Vec<RailItem>, rows: Vec<WorkspaceRosterRow>) {
    let last_index = rows.len().checked_sub(1);
    items.extend(rows.into_iter().enumerate().map(|(index, row)| {
        let cores = Rc::from([row.core]);
        RailItem::Core {
            row,
            is_last_in_section: Some(index) == last_index,
            cores,
        }
    }));
}

/// State capable of releasing a generation-scoped Auto topology guard.
pub(super) trait DeferredAutoTopologyGuard {
    /// Clear the guard only when no newer topology application superseded this callback.
    ///
    /// Args:
    ///     generation: Generation captured when the deferred callback was scheduled.
    fn release_auto_topology_guard(&mut self, generation: u64);
}

/// Defer a weak entity guard release until queued dock-event effects have been delivered.
///
/// Args:
///     entity: Weak owner so a closed window is never retained by the callback.
///     generation: Topology application generation to release.
///     cx: Entity context used to enqueue the callback after current effects.
///
/// Returns:
///     Nothing; a newer generation or dropped entity turns the callback into a no-op.
pub(super) fn defer_auto_topology_guard_release<T>(
    entity: WeakEntity<T>,
    generation: u64,
    cx: &mut Context<T>,
) where
    T: DeferredAutoTopologyGuard + 'static,
{
    cx.defer(move |app| {
        let _ = entity.update(app, |state, _| {
            state.release_auto_topology_guard(generation);
        });
    });
}

impl DeferredAutoTopologyGuard for Shell {
    fn release_auto_topology_guard(&mut self, generation: u64) {
        if self.auto_topology_guard_generation == generation {
            self.applying_auto_topology = false;
        }
    }
}
