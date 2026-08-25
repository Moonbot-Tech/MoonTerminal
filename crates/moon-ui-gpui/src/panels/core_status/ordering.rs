//! Pure ordering and naming helpers for the Core Status panel: server display names, the flat
//! table's column comparators, and natural (human) name ordering. No view state, so they live apart
//! from `mod.rs`.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::net::IpAddr;

use moon_core::feed::ConnStatus;
use moon_core::session::{CoreId, CoreSysStatus};
use moon_core::venue::{Brand, CoreVenue};
use rust_i18n::t;

use crate::core_order::ExchangeSection;

use super::model::{CoreStatusRow, GroupVersion, ServerStatusGroup};
use super::startup::{StartupCell, startup_cell};

/// Which By IP column the server list is sorted on. Warnings always pin to the top regardless of the
/// field (handled by the caller), so this only orders within the warned and the quiet partitions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum GroupSortField {
    /// Server display name — the default, in natural order.
    Name,
    /// Whole-machine system CPU percent.
    Cpu,
    /// Free-memory share of the reconstructed machine total (the "Память своб." column).
    Mem,
    /// Worst client↔core round-trip among the server's ready cores.
    Ping,
    /// Worst core→exchange latency among the server's ready cores.
    Exch,
    /// Ready core count.
    Cores,
    /// The server's most urgent API key, by [`ApiKeyState::urgency`] — which may be a day count,
    /// or neither when nothing is known or every key is unlimited.
    ApiKey,
    /// Startup: still-coming-up servers first, then the ones that took longest. One header click
    /// answers "which machines are slow to come up", which is the question the column exists for.
    Startup,
    /// The server's rolled-up MoonBot build, by [`version_group_rank`] — the agreed build, or
    /// neither when its cores disagree or none reported.
    Version,
}

impl GroupSortField {
    /// Return the stable persistence key for one By-IP sort column.
    pub(super) fn key(self) -> &'static str {
        match self {
            Self::Name => "name",
            Self::Cpu => "cpu",
            Self::Mem => "mem",
            Self::Ping => "ping",
            Self::Exch => "exch",
            Self::Cores => "cores",
            Self::ApiKey => "api_key",
            Self::Startup => "startup",
            Self::Version => "version",
        }
    }

    /// Resolve a persisted By-IP key without treating an unknown value as Name.
    pub(super) fn from_key(key: &str) -> Option<Self> {
        match key {
            "name" => Some(Self::Name),
            "cpu" => Some(Self::Cpu),
            "mem" => Some(Self::Mem),
            "ping" => Some(Self::Ping),
            "exch" => Some(Self::Exch),
            "cores" => Some(Self::Cores),
            "api_key" => Some(Self::ApiKey),
            "startup" => Some(Self::Startup),
            "version" => Some(Self::Version),
            _ => None,
        }
    }
}

/// Restore a valid Flat-mode sort, leaving `None` as the historical attention order.
pub(super) fn restore_flat_sort(
    preference: Option<moon_core::config::TableSortPreference>,
) -> Option<(String, bool)> {
    const KEYS: [&str; 13] = [
        "server",
        "core",
        "status",
        "cpu_proc",
        "cpu_sys",
        "mem_used",
        "free_phys",
        "ping",
        "ping_exch",
        "cpus",
        "api_key",
        "startup",
        "version",
    ];
    preference.and_then(|preference| {
        KEYS.contains(&preference.column.as_str())
            .then_some((preference.column, preference.ascending))
    })
}

/// Restore a valid By-IP sort, falling back to its historical Name-ascending order.
pub(super) fn restore_group_sort(
    preference: Option<moon_core::config::TableSortPreference>,
) -> (GroupSortField, bool) {
    preference
        .and_then(|preference| {
            GroupSortField::from_key(&preference.column).map(|field| (field, preference.ascending))
        })
        .unwrap_or((GroupSortField::Name, true))
}

/// Worst (highest) latency among a group's READY cores for one accessor, matching the value the
/// server row surfaces. `None` when no ready core has the reading.
fn worst_latency(
    group: &ServerStatusGroup,
    read: impl Fn(&CoreSysStatus) -> Option<u32>,
) -> Option<u32> {
    group
        .cores
        .iter()
        .filter(|core| core.status == ConnStatus::Ready)
        .filter_map(|core| read(&core.sys))
        .max()
}

