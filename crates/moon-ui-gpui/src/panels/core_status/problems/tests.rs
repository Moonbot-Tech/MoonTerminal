//! Explicit imports (no `use super::*`) per the crate's test convention: the panel's parent module
//! re-exports `gpui::*`, whose own `test` would shadow the built-in attribute.

use std::collections::HashMap;

use moon_core::config::TableSortPreference;
use moon_core::feed::{CoreProblem, CoreProblemCategory};
use moon_core::util::display_time::format_minute;
use rust_i18n::t;

use super::{
    ActionGate, ORDER_KEYS, ProblemRef, ProblemRow, ProblemsScope, category_label, core_label,
    details_text, drawn_kinds, empty_text, fleet_refusal, mark_signature, notice_text,
    order_and_cap, restore_problems_sort, shown_sort,
};

/// A scope with nothing truncated, which is every case but the cap's own test.
fn scope(cores: usize, silent: usize) -> ProblemsScope {
    ProblemsScope {
        cores,
        silent: (0..silent).map(|i| format!("core-{i}")).collect(),
        truncated: false,
        actions: ActionGate::NoSingleChoice,
        picked: false,
        targets: 0,
    }
}

/// The empty state must never say "no problems" for a core that never answered.
///
/// Regression target for the one failure mode this whole surface exists to avoid, and the one the
/// protocol documentation calls out in as many words: an absent list is not proof that the core is
/// healthy. A core older than the extension sends nothing at all, so its silence is
/// indistinguishable from a clean bill unless the count of silent cores decides the wording.
#[test]
fn an_empty_list_reads_as_clean_only_when_every_core_answered() {
    let _locale = crate::test_locale::force("en");
    // Asserted against the KEYS, not against each other: two strings merely differing would still
    // pass with the branch inverted, which is the exact failure this guards.
    let clean = t!("core_status.problems_empty").to_string();
    let neutral = t!("core_status.problems_nothing_listed").to_string();
    assert_ne!(clean, neutral, "the fixture itself must distinguish them");

    assert_eq!(
        empty_text(&scope(4, 0)),
        clean,
        "every core answered, none reported"
    );
    assert_eq!(
        empty_text(&scope(4, 1)),
        neutral,
        "one silent core forbids the clean bill"
    );
    assert_eq!(
        empty_text(&scope(0, 0)),
        neutral,
        "an empty scope says nothing about anyone's health"
    );
}

/// A category this build has never seen keeps its raw byte instead of folding into `Other`.
///
/// `Other` is a real category the core assigns deliberately. Collapsing an unrecognised one into it
/// would erase the only signal that says the TERMINAL is behind, not the core.
#[test]
fn an_unknown_category_is_shown_as_its_raw_byte() {
    let _locale = crate::test_locale::force("en");
    assert_eq!(category_label(CoreProblemCategory::Unknown(7)), "#7");
    assert_ne!(
        category_label(CoreProblemCategory::Unknown(7)),
        category_label(CoreProblemCategory::Other)
    );
}

/// The unknown-cores caveat must survive a table that is NOT empty.
///
/// Regression target for the first shape of this surface, where the count reached the screen only
/// through the empty-state string: one finding from one core hid the fact that every other core in
/// the fleet had never answered — the exact conflation the feature exists to prevent.
#[test]
fn the_silent_core_notice_does_not_depend_on_an_empty_table() {
    let _locale = crate::test_locale::force("en");
    let silent = notice_text(&scope(200, 199)).expect("199 silent cores must be stated");
    assert!(
        silent.contains("199"),
        "the caveat names how many cores are silent: {silent}"
    );
    // The empty-scope arm must be its OWN message: "Not reported: 0" would be nonsense.
    assert_eq!(
        notice_text(&scope(0, 0)),
        Some(t!("core_status.problems_no_cores").to_string())
    );
    assert_eq!(
        notice_text(&scope(3, 0)),
        None,
        "nothing to caveat once every core has answered"
    );
}

