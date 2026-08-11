//! Regression tests for the Profit Monitor's saved-group sections and their subtotals.

use std::collections::HashSet;

use moon_core::config::CoreGroup;

use super::*;
use crate::analytics::profit_monitor::MonitorSortColumn;
use crate::analytics::profit_monitor::next_sort;

/// Build the localized captions the pure pass takes as data.
///
/// Returns:
///     English captions, so an expectation reads as the table does.
fn labels() -> SectionLabels<'static> {
    SectionLabels {
        ungrouped: "Ungrouped",
        subtotal: &|name| format!("Total: {name}"),
    }
}

/// Build one core row with the two values every assertion here cares about.
///
/// Args:
///     core: The row's core.
///     profit: Its projected profit.
///     trades: Its closed-trade count.
///
/// Returns:
///     A row shaped like the one `grouped_rows` produces in Core mode.
fn row(core: CoreId, profit: f64, trades: i64) -> MonitorRow {
    MonitorRow {
        name: format!("Core {core}"),
        profit,
        trades,
        wins: trades,
        cores: vec![core],
        primary_core: core,
        last_close: 1_700_000_000 + core as i64,
        last_core: core,
        last_profit: Some(profit),
        filter_cores: Rc::from([core].as_slice()),
        ..MonitorRow::default()
    }
}

/// Build a context whose groups and canonical order are stated explicitly.
///
/// Args:
///     order: Configured cores in canonical order.
///     groups: Saved groups as `(name, members)`.
///
/// Returns:
///     The context `sectioned` reads.
fn context(order: &[CoreId], groups: &[(&str, &[CoreId])]) -> LiveContext {
    LiveContext {
        core_order: order.to_vec(),
        core_groups: groups
            .iter()
            .map(|(name, cores)| CoreGroup {
                name: (*name).to_string(),
                cores: cores.to_vec(),
            })
            .collect(),
        ..LiveContext::default()
    }
}

/// Return every caption in the produced sequence, in order.
///
/// Args:
///     entries: The produced sequence.
///
/// Returns:
///     Section captions.
fn headers(entries: &[MonitorEntry]) -> Vec<String> {
    entries
        .iter()
        .filter_map(|entry| match entry {
            MonitorEntry::Header(head) => Some(head.name.clone()),
            _ => None,
        })
        .collect()
}

/// Return every subtotal as `(caption, profit, trades)`, in order.
///
/// Args:
///     entries: The produced sequence.
///
/// Returns:
///     Subtotal captions with their folded values.
fn subtotals(entries: &[MonitorEntry]) -> Vec<(String, f64, i64)> {
    entries
        .iter()
        .filter_map(|entry| match entry {
            MonitorEntry::Subtotal { label, row, .. } => {
                Some((label.clone(), row.profit, row.trades))
            }
            _ => None,
        })
        .collect()
}

/// `sections.rs:sectioned` must leave a table with no saved group exactly as it found it.
///
/// Breakage: wrapping everything in one "Ungrouped" section adds a caption and a subtotal that
/// duplicate the window's own Total for every user who never saved a group.
#[test]
fn without_saved_groups_the_table_keeps_its_plain_shape() {
    let live = context(&[1, 2], &[]);
    let entries = sectioned(
        vec![row(1, 10.0, 2), row(2, -3.0, 1)],
        &live,
        None,
        labels(),
    );

    assert!(headers(&entries).is_empty(), "no caption without a group");
    assert!(subtotals(&entries).is_empty(), "no subtotal either");
    assert_eq!(entries.len(), 2);
    assert!(matches!(
        entries[0],
        MonitorEntry::Row { stripe: false, .. }
    ));
    assert!(matches!(entries[1], MonitorEntry::Row { stripe: true, .. }));
}

/// `sections.rs:sectioned` must also leave the table plain when saved groups exist but name
/// nothing that is on screen.
///
/// Breakage: emitting the trailing remainder unconditionally wraps the whole table in one
/// "Ungrouped" caption plus a subtotal that repeats the footer Total exactly — a shape that states
/// nothing and costs two lines, hit whenever every saved group is quiet for the period.
#[test]
fn saved_groups_that_name_nothing_visible_leave_the_table_plain() {
    let live = context(&[1, 2], &[("Elsewhere", &[41, 42])]);
    let entries = sectioned(
        vec![row(1, 10.0, 2), row(2, -3.0, 1)],
        &live,
        None,
        labels(),
    );

    assert!(headers(&entries).is_empty());
    assert!(subtotals(&entries).is_empty());
    assert_eq!(entries.len(), 2);
}