/// Free-memory percentage of the reconstructed machine total (process RAM sum + free physical),
/// matching the "Память своб." column. `None` until free memory has arrived, so such servers group
/// together at the ascending end.
fn free_pct(group: &ServerStatusGroup) -> Option<u64> {
    let free_mb = u64::from(group.free_physical_memory_mb?);
    let total_mb = group.process_memory_mb.unwrap_or(0) + free_mb;
    if total_mb == 0 {
        return Some(0);
    }
    Some(free_mb * 100 / total_mb)
}

/// Compare two server groups on one sort field, ascending. The caller reverses for descending and
/// applies the warnings-first pin separately; a name tiebreak keeps the order stable when the field
/// ties (so equal metrics don't reshuffle each tick).
///
/// Args:
///     a: First group.
///     b: Second group.
///     field: The active sort column.
///
/// Returns:
///     The ascending ordering for that field, then by name.
pub(super) fn compare_groups(
    a: &ServerStatusGroup,
    b: &ServerStatusGroup,
    field: GroupSortField,
) -> Ordering {
    match field {
        GroupSortField::Name => Ordering::Equal,
        GroupSortField::Cpu => a.system_cpu_percent.cmp(&b.system_cpu_percent),
        GroupSortField::Mem => free_pct(a).cmp(&free_pct(b)),
        GroupSortField::Ping => worst_latency(a, |sys| sys.round_trip_ms)
            .cmp(&worst_latency(b, |sys| sys.round_trip_ms)),
        GroupSortField::Exch => worst_latency(a, |sys| sys.order_api_latency_ms.map(u32::from))
            .cmp(&worst_latency(b, |sys| {
                sys.order_api_latency_ms.map(u32::from)
            })),
        GroupSortField::Cores => a
            .ready_count
            .cmp(&b.ready_count)
            .then_with(|| a.cores.len().cmp(&b.cores.len())),
        // The very key the server row displays, ordered the same way — so the column cannot sort by
        // one thing and show another. Not Ready-gated, unlike the latencies: a key keeps ageing
        // while its core is down.
        GroupSortField::ApiKey => a.api_key.urgency().cmp(&b.api_key.urgency()),
        // Rank on the SAME cell the server row displays, so the column cannot sort by one thing and
        // show another. `startup_rank` puts unfinished startups first because they are the ones
        // still costing the user time.
        GroupSortField::Startup => startup_rank(a.startup).cmp(&startup_rank(b.startup)),
        // Ranks the SAME rolled-up value the server row displays, by the same rule as every other
        // heading here. Not Ready-gated: the store already drops a build the moment its core leaves
        // Ready, so a stale one cannot reach this comparison.
        GroupSortField::Version => {
            version_group_rank(a.version).cmp(&version_group_rank(b.version))
        }
    }
    .then_with(|| natural_cmp(&a.display_name, &b.display_name))
}

/// Rank one startup cell for sorting: unfinished startups first (least progress, then longest
/// running), then finished ones by how long they took, then rows with nothing to report.
///
/// A single total key rather than a comparator, so the ordering stays transitive however the cell
/// variants grow.
fn startup_rank(cell: Option<StartupCell>) -> (u8, i64, i64) {
    match cell {
        Some(StartupCell::Progress {
            done,
            total,
            elapsed_ms,
        }) => {
            // Share of the work still outstanding, scaled so a fractional comparison stays integral.
            let remaining = i64::from(total.saturating_sub(done)) * 1000 / i64::from(total.max(1));
            (0, -remaining, -(elapsed_ms as i64))
        }
        Some(StartupCell::Done { elapsed_ms }) => (1, -(elapsed_ms as i64), 0),
        Some(StartupCell::Absent) | None => (2, 0, 0),
    }
}