/// A cut list must say so; an uncut one must not.
///
/// Silently dropping findings past the cap makes a truncated view read as the complete picture —
/// the same conflation as a silent core reading as a healthy one, which is what the notice exists
/// to prevent.
#[test]
fn a_truncated_list_is_stated_even_when_every_core_answered() {
    let _locale = crate::test_locale::force("en");
    let full = ProblemsScope {
        cores: 4,
        silent: Vec::new(),
        truncated: true,
        actions: ActionGate::NoSingleChoice,
        picked: false,
        targets: 0,
    };
    let text = notice_text(&full).expect("a cut list is stated");
    assert!(
        text.contains(&super::PROBLEM_LIST_LIMIT.to_string()),
        "the caveat names the cap: {text}"
    );
    assert_eq!(
        notice_text(&scope(4, 0)),
        None,
        "an uncut, fully answered scope has nothing to caveat"
    );
}

/// The hover carries the body and the evidence, and never opens empty.
///
/// A core that sent neither must not produce a blank popup on a cell that is hoverable regardless,
/// and a detector message long enough to overflow the window must say it was cut rather than end
/// mid-sentence as if that were all the core said.
#[test]
fn the_hover_carries_what_the_cell_could_not_and_stays_bounded() {
    let problem = |message: &str, details: &str| CoreProblem {
        kind: 1,
        kind_name: "vds-maintenance".to_string(),
        category: CoreProblemCategory::Machine,
        title: "heading".to_string(),
        message: message.to_string(),
        technical_details: details.to_string(),
        first_seen_ms: None,
        confirmed_ms: None,
        confirmations: 0,
    };

    assert_eq!(details_text(&problem("", "")), None, "no blank popup");
    assert_eq!(
        details_text(&problem("body", "")),
        Some("body".to_string()),
        "evidence is optional, the body is not padded around it"
    );
    let both = details_text(&problem("body", "evidence")).expect("both halves");
    assert!(both.starts_with("body") && both.ends_with("evidence"));
    assert!(
        !both.contains("heading"),
        "the heading is in the cell already: {both}"
    );

    // A tooltip has a fixed width and no scrollbar, so the projection's own 2000-char clamp is not
    // the bound that matters here.
    let long = details_text(&problem(&"x".repeat(5_000), "")).expect("long body");
    assert!(long.chars().count() <= super::DETAILS_MAX_CHARS + 1);
    assert!(long.ends_with('…'), "a cut message says it was cut: {long}");
}

/// The silent-core hover names them, and stays bounded on a large fleet.
#[test]
fn the_silent_core_hover_names_them_without_outgrowing_the_window() {
    let names: Vec<String> = (0..200).map(|i| format!("core-{i}")).collect();
    let joined = super::ellipsize(&names.join(", "), super::DETAILS_MAX_CHARS);
    assert!(joined.starts_with("core-0, core-1"), "names, in order");
    assert!(
        joined.chars().count() <= super::DETAILS_MAX_CHARS + 1,
        "a 200-core fleet must not open a popup taller than the window"
    );
}

/// The channel test addresses ONE core, and says which reason keeps it shut.
///
/// The gate guards the test alone. It used to guard the reset as well, on the argument that an
/// irreversible action must not ride on a coincidence of scope — and that argument still holds; it
/// simply moved. The reset no longer reads this gate at all: it carries its own list of targets and
/// names them in its confirm, so nothing about it is decided by which core happened to be left.
#[test]
fn the_channel_test_opens_only_for_one_connected_core() {
    let _locale = crate::test_locale::force("en");
    assert_eq!(ActionGate::Ready(7).core(), Some(7));
    assert_eq!(ActionGate::NoSingleChoice.core(), None);
    assert_eq!(
        ActionGate::NotConnected.core(),
        None,
        "a queued test would fire on reconnect and publish its fact unannounced"
    );

    // Each refusal explains itself: one greyed button with one generic tooltip cannot say which of
    // the two remedies applies.
    assert_eq!(ActionGate::Ready(7).refusal(), None);
    let no_choice = ActionGate::NoSingleChoice
        .refusal()
        .expect("a closed gate states its reason");
    let offline = ActionGate::NotConnected
        .refusal()
        .expect("a closed gate states its reason");
    assert_ne!(
        no_choice, offline,
        "picking a core and waiting for one are different problems"
    );
}

