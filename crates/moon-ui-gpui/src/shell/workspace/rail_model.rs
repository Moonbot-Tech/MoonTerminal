//! Responsive rail geometry, flattened items, and deferred topology guards.

use super::*;
use crate::shell::fleet_model::FleetModelKey;
use crate::workspace::{RosterGrouping, WorkspaceCoreAvailability, WorkspaceRosterSummary};
use moon_core::config::{AppConfig, ServerConfig, WorkspaceMembership};
use moon_core::feed::{ConnFault, CoreStartupStatus};
use moon_core::session::CoreStore;
use std::borrow::Cow;
use std::net::IpAddr;

/// The inputs that affect rail membership, grouping, and the flattened headings.
pub(in crate::shell) struct RailInputs<'a> {
    pub(in crate::shell) config: &'a AppConfig,
    pub(in crate::shell) store: &'a CoreStore,
    pub(in crate::shell) venues: &'a HashMap<CoreId, CoreVenue>,
    pub(in crate::shell) group: &'a str,
    pub(in crate::shell) selected: Option<CoreId>,
    pub(in crate::shell) grouping: RosterGrouping,
    pub(in crate::shell) server_names: &'a HashMap<String, String>,
    pub(in crate::shell) availability: &'a dyn Fn(&ServerConfig) -> WorkspaceCoreAvailability,
    pub(in crate::shell) locale: &'a str,
    pub(in crate::shell) logos_ready: bool,
}

/// Live and configured facts used by one rail item and its group headings.
#[derive(PartialEq, Eq)]
struct RailCore<'a> {
    group: Cow<'a, str>,
    membership: WorkspaceMembership,
    availability: WorkspaceCoreAvailability,
    endpoint_override: Option<Cow<'a, str>>,
    endpoint: Option<IpAddr>,
    venue: Option<Cow<'a, CoreVenue>>,
    status: Option<Cow<'a, ConnStatus>>,
    fault: Option<Cow<'a, ConnFault>>,
    startup: Option<CoreStartupStatus>,
}

impl<'a> RailCore<'a> {
    /// Borrow each per-core dependency without building a roster row.
    fn read(inputs: &RailInputs<'a>, server: &'a ServerConfig) -> Self {
        let core = inputs.store.core(server.id);
        let by_server = inputs.grouping.by_server;
        Self {
            group: Cow::Borrowed(&server.group),
            membership: server.workspace_membership,
            availability: (inputs.availability)(server),
            endpoint_override: by_server
                .then_some(Cow::Borrowed(server.endpoint_override.as_str())),
            endpoint: core
                .filter(|_| by_server)
                .and_then(|core| core.endpoint)
                .map(|endpoint| endpoint.address),
            venue: inputs.venues.get(&server.id).map(Cow::Borrowed),
            status: core.map(|core| Cow::Borrowed(&core.status)),
            fault: core.and_then(|core| core.fault.as_ref()).map(Cow::Borrowed),
            startup: core.map(|core| core.startup),
        }
    }

    /// Own only the facts retained across renders.
    fn into_owned(self) -> RailCore<'static> {
        RailCore {
            group: Cow::Owned(self.group.into_owned()),
            membership: self.membership,
            availability: self.availability,
            endpoint_override: self
                .endpoint_override
                .map(|value| Cow::Owned(value.into_owned())),
            endpoint: self.endpoint,
            venue: self.venue.map(|value| Cow::Owned(value.into_owned())),
            status: self.status.map(|value| Cow::Owned(value.into_owned())),
            fault: self.fault.map(|value| Cow::Owned(value.into_owned())),
            startup: self.startup,
        }
    }
}

/// Per-window memo of the inputs and flattened virtual-list data.
#[derive(Default)]
pub(in crate::shell) struct RailMemo(Option<RailCache>);

/// Exact values that produced the cached model, including live membership in original order.
struct RailCache {
    fleet: FleetModelKey,
    cores: Vec<RailCore<'static>>,
    live: Vec<(CoreId, String)>,
    group: String,
    selected: Option<CoreId>,
    grouping: RosterGrouping,
    server_names: HashMap<String, String>,
    locale: String,
    logos_ready: bool,
    model: Rc<RailModel>,
}

/// Virtual items and stable counts, shared across frames while the inputs agree.
pub(in crate::shell) struct RailModel {
    pub(in crate::shell) items: Rc<[RailItem]>,
    pub(in crate::shell) summary: WorkspaceRosterSummary,
}