/// Rank one core's reported build for sorting: reported builds first, ascending, then the rows
/// with nothing to show.
///
/// A tagged tuple rather than the bare `Option`, for [`ApiKeyState::urgency`]'s stated reason:
/// `None` sorts FIRST as an `Option`, which is the opposite of what this column is scanned for.
/// The scan is "which cores run an odd or old build", so the numbers lead.
///
/// KNOWN AND ACCEPTED: `sorted_flat_rows` reverses the whole comparator, so descending leads with
/// the blanks. `api_key` behaves identically for the same reason, and matching it keeps one rule in
/// the panel rather than making this the single column that behaves differently.
fn version_rank(version: Option<u32>) -> (u8, u32) {
    match version {
        Some(version) => (0, version),
        None => (1, 0),
    }
}

/// Rank a server's rolled-up build: an agreed build first, ascending, then disagreement, then
/// nothing reported.
///
/// `Mixed` outranks `Absent` because a mixed group has something to look at — expanding it shows
/// real numbers — while an absent one does not.
fn version_group_rank(version: GroupVersion) -> (u8, u32) {
    match version {
        GroupVersion::Uniform(version) => (0, version),
        GroupVersion::Mixed => (1, 0),
        GroupVersion::Absent => (2, 0),
    }
}

/// Fill each group's display name from a custom name or a stable `Server N` ordinal.
///
/// Ordinals rank address servers by sorted address so a name stays put under attention-first
/// reordering. Unknown-endpoint servers keep a core-qualified fallback label.
///
/// Args:
///     groups: Aggregated server snapshots to name in place.
///     names: Custom names keyed by endpoint IP string.
///
/// Returns:
///     Nothing; `display_name` is set on every group.
pub(super) fn assign_server_names(
    groups: &mut [ServerStatusGroup],
    names: &HashMap<String, String>,
) {
    let mut addresses = groups
        .iter()
        .filter_map(|group| group.address)
        .collect::<Vec<IpAddr>>();
    addresses.sort();
    for group in groups.iter_mut() {
        group.display_name = match group.address {
            Some(address) => {
                let ip = address.to_string();
                names.get(&ip).cloned().unwrap_or_else(|| {
                    let ordinal = addresses
                        .iter()
                        .position(|candidate| *candidate == address)
                        .map(|index| index + 1)
                        .unwrap_or(0);
                    t!("core_status.server_n", n = ordinal).to_string()
                })
            }
            None => {
                let core = group
                    .cores
                    .first()
                    .map(|core| core.name.as_str())
                    .unwrap_or("-");
                t!("core_status.unknown_server", core = core).to_string()
            }
        };
    }
}

/// Return a stable ordinal for sorting the flat table's status column.
///
/// Args:
///     s: Current connection status.
///
/// Returns:
///     A rank placing ready cores first and failed cores last.
fn status_ord(s: &ConnStatus) -> u64 {
    match s {
        ConnStatus::Ready => 0,
        ConnStatus::Connecting | ConnStatus::Stage(_) => 1,
        ConnStatus::Disconnected => 2,
        ConnStatus::Failed(_) => 3,
    }
}

/// Compare two names in natural order: digit runs compare as numbers, everything else
/// case-insensitively. So `Server 2` sorts before `Server 10`, and `F1` before `Server 1`.
///
/// Args:
///     a: First name.
///     b: Second name.
///
/// Returns:
///     The natural ordering of the two names.
pub(super) fn natural_cmp(a: &str, b: &str) -> Ordering {
    let mut a = a.chars().peekable();
    let mut b = b.chars().peekable();
    loop {
        match (a.peek().copied(), b.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(ca), Some(cb)) if ca.is_ascii_digit() && cb.is_ascii_digit() => {
                // Compare digit runs numerically without parsing: strip leading zeros, then longer
                // run wins, then lexically.
                let da = take_digits(&mut a);
                let db = take_digits(&mut b);
                let na = da.trim_start_matches('0');
                let nb = db.trim_start_matches('0');
                match na.len().cmp(&nb.len()).then_with(|| na.cmp(nb)) {
                    Ordering::Equal => {}
                    ord => return ord,
                }
            }
            (Some(ca), Some(cb)) => match ca.to_ascii_lowercase().cmp(&cb.to_ascii_lowercase()) {
                Ordering::Equal => {
                    a.next();
                    b.next();
                }
                ord => return ord,
            },
        }
    }
}