/// The fleet actions refuse for their own reasons, and never quietly.
///
/// Regression target for the shape this replaced: one gate refused BOTH actions whenever the
/// operator had not hand-picked a core, which in a workspace-owned panel is permanent — the
/// selector is pinned there and the pick can never be made. A live button with no targets and a
/// greyed button with no reason are the same defect from opposite sides.
#[test]
fn the_fleet_actions_state_their_own_refusals() {
    let _locale = crate::test_locale::force("en");
    let no_cores = fleet_refusal(0, false, 0).expect("an empty scope refuses");
    assert_eq!(
        no_cores,
        t!("core_status.problems_no_cores").to_string(),
        "an empty scope is explained the way the notice above the table already explains it"
    );

    let none_up = fleet_refusal(4, false, 0).expect("a scope with nothing connected refuses");
    assert_ne!(
        none_up, no_cores,
        "covering no cores and covering four that are down are different facts"
    );

    // The regression this argument exists for: a scope of live cores must never be reported as
    // offline because the ONE core the operator clicked happens to be down. Same numbers, and the
    // two states must still not read alike.
    let picked_down = fleet_refusal(4, true, 0).expect("a picked core that is down refuses");
    assert_ne!(
        picked_down, none_up,
        "the core you clicked being down is not the scope having nothing up"
    );

    assert_eq!(
        fleet_refusal(4, false, 1),
        None,
        "one connected core in scope opens both actions"
    );
    assert_eq!(
        fleet_refusal(1, true, 1),
        None,
        "a core that has never delivered a list still holds pending hypotheses to drop"
    );
}

/// What counts as looked at is taken from the ROWS on screen, per core.
///
/// Identity, not a timestamp: the stamps come off the wire unbounded and from each core's own
/// clock, so a single far-future value would have raised a monotonic watermark past every real
/// finding and silenced that core permanently, with nothing in the app able to reset it.
///
/// Driven by the drawn rows rather than the store because the merged list is capped: marking a
/// finding read that the cap dropped is the same lie as calling a silent core healthy.
#[test]
fn only_the_kinds_actually_drawn_count_as_looked_at() {
    let row = |core: u64, kind: u8| ProblemRow {
        core,
        problem: CoreProblem {
            kind,
            kind_name: "k".to_string(),
            category: CoreProblemCategory::Machine,
            title: "t".to_string(),
            message: "m".to_string(),
            technical_details: String::new(),
            first_seen_ms: None,
            confirmed_ms: None,
            confirmations: 0,
        },
    };
    let rows = vec![row(1, 10), row(1, 11), row(2, 10)];

    assert_eq!(
        drawn_kinds(&rows, 1),
        vec![10, 11],
        "this core's kinds only"
    );
    assert_eq!(drawn_kinds(&rows, 2), vec![10]);
    assert_eq!(
        drawn_kinds(&rows, 3),
        Vec::<u8>::new(),
        "a core with nothing drawn contributes nothing to mark"
    );

    // An undated finding is no longer a special case at all — there is no clock in the rule.
    assert_eq!(drawn_kinds(&rows, 1).len(), 2);
}

/// The read-mark's early-out must move whenever the drawn set moves.
///
/// Regression target: the early-out started as "nothing unseen", which skipped the PRUNE as well as
/// the count — so a finding that went away and came back stayed silent forever, the one property
/// the identity set exists to provide. A signature over the drawn pairs cannot make that mistake:
/// a row leaving changes it just as much as a row arriving.
#[test]
fn the_mark_signature_moves_whenever_the_drawn_set_does() {
    let row = |core: u64, kind: u8| ProblemRow {
        core,
        problem: CoreProblem {
            kind,
            kind_name: "k".to_string(),
            category: CoreProblemCategory::Machine,
            title: "t".to_string(),
            message: "m".to_string(),
            technical_details: String::new(),
            first_seen_ms: None,
            confirmed_ms: None,
            confirmations: 0,
        },
    };
    let base = vec![row(1, 10), row(2, 11)];
    let same = vec![row(1, 10), row(2, 11)];
    assert_eq!(
        mark_signature(&base),
        mark_signature(&same),
        "an unchanged screen must not re-mark every frame"
    );

    // A row LEAVING is the case the old early-out missed entirely.
    assert_ne!(mark_signature(&base), mark_signature(&[row(1, 10)]));
    // A row arriving.
    assert_ne!(
        mark_signature(&base),
        mark_signature(&[row(1, 10), row(2, 11), row(2, 12)])
    );
    // The same kind on a different core is a different fact.
    assert_ne!(
        mark_signature(&base),
        mark_signature(&[row(1, 10), row(3, 11)])
    );
    assert_ne!(mark_signature(&base), mark_signature(&[]));
}