impl RailMemo {
    /// Check borrowed source facts on every draw and rebuild the rail only on a change.
    pub(in crate::shell) fn resolve<'a>(
        &mut self,
        inputs: &RailInputs<'a>,
        live: impl ExactSizeIterator<Item = (CoreId, &'a str)> + Clone,
        build: impl FnOnce() -> RailModel,
    ) -> Rc<RailModel> {
        let cores = inputs
            .config
            .servers
            .iter()
            .map(|server| RailCore::read(inputs, server));
        if let Some(cache) = &self.0
            && cache.group == inputs.group
            && cache.selected == inputs.selected
            && cache.grouping == inputs.grouping
            && cache.locale == inputs.locale
            && cache.logos_ready == inputs.logos_ready
            && (!inputs.grouping.by_server || &cache.server_names == inputs.server_names)
            && cache.fleet.matches(inputs.config, inputs.store)
            && cache.live.len() == live.len()
            && cache
                .live
                .iter()
                .zip(live.clone())
                .all(|((id, group), (new_id, new_group))| *id == new_id && group == new_group)
            && cache
                .cores
                .iter()
                .zip(cores.clone())
                .all(|(old, new)| old == &new)
        {
            return cache.model.clone();
        }
        let model = Rc::new(build());
        self.0 = Some(RailCache {
            fleet: FleetModelKey::capture(inputs.config, inputs.store),
            cores: cores.map(RailCore::into_owned).collect(),
            live: live.map(|(id, group)| (id, group.to_owned())).collect(),
            group: inputs.group.to_owned(),
            selected: inputs.selected,
            grouping: inputs.grouping,
            server_names: if inputs.grouping.by_server {
                inputs.server_names.clone()
            } else {
                HashMap::new()
            },
            locale: inputs.locale.to_owned(),
            logos_ready: inputs.logos_ready,
            model: model.clone(),
        });
        model
    }
}

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
// `Exchange` holds the logo and core list for one heading, shared by visible row renders.
#[derive(Clone, PartialEq, Eq)]
#[allow(clippy::large_enum_variant)]
pub(in crate::shell) enum RailItem {
    /// Current group aggregate scope.
    Overview { selected: bool },
    /// Machine heading of the optional server grouping, its name resolved before the virtual row
    /// closure.
    Server {
        /// Element identity, built once at flatten time and free of the address itself.
        id: SharedString,
        /// Custom name, `Server N`, or the unknown-server caption.
        label: SharedString,
        /// The one glyph Icon density draws: the ordinal for a default name, a custom name's first
        /// letter, `?` for the unknown block.
        short: SharedString,
        /// Position of this heading among the drawn headings — the run cell's stable identity.
        section: usize,
        /// Every core drawn under this machine, which its run cell commands.
        cores: Rc<[CoreId]>,
    },
    /// Venue heading and its logo, both resolved before the virtual row closure.
    Exchange {
        /// Element identity, keyed on the venue (and its server block), never on the caption.
        id: SharedString,
        venue: Option<CoreVenue>,
        logo: Option<Arc<RenderImage>>,
        /// Position of this heading among the drawn headings — the run cell's stable identity.
        section: usize,
        /// Every core drawn under this heading, which its run cell commands.
        cores: Rc<[CoreId]>,
        /// Drawn one level in, under a server heading.
        nested: bool,
    },
    /// Configured core row and whether its branch stem ends at this leaf.
    Core {
        row: WorkspaceRosterRow,
        is_last_in_section: bool,
        /// The one core this row commands, built once when the rail is flattened — the item builder
        /// runs per visible row on every frame and must not allocate.
        cores: Rc<[CoreId]>,
        /// Drawn one level in, under an exchange heading that is itself under a server heading.
        nested: bool,
    },
}

/// The machine a core runs on, as the rail's server grouping keys it.
///
/// The address its feed reported wins: that is the one Core Status groups and names by. Until one
/// is known — the core is off, or a reconnect just cleared it (`CoreData::begin_connection_attempt`
/// drops the endpoint on every new attempt) — the address the core's own configuration names is
/// used instead, so a reconnecting core keeps its place rather than flickering into the unknown
/// block. That fallback never resolves a name, so a host name stays unknown until the feed reports;
/// a key naming no address falls back to localhost, the address the feed itself dials then.
///
/// Args:
///     server: The core's configuration.
///     core: Its live store entry, if any.
///
/// Returns:
///     The machine's address, or `None` when neither the feed nor the configuration names an IP.
pub(super) fn core_machine(
    server: &moon_core::config::ServerConfig,
    core: Option<&moon_core::session::store::CoreData>,
) -> Option<std::net::IpAddr> {
    if let Some(endpoint) = core.and_then(|core| core.endpoint) {
        return Some(endpoint.address);
    }
    let endpoint = moon_core::config::parse_endpoint_override(&server.endpoint_override).ok()?;
    let target = moon_core::config::target_from_key(server.key.expose(), endpoint.as_ref())?;
    match target.host {
        moon_core::config::CoreHost::Ip(ip) => Some(ip),
        moon_core::config::CoreHost::Name(_) => None,
    }
}