/// Consume and return a run of consecutive ASCII digits from a peekable char iterator.
fn take_digits<I: Iterator<Item = char>>(it: &mut std::iter::Peekable<I>) -> String {
    let mut digits = String::new();
    while let Some(&c) = it.peek() {
        if c.is_ascii_digit() {
            digits.push(c);
            it.next();
        } else {
            break;
        }
    }
    digits
}

/// Compare two flat rows on one column key for header-click sorting.
///
/// `None` metrics sort before any value, so cores that have not reported a field group together.
///
/// Args:
///     a: First row.
///     b: Second row.
///     key: Column key from the table header.
///
/// Returns:
///     The ascending ordering for that column.
pub(super) fn compare_flat_rows(a: &CoreStatusRow, b: &CoreStatusRow, key: &str) -> Ordering {
    match key {
        // "server" is handled by name in `sorted_flat_rows`, not here.
        "status" => status_ord(&a.status).cmp(&status_ord(&b.status)),
        "cpu_proc" => a.sys.process_cpu_percent.cmp(&b.sys.process_cpu_percent),
        "cpu_sys" => a.sys.system_cpu_percent.cmp(&b.sys.system_cpu_percent),
        "mem_used" => a.sys.used_memory_mb.cmp(&b.sys.used_memory_mb),
        "free_phys" => a
            .sys
            .free_physical_memory_mb
            .cmp(&b.sys.free_physical_memory_mb),
        "ping" => a.sys.round_trip_ms.cmp(&b.sys.round_trip_ms),
        "ping_exch" => a.sys.order_api_latency_ms.cmp(&b.sys.order_api_latency_ms),
        "cpus" => a.sys.logical_cpu_count.cmp(&b.sys.logical_cpu_count),
        // By URGENCY, not by the cell text and not by the raw number: "9" must not sort above "45",
        // an expired key leads, and the two states with no number must trail the counts rather than
        // heading the column — a dash and an infinity are the LAST things to look at here.
        "api_key" => a.api_key.urgency().cmp(&b.api_key.urgency()),
        // Same rank the By-IP column sorts by, over the per-core cell, so the two modes cannot
        // disagree about which core is slower to come up.
        "startup" => startup_rank(Some(startup_cell(&a.status, &a.startup)))
            .cmp(&startup_rank(Some(startup_cell(&b.status, &b.startup)))),
        // The reported build, numerically rather than lexically, with the blanks kept off the head
        // of the ascending scan — see `version_rank`.
        "version" => version_rank(a.server_version).cmp(&version_rank(b.server_version)),
        // "core" and any unknown key sort by name.
        _ => a.name.cmp(&b.name),
    }
}

/// One line of the Flat presentation, in render order.
///
/// The flat table draws cores AND the exchange headings that introduce them from one list, because
/// [`MoonDataTable`] has no notion of a group row: every line it draws is a row of uniform height,
/// so a heading has to BE a row. Keeping both in one enum makes a table index resolve to the line
/// it draws, without a second list that could fall out of step with the core rows.
///
/// [`MoonDataTable`]: moon_ui::MoonDataTable
pub(super) enum FlatLine {
    /// An exchange heading, introducing the cores that follow it.
    Section(FlatSection),
    /// Index into the sorted row slice this line draws.
    Core(usize),
}

/// What one exchange heading draws.
///
/// Owned rather than borrowed: the table's row closure is `'static`, so it cannot hold a
/// `&CoreVenue` borrowed out of the session's venue map.
pub(super) struct FlatSection {
    /// The bucket's IDENTITY. The heading's element id is built from this and never from
    /// [`Self::label`]: an id built from rendered text changes with the interface language and with
    /// a core build's spelling, which makes GPUI treat one heading as a different element and drop
    /// its hover and tooltip state.
    pub(super) section: ExchangeSection,
    /// Caption, from [`crate::controls::venue_section_label`].
    pub(super) label: String,
    /// Brand whose logo the heading shows, when the directory names one. `None` draws no logo and
    /// deliberately no placeholder glyph.
    pub(super) brand: Option<Brand>,
    /// How many cores this section holds.
    ///
    /// Drawn right-aligned at the far end of the band, unless that is also the caption cell, where
    /// it follows the caption. This makes a heading read unmistakably as a GROUP rather than as
    /// another core: MoonUI's sort arrow cannot say that it orders rows WITHIN a section, so the
    /// heading has to carry that meaning itself.
    pub(super) members: usize,
}

