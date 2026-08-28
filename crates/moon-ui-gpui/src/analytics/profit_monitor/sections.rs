//! Splitting the by-core table into the user's saved core groups, each with its own subtotal.
//!
//! Pure, like [`super::rows`]: it takes finished rows plus the configuration context and returns
//! the flat sequence the table draws. Nothing here reads view state or touches SQLite — a group is
//! a display axis over rows the report already produced.
//!
//! The sequence is FLAT rather than a list of sections because the table is virtualized on one
//! fixed row height (`MoonVirtualList`): a caption and a subtotal are rows like any other, and a
//! nested shape would only have to be flattened again at the one place that draws it.
//!
//! Five rules the tests pin down, because each is a decision rather than an implementation detail:
//! - a core saved into two groups appears in BOTH, and its numbers count toward both subtotals. The
//!   window's grand-total footer is therefore NOT the sum of the subtotals: its caller folds it
//!   from the rows BEFORE they reach this module, so every core counts once there however many
//!   groups name it;
//! - sections are ordered by NAME, while the ungrouped remainder always comes last, wherever its
//!   localized caption would otherwise sort;
//! - the table is left unsectioned whenever sectioning would produce ONE caption over everything —
//!   no saved groups, none that names a visible row, or a single group holding every visible row.
//!   That shape only adds a subtotal that repeats the footer;
//! - a section of one row gets no subtotal, for the same reason: it would restate the row above it;
//! - a section keeps the row order it was handed, never the order its members were saved in, so a
//!   section and the remainder below it can never read in two different orders.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use moon_core::config::CoreGroup;
use moon_core::session::CoreId;

use super::MonitorSort;
use super::rows::{LiveContext, MonitorRow, fold_total};
use super::sort_rows;

#[cfg(test)]
mod tests;

/// One line of the virtualized table.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum MonitorEntry {
    /// A group caption introducing the rows below it.
    Header(SectionHead),
    /// One core or exchange row.
    Row {
        /// The row's values.
        row: MonitorRow,
        /// Whether this line takes the zebra stripe.
        ///
        /// Carried rather than derived from the entry index: captions and subtotals are entries
        /// too, so a stripe computed from that index would alternate at the wrong places and shift
        /// under every section.
        stripe: bool,
        /// How many times this row's core already appeared above.
        ///
        /// Zero for every row of an unsectioned table and for a core saved into one group. It
        /// exists so the drawn rows of a core saved into SEVERAL groups can take distinct element
        /// identities: gpui keys interactive state on that identity, and two live rows sharing one
        /// would trade hover and press state every frame.
        occurrence: usize,
    },
    /// The fold of one section's rows.
    Subtotal {
        /// Already-localized caption, such as `Scalpers: total`.
        label: String,
        /// The combined values.
        row: MonitorRow,
        /// Position of the section this closes, counting only drawn sections.
        section: usize,
    },
}

impl MonitorEntry {
    /// Return the cores this line stands for, or `None` for a line that stands for no set.
    ///
    /// One definition for both passes that need it — the core-filter payload and the run control —
    /// because the answer is a property of the line, not of the feature reading it. A subtotal is
    /// the one line with no set: it is a fold, and "act on this fold" would mean whatever its
    /// section happens to hold right now.
    ///
    /// Returns:
    ///     The line's cores, shared rather than copied.
    pub(super) fn scope_cores(&self) -> Option<Rc<[CoreId]>> {
        match self {
            Self::Row { row, .. } => Some(row.filter_cores.clone()),
            Self::Header(head) => Some(head.cores.clone()),
            Self::Subtotal { .. } => None,
        }
    }
}

/// A group caption and what clicking it stands for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct SectionHead {
    /// The group's name, or the localized caption of the ungrouped remainder.
    pub(super) name: String,
    /// Cores the caption filters the terminal to, shared rather than copied per frame.
    pub(super) cores: Rc<[CoreId]>,
    /// Position of this section among the drawn ones.
    ///
    /// The caption's element identity, and deliberately not the entry index: a row appearing in an
    /// earlier section would shift every index below it and migrate hover state between captions.
    pub(super) section: usize,
}

/// Localized captions the pure pass cannot produce on its own.
pub(super) struct SectionLabels<'a> {
    /// Caption of the trailing section holding cores no saved group names.
    pub(super) ungrouped: &'a str,
    /// Builds one section's subtotal caption from its name.
    pub(super) subtotal: &'a dyn Fn(&str) -> String,
}

