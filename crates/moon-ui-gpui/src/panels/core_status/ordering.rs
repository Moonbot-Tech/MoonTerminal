//! Pure ordering and naming helpers for the Core Status panel: server display names, the flat
//! table's column comparators, and natural (human) name ordering. No view state, so they live apart
//! from `mod.rs`.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::net::IpAddr;
use std::rc::Rc;

use moon_core::feed::ConnStatus;
use moon_core::session::CoreId;
use moon_core::venue::{Brand, CoreVenue};
use rust_i18n::t;

use moon_core::session::core_order::{self, ExchangeSection};

use super::model::{CoreStatusRow, ServerStatusGroup};
use super::startup::{StartupCell, startup_cell};
use super::time_offset::{tz_offset_cell, tz_offset_rank};

/// Restore a valid Flat-mode sort, leaving `None` as the historical attention order.
pub(super) fn restore_flat_sort(
    preference: Option<moon_core::config::TableSortPreference>,
) -> Option<(String, bool)> {
    const KEYS: [&str; 15] = [
        "status",
        "core",
        "server",
        "version",
        "ping",
        "ping_exch",
        "cpu_proc",
        "cpu_sys",
        "cpus",
        "mem_used",
        "free_phys",
        "api_key",
        "api_quota",
        "startup",
        "tz_off",
    ];
    preference.and_then(|preference| {
        KEYS.contains(&preference.column.as_str())
            .then_some((preference.column, preference.ascending))
    })
}

/// Median ping and connectivity of one exchange section.
///
/// `median_ping_ms` is the middle Ready ping. An odd count takes that one sample. An even count
/// takes the mean of the two middle samples, rounded half up (`(a + b + 1) / 2`). Only Ready
/// cores with a known core ping contribute. A core that is not Ready counts as not connected
/// even when a stale ping is still stored on the row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct SectionAggregate {
    /// Cores in the section, whatever their connection.
    pub(super) cores: usize,
    /// Middle Ready ping, in milliseconds, or `None` when no Ready core has a ping.
    /// An even count is the half-up mean of the two middle samples.
    pub(super) median_ping_ms: Option<u32>,
    /// Cores whose connection is not [`ConnStatus::Ready`].
    pub(super) not_connected: usize,
}

/// Summarise one exchange section for its heading.
///
/// Args:
///     rows: The cores that belong to the section, in any order.
///
/// Returns:
///     The count, the median Ready ping, and how many cores are not connected.
pub(super) fn section_aggregate(rows: &[&CoreStatusRow]) -> SectionAggregate {
    let mut pings: Vec<u32> = rows
        .iter()
        .filter(|row| row.status == ConnStatus::Ready)
        .filter_map(|row| row.sys.round_trip_ms)
        .collect();
    pings.sort_unstable();
    let median_ping_ms = match pings.len() {
        0 => None,
        n if n % 2 == 1 => Some(pings[n / 2]),
        n => {
            let lower = u64::from(pings[n / 2 - 1]);
            let upper = u64::from(pings[n / 2]);
            Some((lower + upper).div_ceil(2) as u32)
        }
    };
    let not_connected = rows
        .iter()
        .filter(|row| row.status != ConnStatus::Ready)
        .count();
    SectionAggregate {
        cores: rows.len(),
        median_ping_ms,
        not_connected,
    }
}