/// `sections.rs:sectioned` must keep a section's rows in the order the table already had them,
/// never in the order the group's members happen to be saved in.
///
/// Breakage: collecting by member order makes a section disagree with the ungrouped remainder
/// below it — and with the plain table — the moment no column is selected, which is the default:
/// `sort_rows` is a no-op there and cannot repair the order.
#[test]
fn a_section_keeps_the_canonical_row_order() {
    // The group names its members backwards; the rows arrive canonically ordered.
    let live = context(&[1, 2, 3, 4], &[("Backwards", &[3, 1]), ("Other", &[2, 4])]);
    let entries = sectioned(
        vec![
            row(1, 1.0, 1),
            row(2, 2.0, 1),
            row(3, 3.0, 1),
            row(4, 4.0, 1),
        ],
        &live,
        None,
        labels(),
    );

    let drawn: Vec<CoreId> = entries
        .iter()
        .filter_map(|entry| match entry {
            MonitorEntry::Row { row, .. } => Some(row.primary_core),
            _ => None,
        })
        .collect();
    assert_eq!(
        drawn,
        [1, 3, 2, 4],
        "Backwards draws 1 before 3, exactly as the table above it would"
    );
}

/// The same order rule must hold where the fix's own machinery is exercised: a core saved into two
/// groups, and a group whose member list repeats a core.
///
/// Breakage: de-duplicating members through a set and then reading the set's iteration order puts a
/// section's rows in hash order, which is neither the table's nor the group's — and is unstable
/// between two runs on identical data.
#[test]
fn shared_and_duplicated_members_keep_that_order_too() {
    let live = context(
        &[1, 2, 3, 4],
        // `Shared` names 3 before 1 and repeats 1; `Second` shares core 1 with it.
        &[("Shared", &[3, 1, 1]), ("Second", &[1, 2])],
    );
    let entries = sectioned(
        vec![
            row(1, 1.0, 1),
            row(2, 2.0, 1),
            row(3, 3.0, 1),
            row(4, 4.0, 1),
        ],
        &live,
        None,
        labels(),
    );

    let drawn: Vec<CoreId> = entries
        .iter()
        .filter_map(|entry| match entry {
            MonitorEntry::Row { row, .. } => Some(row.primary_core),
            _ => None,
        })
        .collect();
    assert_eq!(
        drawn,
        [1, 2, 1, 3, 4],
        "Second, then Shared (drawing 1 once, before 3), then the remainder"
    );
}

/// `sections.rs:sectioned` must leave the table plain when sectioning would draw ONE caption over
/// everything — including a single group that happens to name every visible row.
///
/// Breakage: guarding only the "no section at all" case still wraps the whole table in one caption
/// plus a subtotal that repeats the footer exactly, which is the shape the module promises never to
/// draw.
#[test]
fn one_section_covering_everything_is_not_a_section() {
    let live = context(&[1, 2], &[("Everything", &[1, 2])]);
    let entries = sectioned(vec![row(1, 10.0, 2), row(2, 4.0, 1)], &live, None, labels());

    assert!(headers(&entries).is_empty());
    assert!(subtotals(&entries).is_empty());
    assert_eq!(entries.len(), 2);
}

/// `sections.rs:push_section` must not restate a single row as a subtotal.
///
/// Breakage: a subtotal under a one-row section duplicates the row above it, so a dozen
/// single-core groups double the table's height for no information at all.
#[test]
fn a_one_row_section_gets_no_subtotal() {
    let live = context(&[1, 2, 3], &[("Solo", &[1]), ("Pair", &[2, 3])]);
    let entries = sectioned(
        vec![row(1, 10.0, 2), row(2, 1.0, 1), row(3, 2.0, 1)],
        &live,
        None,
        labels(),
    );

    assert_eq!(headers(&entries), ["Pair", "Solo"]);
    assert_eq!(
        subtotals(&entries),
        [("Total: Pair".to_string(), 3.0, 2)],
        "only the section with something to add up carries a fold"
    );
}