/// Split rows into saved-group sections, or return them unchanged when there is nothing to split.
///
/// Args:
///     rows: Finished core rows, already carrying their filter payloads.
///     live: Configuration context holding the saved groups and canonical core order.
///     sort: Explicit user ordering, applied INSIDE each section so the group order stays the
///         user's own.
///     labels: Localized captions.
///
/// Returns:
///     The flat entry sequence: caption, rows, subtotal, per drawn section. Whenever sectioning
///     would draw ONE caption over everything — no saved group, none that names a visible row, or a
///     single group holding every visible row — the untouched rows in their sorted order.
pub(super) fn sectioned(
    rows: Vec<MonitorRow>,
    live: &LiveContext,
    sort: Option<MonitorSort>,
    labels: SectionLabels<'_>,
) -> Vec<MonitorEntry> {
    if live.core_groups.is_empty() {
        return flat(rows, sort);
    }

    // One index instead of one scan per group: a saved group names its members directly, so its
    // rows are a lookup each. Scanning every row per group would be `groups × rows` on a pass the
    // arrival highlight can drive at 10 Hz.
    //
    // One entry per core, which is exactly what Core mode produces — the only mode that reaches
    // this function (`ProfitMonitorView::body`). An Exchange row merges several cores under one
    // `primary_core`, and sectioning it would be meaningless anyway.
    let by_core: HashMap<CoreId, usize> = rows
        .iter()
        .enumerate()
        .map(|(index, row)| (row.primary_core, index))
        .collect();
    // Which cores any group names, and therefore which rows the remainder keeps. Resolved BEFORE
    // anything is drawn, because "would sectioning produce more than one section" decides whether
    // to section at all.
    let grouped_cores: HashSet<CoreId> = live
        .core_groups
        .iter()
        .flat_map(|group| group.cores.iter().copied())
        .collect();
    let filled = live
        .core_groups
        .iter()
        .filter(|group| group.cores.iter().any(|core| by_core.contains_key(core)))
        .count();
    // One predicate for both the count below and the rows built at the end: two copies of "what the
    // remainder holds" is how the early exit and the drawn table come to disagree.
    let ungrouped = |row: &MonitorRow| !grouped_cores.contains(&row.primary_core);
    let remainder_rows = rows.iter().filter(|row| ungrouped(row)).count();
    // One caption over the whole table plus a subtotal repeating the footer states nothing the
    // footer does not — whether that one section is a group holding every visible row, or the
    // remainder holding every row because no group names one.
    if filled + usize::from(remainder_rows > 0) < 2 {
        return flat(rows, sort);
    }
    // By name, and case-insensitively: `sanitize_core_groups` already guarantees names are unique
    // that way, so this is a total order and two launches cannot draw the sections differently.
    let mut order: Vec<&CoreGroup> = live.core_groups.iter().collect();
    order.sort_by_cached_key(|group| group.name.to_lowercase());

    // Every uid the configuration still holds — the authority both captions read.
    let configured: HashSet<CoreId> = live.core_order.iter().copied().collect();
    let mut entries = Vec::new();
    let mut drawn = Section::default();
    for group in order {
        let members: HashSet<CoreId> = group.cores.iter().copied().collect();
        // Collected by POSITION in `rows`, never in saved-member order: `rows` already carries the
        // canonical core order, and with no column selected `sort_rows` leaves what it is given.
        // Taking the group's own order would make a section disagree with the ungrouped remainder
        // beneath it — one table showing two orders.
        let mut picked: Vec<usize> = members
            .iter()
            .filter_map(|core| by_core.get(core))
            .copied()
            .collect();
        picked.sort_unstable();
        let section: Vec<MonitorRow> = picked
            .into_iter()
            .map(|index| rows[index].clone())
            .collect();
        // Configured members first, then any member that traded but left the configuration — its
        // row is on screen under this caption, and a caption filtering away a row it visibly
        // contains contradicts itself. The remainder's payload is built the same way below.
        let mut cores = cores_in_order(live, |core| members.contains(&core)).to_vec();
        cores.extend(
            section
                .iter()
                .map(|row| row.primary_core)
                .filter(|core| !configured.contains(core)),
        );
        let cores = Rc::from(cores.as_slice());
        push_section(
            &mut entries,
            &mut drawn,
            &group.name,
            section,
            cores,
            sort,
            &labels,
        );
    }

    // Everything the saved groups do not name, in one trailing section — last by construction
    // rather than by where its localized caption happens to sort.
    let remainder: Vec<MonitorRow> = rows.into_iter().filter(|row| ungrouped(row)).collect();
    // The same promise a group caption makes, read the other way round: every configured core no
    // group names, quiet ones included. A core that traded but is no longer configured is appended,
    // because the user can see its row and a caption that filtered it away would contradict it.
    let mut cores: Vec<CoreId> = live
        .core_order
        .iter()
        .copied()
        .filter(|core| !grouped_cores.contains(core))
        .collect();
    cores.extend(
        remainder
            .iter()
            .map(|row| row.primary_core)
            .filter(|core| !configured.contains(core)),
    );
    push_section(
        &mut entries,
        &mut drawn,
        labels.ungrouped,
        remainder,
        Rc::from(cores.as_slice()),
        sort,
        &labels,
    );
    entries
}