/// One synthetic finding. `kind` is the identity the assertions read back.
fn problem(
    kind: u8,
    kind_name: &str,
    category: CoreProblemCategory,
    title: &str,
    confirmed_ms: Option<i64>,
    first_seen_ms: Option<i64>,
) -> CoreProblem {
    CoreProblem {
        kind,
        kind_name: kind_name.to_string(),
        category,
        title: title.to_string(),
        message: String::new(),
        technical_details: String::new(),
        first_seen_ms,
        confirmed_ms,
        confirmations: 0,
    }
}

/// Run the production sort-then-cut and report `(core, kind)` in display order.
fn sorted_pairs(
    owned: &[(u64, CoreProblem)],
    key: &str,
    ascending: bool,
    names: &HashMap<u64, String>,
) -> (Vec<(u64, u8)>, bool) {
    let mut refs: Vec<ProblemRef<'_>> = owned
        .iter()
        .map(|(core, problem)| ProblemRef {
            core: *core,
            problem,
        })
        .collect();
    let truncated = order_and_cap(&mut refs, key, ascending, names, chrono_tz::UTC);
    (
        refs.iter()
            .map(|row| (row.core, row.problem.kind))
            .collect(),
        truncated,
    )
}

/// Opening Problems with nothing saved leads with the newest confirmation.
///
/// Mutation: default the flag to `true`, or default the column to the core name. The tab would
/// open oldest-first, or grouped by core, which is the layout the request asks to leave behind.
#[test]
fn an_unsaved_problems_sort_is_newest_confirmation_first() {
    assert_eq!(shown_sort(None), ("time".to_string(), false));
    let saved = restore_problems_sort(Some(TableSortPreference {
        column: "time".to_string(),
        ascending: false,
    }));
    assert_eq!(shown_sort(saved.as_ref()), ("time".to_string(), false));
}

/// A saved column survives only when the table still draws it.
///
/// Mutation: drop a key from the allow-list, or accept a key the table does not have. A restart
/// would forget a real choice, or a hand-edited severity entry would invent a ranking.
#[test]
fn a_saved_problems_sort_round_trips_only_for_a_real_column() {
    for key in ORDER_KEYS {
        let restored = restore_problems_sort(Some(TableSortPreference {
            column: (*key).to_string(),
            ascending: false,
        }));
        assert_eq!(
            restored
                .as_ref()
                .map(|(column, ascending)| (column.as_str(), *ascending)),
            Some((*key, false)),
            "{key} must restore with its direction"
        );
    }
    assert_eq!(
        restore_problems_sort(Some(TableSortPreference {
            column: "severity".to_string(),
            ascending: true,
        })),
        None,
        "severity is not a column the core sends"
    );
    let saved = restore_problems_sort(Some(TableSortPreference {
        column: "kind".to_string(),
        ascending: true,
    }));
    assert_eq!(shown_sort(saved.as_ref()), ("kind".to_string(), true));
}

/// A missing confirmation sorts by first-seen, including when confirmation is unprintable.
///
/// Mutation: ignore `first_seen_ms`, or treat a non-positive confirmation as an instant of zero.
/// The row the cell times from first-seen would sink under every confirmed row, or sort as if it
/// were the oldest possible fact.
#[test]
fn a_missing_confirmation_sorts_by_first_seen() {
    let older = problem(
        1,
        "older",
        CoreProblemCategory::Machine,
        "older",
        Some(1_700_000_040_100),
        None,
    );
    let via_first = problem(
        2,
        "first",
        CoreProblemCategory::Machine,
        "first",
        None,
        Some(1_700_000_080_900),
    );
    let fallback = problem(
        3,
        "fallback",
        CoreProblemCategory::Machine,
        "fallback",
        Some(0),
        Some(1_700_000_090_900),
    );
    let owned = [(1, older), (1, via_first), (1, fallback)];
    let (pairs, truncated) = sorted_pairs(&owned, "time", false, &HashMap::new());
    assert!(!truncated);
    assert_eq!(
        pairs.iter().map(|pair| pair.1).collect::<Vec<_>>(),
        vec![3, 2, 1],
        "newest instant first, whether it came from confirmation or first-seen"
    );
}