/// `sections.rs:push_section` must give each drawn occurrence of one core its own element identity.
///
/// Breakage: two rows of a core saved into two groups share one gpui element state, so hover and
/// press leak between sections and a click can land on the wrong row.
#[test]
fn every_drawn_occurrence_of_a_core_is_numbered() {
    let live = context(&[1, 2], &[("Both", &[1, 2]), ("Alone", &[1])]);
    let entries = sectioned(vec![row(1, 10.0, 2), row(2, 4.0, 1)], &live, None, labels());

    let occurrences: Vec<(CoreId, usize)> = entries
        .iter()
        .filter_map(|entry| match entry {
            MonitorEntry::Row {
                row, occurrence, ..
            } => Some((row.primary_core, *occurrence)),
            _ => None,
        })
        .collect();
    assert_eq!(
        occurrences,
        [(1, 0), (1, 1), (2, 0)],
        "Alone draws core 1 first, Both draws it a second time"
    );
}

/// `sections.rs:remainder_cores` must make the ungrouped caption promise what a group caption does:
/// every configured core no group names, quiet ones included.
///
/// Breakage: filtering to the cores that produced a row makes the two captions mean different
/// things, and hides a quiet core's open orders in every panel the click narrows.
#[test]
fn the_remainder_caption_filters_to_configured_cores_too() {
    let live = context(&[1, 2, 3], &[("Named", &[1])]);
    // Core 3 is configured but quiet; core 77 traded and is no longer configured.
    let entries = sectioned(
        vec![row(1, 1.0, 1), row(2, 2.0, 1), row(77, 5.0, 1)],
        &live,
        None,
        labels(),
    );

    let MonitorEntry::Header(head) = &entries[2] else {
        panic!("the remainder must open with its caption");
    };
    assert_eq!(head.name, "Ungrouped");
    assert_eq!(
        head.cores.as_ref(),
        [2, 3, 77],
        "configured cores outside every group, then the row that outlived its configuration"
    );
}

/// `sections.rs:sectioned` must order sections by NAME and keep the ungrouped remainder last.
///
/// Breakage: following the saved list order makes the sections move whenever the management modal
/// reorders groups; sorting the ungrouped caption with the rest buries it in the middle in one
/// language and puts it last in another.
#[test]
fn sections_follow_their_names_and_the_remainder_stays_last() {
    let live = context(
        &[1, 2, 3, 4, 5, 6],
        &[("Zulu", &[1, 2]), ("alpha", &[3, 4])],
    );
    let entries = sectioned(
        vec![
            row(1, 10.0, 2),
            row(2, 5.0, 1),
            row(3, 1.0, 1),
            row(4, 2.0, 1),
            row(5, 3.0, 1),
            row(6, 4.0, 1),
        ],
        &live,
        None,
        labels(),
    );

    assert_eq!(headers(&entries), ["alpha", "Zulu", "Ungrouped"]);
    assert_eq!(
        subtotals(&entries)
            .into_iter()
            .map(|(label, _, _)| label)
            .collect::<Vec<_>>(),
        ["Total: alpha", "Total: Zulu", "Total: Ungrouped"]
    );
}

/// `sections.rs:sectioned` must show a core saved into two groups in BOTH, counting it in each
/// subtotal.
///
/// Breakage: assigning each core to its first matching group hides it from the second group whose
/// name promises it, which is the opposite of what saving it there meant.
#[test]
fn a_core_in_two_groups_appears_and_counts_in_both() {
    let live = context(&[1, 2, 3], &[("Both", &[1, 2]), ("Alone", &[1, 3])]);
    let entries = sectioned(
        vec![row(1, 10.0, 2), row(2, 4.0, 1), row(3, 1.0, 1)],
        &live,
        None,
        labels(),
    );

    assert_eq!(headers(&entries), ["Alone", "Both"]);
    assert_eq!(
        subtotals(&entries),
        [
            ("Total: Alone".to_string(), 11.0, 3),
            ("Total: Both".to_string(), 14.0, 3),
        ],
        "the shared core counts inside both folds"
    );
}

