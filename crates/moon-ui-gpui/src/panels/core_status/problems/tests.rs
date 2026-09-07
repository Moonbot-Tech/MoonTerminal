//! Explicit imports (no `use super::*`) per the crate's test convention: the panel's parent module
//! re-exports `gpui::*`, whose own `test` would shadow the built-in attribute.

use moon_core::feed::{CoreProblem, CoreProblemCategory};
use rust_i18n::t;

use super::{
    ActionGate, ProblemRow, ProblemsScope, category_label, details_text, drawn_kinds, empty_text,
    fleet_refusal, mark_signature, notice_text,
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