/// A dash sorts last when the newest confirmation leads, and first when that column is reversed.
///
/// Mutation: always pin dashes to the tail, or sort them as zero. Reversing Time would still hide
/// the dashes, or a non-positive stamp would lead the newest-first view.
#[test]
fn a_dash_sorts_last_when_newest_confirmation_leads() {
    let dash = problem(1, "dash", CoreProblemCategory::Machine, "dash", None, None);
    let zero = problem(
        2,
        "zero",
        CoreProblemCategory::Machine,
        "zero",
        Some(0),
        None,
    );
    let real = problem(
        3,
        "real",
        CoreProblemCategory::Machine,
        "real",
        Some(1_700_000_040_000),
        None,
    );
    // Dash first in the input, so a sort that ignores it keeps the dash on top.
    let owned = [(1, dash), (1, zero), (1, real)];
    let (newest, _) = sorted_pairs(&owned, "time", false, &HashMap::new());
    assert_eq!(
        newest.iter().map(|pair| pair.1).collect::<Vec<_>>(),
        vec![3, 1, 2],
        "the real instant leads; the two dashes keep their incoming order behind it"
    );
    let (oldest, _) = sorted_pairs(&owned, "time", true, &HashMap::new());
    assert_eq!(
        oldest.iter().map(|pair| pair.1).collect::<Vec<_>>(),
        vec![1, 2, 3],
        "reversing the column reverses the dash, it does not pin the dash"
    );
}

/// Two rows that print as the same minute stay ordered by the underlying millisecond.
///
/// Mutation: compare `format_minute`'s text, or the timestamp truncated to the minute. The later
/// event inside that minute would stay under the earlier one whenever it was listed second.
#[test]
fn rows_in_the_same_printed_minute_keep_timestamp_order() {
    let zone = chrono_tz::UTC;
    let early_ms = 1_700_000_040_100_i64;
    let late_ms = 1_700_000_080_900_i64;
    let early_text = format_minute(early_ms / 1000, zone);
    let late_text = format_minute(late_ms / 1000, zone);
    assert_eq!(
        early_text, late_text,
        "the fixture must share one printed minute"
    );
    assert!(!early_text.is_empty(), "the fixture must be a real minute");

    let owned = [
        (
            1,
            problem(
                1,
                "early",
                CoreProblemCategory::Machine,
                "early",
                Some(early_ms),
                None,
            ),
        ),
        (
            1,
            problem(
                2,
                "late",
                CoreProblemCategory::Machine,
                "late",
                Some(late_ms),
                None,
            ),
        ),
    ];
    let (pairs, truncated) = sorted_pairs(&owned, "time", false, &HashMap::new());
    assert!(!truncated);
    assert_eq!(
        pairs.iter().map(|pair| pair.1).collect::<Vec<_>>(),
        vec![2, 1],
        "the later instant leads even though both cells print the same minute"
    );
}

/// The 500-row cut keeps the rows the sort put first, not the head of the scope walk.
///
/// Mutation: truncate before sorting. The newest finding, appended by a core that sorts late,
/// would fall off the bottom, and the unseen badge would count the old row that remained.
#[test]
fn the_row_cap_is_applied_after_the_sort() {
    let limit = super::PROBLEM_LIST_LIMIT;
    let owned: Vec<(u64, CoreProblem)> = (0..limit + 1)
        .map(|index| {
            let kind = if index == 0 {
                1
            } else if index == limit {
                9
            } else {
                2
            };
            (
                1,
                problem(
                    kind,
                    "paging",
                    CoreProblemCategory::Machine,
                    "t",
                    Some((index as i64 + 1) * 60_000),
                    None,
                ),
            )
        })
        .collect();
    let (pairs, truncated) = sorted_pairs(&owned, "time", false, &HashMap::new());
    assert!(truncated, "one row past the cap must be stated as a cut");
    assert_eq!(pairs.len(), limit);
    assert_eq!(
        pairs[0].1, 9,
        "the newest row was last in scope order and must lead"
    );
    assert!(
        pairs.iter().all(|pair| pair.1 != 1),
        "the oldest row is the one the cap drops, so the badge must not count it"
    );
    let kept_unseen = pairs.iter().filter(|pair| pair.1 == 9).count();
    assert_eq!(
        kept_unseen, 1,
        "the badge's row set is this capped list: the new finding is on it, the old one is not"
    );
}