/// `sections.rs:sectioned` must sort INSIDE a section and never reorder the sections themselves.
///
/// Breakage: sorting the whole table first and slicing it afterwards mixes rows across captions;
/// letting the sort touch section order makes a click on Profit rewrite the user's own grouping.
#[test]
fn an_explicit_sort_orders_rows_within_each_section() {
    let live = context(&[1, 2, 3, 4], &[("Second", &[3, 4]), ("First", &[1, 2])]);
    let entries = sectioned(
        vec![
            row(1, 1.0, 1),
            row(2, 9.0, 1),
            row(3, 2.0, 1),
            row(4, 8.0, 1),
        ],
        &live,
        Some(next_sort(None, MonitorSortColumn::Profit)),
        labels(),
    );

    let shape: Vec<String> = entries
        .iter()
        .map(|entry| match entry {
            MonitorEntry::Header(head) => format!("# {}", head.name),
            MonitorEntry::Row { row, .. } => format!("{}", row.primary_core),
            MonitorEntry::Subtotal { label, .. } => format!("= {label}"),
        })
        .collect();
    assert_eq!(
        shape,
        [
            "# First",
            "2",
            "1",
            "= Total: First",
            "# Second",
            "4",
            "3",
            "= Total: Second",
        ],
        "each section sorts descending on its own, and the captions keep their name order"
    );
}

/// `sections.rs:sectioned` must drop a section whose cores traded nothing, and `push_section` must
/// restart the zebra inside every section it keeps.
///
/// Breakage: emitting an empty group as a zero caption plus a zero subtotal pushes the rows that do
/// carry trades off screen on a configuration with a dozen saved groups; striping from the entry
/// index instead shifts the zebra by one at every caption.
#[test]
fn an_empty_group_is_dropped_and_each_section_restarts_the_stripe() {
    // Core 4 stays outside every group, so the remainder gives the table its second section — the
    // one thing that keeps this from collapsing to a plain list.
    let live = context(&[1, 2, 3, 4], &[("Quiet", &[9]), ("Busy", &[1, 2, 3])]);
    let entries = sectioned(
        vec![
            row(1, 1.0, 1),
            row(2, 2.0, 1),
            row(3, 3.0, 1),
            row(4, 4.0, 1),
        ],
        &live,
        None,
        labels(),
    );

    assert_eq!(
        headers(&entries),
        ["Busy", "Ungrouped"],
        "a silent group draws nothing"
    );
    let stripes: Vec<bool> = entries
        .iter()
        .filter_map(|entry| match entry {
            MonitorEntry::Row { stripe, .. } => Some(*stripe),
            _ => None,
        })
        .collect();
    assert_eq!(
        stripes,
        [false, true, false, false],
        "the remainder's single row starts its own zebra"
    );
}

/// `sections.rs:group_head_cores` must resolve a caption to every CONFIGURED member, in canonical
/// order, whether or not that member traded.
///
/// Breakage: filtering to the cores that traded hides a quiet core's open orders in every panel the
/// click narrows; keeping a member whose core was deleted publishes a filter nothing can match.
#[test]
fn a_caption_filters_to_the_configured_members_of_its_group() {
    // Core 7 sits outside the group so the table has a second section and stays sectioned at all.
    let live = context(&[2, 1, 7], &[("Pair", &[1, 2, 404])]);
    let entries = sectioned(vec![row(1, 1.0, 1), row(7, 2.0, 1)], &live, None, labels());

    let MonitorEntry::Header(head) = &entries[0] else {
        panic!("the section must open with its caption");
    };
    assert_eq!(
        head.cores.as_ref(),
        [2, 1],
        "canonical order, the quiet member included and the deleted one left out"
    );
    assert_eq!(head.name, "Pair");
}

/// `sections.rs:sectioned` must keep the ungrouped remainder to cores no group names at all.
///
/// Breakage: computing the remainder against the sections that were DRAWN puts a member of a
/// dropped empty group into Ungrouped, so a core appears under a caption its user never chose.
#[test]
fn the_remainder_holds_only_cores_no_group_names() {
    let live = context(&[1, 2, 3], &[("Named", &[1, 3])]);
    let entries = sectioned(
        vec![row(1, 1.0, 1), row(2, 2.0, 1), row(3, 3.0, 1)],
        &live,
        None,
        labels(),
    );

    assert_eq!(headers(&entries), ["Named", "Ungrouped"]);
    let remainder: HashSet<CoreId> = entries
        .iter()
        .skip_while(
            |entry| !matches!(entry, MonitorEntry::Header(head) if head.name == "Ungrouped"),
        )
        .filter_map(|entry| match entry {
            MonitorEntry::Row { row, .. } => Some(row.primary_core),
            _ => None,
        })
        .collect();
    assert_eq!(remainder, HashSet::from([2]));
}