/// Flatten grouped roster blocks into virtual-list items, after the Overview row.
///
/// Args:
///     items: Destination, already holding the Overview row.
///     servers: Grouped roster blocks in display order.
///     server_label: Resolves a machine heading's caption and its Icon-density glyph.
///     server_id: Resolves a machine's identity fragment for element ids (`"unknown"` or an
///         ordinal), never its address.
///     logo: Resolves an exchange heading's logo, `None` while logos are still loading.
///
/// Returns:
///     Nothing; one `Server` item leads each server block when server grouping is on, one
///     `Exchange` item leads each exchange section when exchange grouping is on, and every heading
///     takes the next run-cell position in drawing order.
pub(super) fn append_roster_items(
    items: &mut Vec<RailItem>,
    servers: Vec<WorkspaceRosterServer>,
    server_label: impl Fn(RosterServer) -> (SharedString, SharedString),
    server_id: impl Fn(RosterServer) -> String,
    logo: impl Fn(Option<&CoreVenue>) -> Option<Arc<RenderImage>>,
) {
    let mut section = 0;
    for block in servers {
        let block_id = block.server.map(&server_id);
        if let Some(server) = block.server {
            let cores: Rc<[CoreId]> = block
                .sections
                .iter()
                .flat_map(|section| section.rows.iter().map(|row| row.core))
                .collect();
            let (label, short) = server_label(server);
            items.push(RailItem::Server {
                id: SharedString::from(format!(
                    "workspace-server-{}",
                    block_id.as_deref().unwrap_or_default()
                )),
                label,
                short,
                section,
                cores,
            });
            section += 1;
        }
        for roster_section in block.sections {
            let nested_core = match roster_section.heading {
                RosterSectionHeading::None => false,
                RosterSectionHeading::Exchange(venue) => {
                    // Unchanged from before server grouping existed when there is no server block,
                    // so a heading keeps its hover state and its atlas identity across the upgrade.
                    let venue_key = match venue.as_ref() {
                        Some(venue) => format!("{}-{}", venue.id.code, venue.id.dex),
                        None => "unknown".to_string(),
                    };
                    let id = match block_id.as_deref() {
                        Some(server) => format!("workspace-exchange-s{server}-{venue_key}"),
                        None => format!("workspace-exchange-{venue_key}"),
                    };
                    let cores: Rc<[CoreId]> =
                        roster_section.rows.iter().map(|row| row.core).collect();
                    items.push(RailItem::Exchange {
                        id: SharedString::from(id),
                        logo: logo(venue.as_ref()),
                        venue,
                        section,
                        cores,
                        nested: block.server.is_some(),
                    });
                    section += 1;
                    block.server.is_some()
                }
            };
            append_core_section_items(items, roster_section.rows, nested_core);
        }
    }
}

/// What every rail line needs to draw its run cell, resolved once per rail render.
#[derive(Clone, Copy)]
pub(super) struct RailRun {
    /// Slots every line of the rail reserves at the current density.
    pub(super) slots: RunSlots,
    /// Whether a group heading — exchange or server — also fills them for all its cores.
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

/// Append one section's cores while marking exactly its final core as the branch endpoint.
///
/// Args:
///     items: Flattened rail destination.
///     rows: Ordered configured cores belonging to one heading.
///     nested: Whether the rows sit one level in, under a nested exchange heading.
///
/// Returns:
///     Nothing; rows retain their order and only the last receives the terminal shape.
pub(super) fn append_core_section_items(
    items: &mut Vec<RailItem>,
    rows: Vec<WorkspaceRosterRow>,
    nested: bool,
) {
    let last_index = rows.len().checked_sub(1);
    items.extend(rows.into_iter().enumerate().map(|(index, row)| {
        let cores = Rc::from([row.core]);
        RailItem::Core {
            row,
            is_last_in_section: Some(index) == last_index,
            cores,
            nested,
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

#[cfg(test)]
mod tests;