/// The heading column groups the text the core sent, not the finding key.
///
/// Mutation: sort Problem by `kind_name`. Two cores that title one finding differently would
/// sit together, and one finding titled alike in two languages would split.
#[test]
fn the_problem_column_sorts_by_the_heading_the_core_sent() {
    let owned = [
        (
            1,
            problem(
                1,
                "paging",
                CoreProblemCategory::Machine,
                "zeta",
                None,
                None,
            ),
        ),
        (
            1,
            problem(
                2,
                "region-blocked",
                CoreProblemCategory::Machine,
                "alpha",
                None,
                None,
            ),
        ),
        (
            2,
            problem(
                3,
                "paging",
                CoreProblemCategory::Exchange,
                "alpha",
                None,
                None,
            ),
        ),
    ];
    let (pairs, _) = sorted_pairs(&owned, "title", true, &HashMap::new());
    assert_eq!(
        pairs.iter().map(|pair| pair.1).collect::<Vec<_>>(),
        vec![2, 3, 1],
        "identical headings stay together, in the order the cores listed them"
    );
}

/// The sign column groups the stable key, not the heading.
///
/// Mutation: sort Sign by `title`. Two cores that name `paging` in different languages would
/// split, which is the case the column exists to survive.
#[test]
fn the_sign_column_sorts_by_the_finding_key() {
    let owned = [
        (
            1,
            problem(
                1,
                "paging",
                CoreProblemCategory::Machine,
                "zeta",
                None,
                None,
            ),
        ),
        (
            1,
            problem(
                2,
                "region-blocked",
                CoreProblemCategory::Machine,
                "alpha",
                None,
                None,
            ),
        ),
        (
            2,
            problem(
                3,
                "paging",
                CoreProblemCategory::Exchange,
                "alpha",
                None,
                None,
            ),
        ),
    ];
    let (pairs, _) = sorted_pairs(&owned, "kind", true, &HashMap::new());
    assert_eq!(
        pairs.iter().map(|pair| pair.1).collect::<Vec<_>>(),
        vec![1, 3, 2],
        "paging from both cores stays together ahead of region-blocked"
    );
}

/// The core column follows the displayed name, case-insensitively, in plain string order.
///
/// Mutation: use a numeric-natural comparison, or the raw core id. `core 2` would then precede
/// `core 10`, disagreeing with alphabetical core order, or an unnamed core would leave the dash
/// the cell prints.
#[test]
fn the_core_column_sorts_by_displayed_name() {
    assert_eq!(core_label(&HashMap::new(), 5), "—");
    let mut names = HashMap::new();
    names.insert(1, "core 2".to_string());
    names.insert(2, "core 10".to_string());
    names.insert(3, "Alpha".to_string());
    names.insert(4, "beta".to_string());
    let row = |kind: u8| problem(kind, "k", CoreProblemCategory::Machine, "t", None, None);
    let owned = [
        (1, row(1)),
        (2, row(2)),
        (3, row(3)),
        (4, row(4)),
        (5, row(5)),
    ];
    let (pairs, _) = sorted_pairs(&owned, "core", true, &names);
    assert_eq!(
        pairs.iter().map(|pair| pair.0).collect::<Vec<_>>(),
        vec![3, 4, 2, 1, 5],
        "Alpha, beta, core 10, core 2, then the unnamed dash"
    );
    let (reversed, _) = sorted_pairs(&owned, "core", false, &names);
    assert_eq!(
        reversed.iter().map(|pair| pair.0).collect::<Vec<_>>(),
        vec![5, 1, 2, 4, 3],
        "the other header direction reverses the same names"
    );
}

/// Category follows the label the cell prints, not the enum declaration order.
///
/// Mutation: sort by the category discriminant. Machine would lead Exchange, and an unknown
/// category byte would trail every known one instead of sitting with the `#` the cell shows.
#[test]
fn the_category_column_sorts_by_the_printed_label() {
    let _locale = crate::test_locale::force("en");
    let owned = [
        (
            1,
            problem(1, "k", CoreProblemCategory::Machine, "t", None, None),
        ),
        (
            1,
            problem(2, "k", CoreProblemCategory::Exchange, "t", None, None),
        ),
        (
            1,
            problem(3, "k", CoreProblemCategory::Other, "t", None, None),
        ),
        (
            1,
            problem(4, "k", CoreProblemCategory::Network, "t", None, None),
        ),
        (
            1,
            problem(5, "k", CoreProblemCategory::Unknown(3), "t", None, None),
        ),
    ];
    let (pairs, _) = sorted_pairs(&owned, "category", true, &HashMap::new());
    assert_eq!(
        pairs.iter().map(|pair| pair.1).collect::<Vec<_>>(),
        vec![5, 2, 1, 4, 3],
        "#3, Exchange, Machine, Network, Other"
    );
}