/// The heading sentence for one [`SectionAggregate`].
///
/// The median clause is omitted when no Ready core has a ping. A section with nothing offline
/// says that everyone is connected instead of printing a zero.
///
/// Args:
///     aggregate: The summary from [`section_aggregate`].
///
/// Returns:
///     The localized caption that follows the exchange name.
pub(super) fn section_aggregate_text(aggregate: &SectionAggregate) -> String {
    let n = aggregate.cores as u64;
    match (aggregate.median_ping_ms, aggregate.not_connected) {
        (Some(ping), 0) => {
            let key = super::presentation::plural_key("core_status.section.aggregate_ok", n);
            t!(key.as_str(), n = aggregate.cores, ping = ping).to_string()
        }
        (Some(ping), k) => {
            let key = super::presentation::plural_key("core_status.section.aggregate", n);
            t!(key.as_str(), n = aggregate.cores, ping = ping, k = k).to_string()
        }
        (None, 0) => {
            let key = super::presentation::plural_key("core_status.section.aggregate_ok_nomed", n);
            t!(key.as_str(), n = aggregate.cores).to_string()
        }
        (None, k) => {
            let key = super::presentation::plural_key("core_status.section.aggregate_nomed", n);
            t!(key.as_str(), n = aggregate.cores, k = k).to_string()
        }
    }
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
/// with nothing to show. The letter is the third key, so equal numbers order `None` < `""` < a
/// name. The number stays first, so `7.70 R3` sorts before `7.71`. A missing number drops the
/// letter: an unreported row has nothing to order by.
///
/// A tagged tuple rather than the bare `Option`, for [`ApiKeyState::urgency`]'s stated reason:
/// `None` sorts FIRST as an `Option`, which is the opposite of what this column is scanned for.
/// The scan is "which cores run an odd or old build", so the numbers lead.
///
/// KNOWN AND ACCEPTED: `sorted_flat_rows` reverses the whole comparator, so descending leads with
/// the blanks. `api_key` behaves identically for the same reason, and matching it keeps one rule in
/// the panel rather than making this the single column that behaves differently.
///
/// Args:
///     version: The build this core reported, when it reported one.
///     suffix: The letter reported with that build.
///
/// Returns:
///     Sort category, build number, then the letter. The letter is `None` when the number is.
fn version_rank(version: Option<u32>, suffix: Option<&str>) -> (u8, u32, Option<&str>) {
    match version {
        Some(version) => (0, version, suffix),
        None => (1, 0, None),
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
        // Smallest quota first, with the cores that publish none pushed to the tail rather than the
        // head: an absent number is the last thing to look at, exactly as for `api_key` above.
        "api_quota" => a
            .api_quota
            .unwrap_or(u64::MAX)
            .cmp(&b.api_quota.unwrap_or(u64::MAX)),
        // Same rank the By-IP column sorts by, over the per-core cell, so the two modes cannot
        // disagree about which core is slower to come up.
        "startup" => startup_rank(Some(startup_cell(&a.status, &a.startup)))
            .cmp(&startup_rank(Some(startup_cell(&b.status, &b.startup)))),
        // The reported build, numerically rather than lexically, with the blanks kept off the head
        // of the ascending scan — see `version_rank`.
        // `Option<&str>` orders `None` < `Some("")` < `Some("R3")`. Both rows share this
        // function's lifetime, so the rank tuples compare directly.
        "version" => version_rank(a.server_version, a.server_version_suffix.as_deref()).cmp(
            &version_rank(b.server_version, b.server_version_suffix.as_deref()),
        ),
        // Same rank the By-IP column sorts by, over the per-core cell, so the two modes cannot
        // disagree about which core is ahead of or behind UTC.
        "tz_off" => tz_offset_rank(tz_offset_cell(&a.time_offset))
            .cmp(&tz_offset_rank(tz_offset_cell(&b.time_offset))),
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
    /// The heading draws [`Self::aggregate`], which carries this same count. The field stays so
    /// the ordering tests can read the partition size without parsing that sentence. The binary
    /// target does not read it.
    #[allow(dead_code)]
    pub(super) members: usize,
    /// Count, median ping, and not-connected tally for the caption.
    pub(super) aggregate: SectionAggregate,
}

/// Lay already-sorted flat rows out as exchange sections followed by their members.
///
/// Sorting orders rows; grouping PARTITIONS that order. The caller applies the active column sort
/// (or the default attention-first order) to `rows` first, and this function only cuts the result
/// into sections — so a descending click reverses rows WITHIN each section and never moves a
/// section. Section order comes from the shared directory ordering in
/// [`core_order::exchange_sections`], the same one the left rail, the Strategies tree and
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
    let sections = core_order::exchange_sections(rows.iter().enumerate().map(|(index, row)| {
        let venue = venues.get(&row.id);
        (index, venue)
    }));
    // One heading plus every member, so the exact final length is known up front.
    let mut lines = Vec::with_capacity(rows.len() + sections.len());
    for (venue, members) in sections {
        let member_rows: Vec<&CoreStatusRow> = members
            .iter()
            .filter_map(|index| rows.get(*index))
            .collect();
        // The caption count is the partition size. A missing index must not shrink it: the
        // heading was built to name every member, and `section_row` no longer overwrites this.
        let mut aggregate = section_aggregate(&member_rows);
        aggregate.cores = members.len();
        lines.push(FlatLine::Section(FlatSection {
            // Through the shared bucketing rule rather than re-deciding here what "unidentified"
            // means, so the heading and the partition it heads cannot drift apart.
            section: core_order::section_of(venue),
            label: stable_section_label(venue, &members, rows, venues),
            // Identity, not caption: every member of one section shares an `ExchangeId`, so the
            // brand is the same whichever member the partition handed back.
            brand: venue.and_then(CoreVenue::brand),
            members: members.len(),
            aggregate,
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
/// [`core_order::exchange_sections`] hands back the FIRST member's venue, and "first" moves
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

/// Project the flat table's LINES onto the cores they draw.
///
/// `MoonDataTable` addresses every line it draws by index, exchange headings included, so a line
/// index is NOT a core index. This is the translation the selection needs: a heading becomes
/// `None`, which the click algorithm treats as "not a selectable row" and ignores, and a member
/// becomes its core's stable id. Selection identity is the CORE, never the line -- a sort or a
/// re-group moves every line index and would silently retarget a selection built from them.
///
/// Args:
///     lines: Headings and member rows in render order, as [`flat_lines`] laid them out.
///     rows: The already-sorted rows those lines address.
///
/// Returns:
///     One entry per line, parallel to `lines`.
pub(super) fn flat_order(lines: &[FlatLine], rows: &[CoreStatusRow]) -> Vec<Option<CoreId>> {
    lines
        .iter()
        .map(|line| match line {
            FlatLine::Section(_) => None,
            FlatLine::Core(row) => rows.get(*row).map(|row| row.id),
        })
        .collect()
}

/// Collect the cores one exchange heading introduces.
///
/// Reads the layout invariant [`flat_lines`] builds: a heading is followed by exactly its own
/// members, and the next heading ends the run. Walking the lines is what keeps this honest -- the
/// heading's own `members` count says how many there are but not WHICH, and the two must never be
/// able to disagree about a scope that reaches live cores.
///
/// Args:
///     lines: Headings and member rows in render order.
///     heading: Index of the heading line the user acted on.
///     rows: The rows those lines address.
///
/// Returns:
///     The section's cores in render order. Empty when `heading` does not address a heading.
pub(super) fn section_cores(
    lines: &[FlatLine],
    heading: usize,
    rows: &[CoreStatusRow],
) -> Rc<[CoreId]> {
    if !matches!(lines.get(heading), Some(FlatLine::Section(_))) {
        return Rc::from(Vec::new());
    }
    let cores: Vec<CoreId> = lines[heading + 1..]
        .iter()
        .map_while(|line| match line {
            FlatLine::Core(row) => Some(rows.get(*row).map(|row| row.id)),
            FlatLine::Section(_) => None,
        })
        .flatten()
        .collect();
    Rc::from(cores)
}

#[cfg(test)]
mod tests;