/// Running position and per-core repeat count of the sections drawn so far.
///
/// Both are exactly "what has been pushed already", so they are carried rather than recovered by
/// re-scanning the entries — a scan per section is quadratic in a table this module can fill with
/// a few hundred lines.
#[derive(Default)]
struct Section {
    /// How many captions have been drawn.
    index: usize,
    /// How many times each core has been drawn.
    occurrences: HashMap<CoreId, usize>,
}

/// Order rows and return them as one unsectioned table.
///
/// What every mode that is NOT "by core, grouped" draws: the exchange axis, the core axis with
/// grouping switched off, and the two cases above where sectioning would say nothing.
///
/// Args:
///     rows: Finished rows.
///     sort: Explicit user ordering, if any.
///
/// Returns:
///     One striped entry per row.
pub(super) fn flat(mut rows: Vec<MonitorRow>, sort: Option<MonitorSort>) -> Vec<MonitorEntry> {
    sort_rows(&mut rows, sort);
    rows.into_iter()
        .enumerate()
        .map(|(index, row)| MonitorEntry::Row {
            row,
            stripe: index % 2 == 1,
            occurrence: 0,
        })
        .collect()
}

/// Append one complete section, or nothing at all when it holds no row.
///
/// An empty section is dropped rather than drawn with zeroes: a group whose cores were all quiet
/// says nothing the grand-total footer does not, and on a configuration with a dozen saved groups
/// it would push the rows that DO carry trades off the visible area. (With zero rows for idle cores
/// switched on, a group of active cores is never empty — that preference is a request to see them.)
///
/// Args:
///     entries: The sequence being built.
///     drawn: Position and repeat counts of the sections already pushed, advanced here.
///     name: Section caption.
///     rows: The section's rows, in arrival order.
///     cores: What the caption filters to when clicked.
///     sort: Explicit ordering applied within this section.
///     labels: Localized captions.
fn push_section(
    entries: &mut Vec<MonitorEntry>,
    drawn: &mut Section,
    name: &str,
    mut rows: Vec<MonitorRow>,
    cores: Rc<[CoreId]>,
    sort: Option<MonitorSort>,
    labels: &SectionLabels<'_>,
) {
    if rows.is_empty() {
        return;
    }
    sort_rows(&mut rows, sort);
    let section = drawn.index;
    drawn.index += 1;
    // A hand-edited saved name may contain a hard break. Fold it once for both visible labels:
    // GPUI's nowrap suppresses soft wrapping only, so raw CR/LF would violate the fixed row height.
    let display_name = crate::display_text::flatten_lines(name);
    // Folded before the rows move out, and only where there is something to add up: a fold of one
    // row restates the row above it.
    let subtotal = (rows.len() > 1).then(|| fold_total(&rows));
    entries.push(MonitorEntry::Header(SectionHead {
        name: display_name.clone(),
        cores,
        section,
    }));
    for (index, row) in rows.into_iter().enumerate() {
        let occurrence = drawn.occurrences.entry(row.primary_core).or_default();
        entries.push(MonitorEntry::Row {
            row,
            stripe: index % 2 == 1,
            occurrence: *occurrence,
        });
        *occurrence += 1;
    }
    if let Some(row) = subtotal {
        entries.push(MonitorEntry::Subtotal {
            label: (labels.subtotal)(&display_name),
            row,
            section,
        });
    }
}

/// Select the CONFIGURED cores a caption stands for, in canonical order.
///
/// A caption filters to every core it names, not only the ones that traded inside the period — the
/// same reason an exchange row filters to its quiet cores: the label names a set, and silently
/// dropping the quiet ones would hide their open orders in every other panel. A saved member naming
/// a core that no longer exists simply is not selected; the group keeps it on disk (see
/// `moon_core::config::core_groups`), but there is nothing to filter to.
///
/// Args:
///     live: Configuration context supplying the canonical order.
///     belongs: Whether one configured core belongs to this caption.
///
/// Returns:
///     The caption's click payload, possibly empty when it names nothing configured.
fn cores_in_order(live: &LiveContext, belongs: impl Fn(CoreId) -> bool) -> Rc<[CoreId]> {
    let cores: Vec<CoreId> = live
        .core_order
        .iter()
        .copied()
        .filter(|core| belongs(*core))
        .collect();
    Rc::from(cores.as_slice())
}