/// Lay already-sorted flat rows out as exchange sections followed by their members.
///
/// Sorting orders rows; grouping PARTITIONS that order. The caller applies the active column sort
/// (or the default attention-first order) to `rows` first, and this function only cuts the result
/// into sections — so a descending click reverses rows WITHIN each section and never moves a
/// section. Section order comes from the shared directory ordering in
/// [`crate::core_order::exchange_sections`], the same one the left rail, the Strategies tree and
/// the Assets panel already use, so a sort click here can never make this panel disagree with them
/// about where an exchange sits.
///
/// Bucketing is by venue IDENTITY, not by the caption a core reported, so two cores of different
/// vintage on one venue share a section however their builds spell its name.
///
/// Args:
///     rows: Flat rows in their final display order.
///     venues: What each core reported it is connected to, keyed by core.
///
/// Returns:
///     Heading and member lines in render order; empty when `rows` is empty. A section is emitted
///     only when it has members, so a heading with nothing under it is not representable.
pub(super) fn flat_lines(
    rows: &[CoreStatusRow],
    venues: &HashMap<CoreId, CoreVenue>,
) -> Vec<FlatLine> {
    let sections =
        crate::core_order::exchange_sections(rows.iter().enumerate().map(|(index, row)| {
            let venue = venues.get(&row.id);
            (index, venue)
        }));
    // One heading plus every member, so the exact final length is known up front.
    let mut lines = Vec::with_capacity(rows.len() + sections.len());
    for (venue, members) in sections {
        lines.push(FlatLine::Section(FlatSection {
            // Through the shared bucketing rule rather than re-deciding here what "unidentified"
            // means, so the heading and the partition it heads cannot drift apart.
            section: crate::core_order::section_of(venue),
            label: stable_section_label(venue, &members, rows, venues),
            // Identity, not caption: every member of one section shares an `ExchangeId`, so the
            // brand is the same whichever member the partition handed back.
            brand: venue.and_then(CoreVenue::brand),
            members: members.len(),
        }));
        lines.extend(members.into_iter().map(FlatLine::Core));
    }
    lines
}

/// Caption a section so that the active row sort cannot rename it.
///
/// A venue the directory NAMES captions from the directory, so every member spells it identically.
/// A venue nothing names falls back to the core's own wire text, and members of one ordinal can
/// disagree about it — two cores on the same unknown platform can report two spellings.
/// [`crate::core_order::exchange_sections`] hands back the FIRST member's venue, and "first" moves
/// with the active column sort, so captioning from it would make such a heading rename itself when
/// the user clicks a sort arrow.
///
/// The smallest RENDERED caption is stable under every row order. It is compared as the caption
/// rather than as the underlying wire field on purpose: a core build's own spelling belongs to
/// [`crate::controls::venue_label`] and is not this module's to read — a rule the theme contract
/// enforces across the whole crate.
///
/// Args:
///     venue: The section's representative venue, or `None` for the unidentified group.
///     members: Row indices belonging to this section.
///     rows: The rows those indices address.
///     venues: What each core reported it is connected to, keyed by core.
///
/// Returns:
///     The caption to draw, never empty.
fn stable_section_label(
    venue: Option<&CoreVenue>,
    members: &[usize],
    rows: &[CoreStatusRow],
    venues: &HashMap<CoreId, CoreVenue>,
) -> String {
    let label = crate::controls::venue_section_label(venue);
    // Only an unnameable ordinal can disagree between members; everything else is already stable,
    // and formatting one caption per member would be waste.
    if venue.is_none_or(|venue| venue.resolved().is_some()) {
        return label;
    }
    members
        .iter()
        .filter_map(|index| rows.get(*index))
        .filter_map(|row| venues.get(&row.id))
        .map(|venue| crate::controls::venue_section_label(Some(venue)))
        .min()
        .unwrap_or(label)
}

#[cfg(test)]
mod tests;
