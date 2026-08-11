//! The shared core-picker's affordances — unified All-row semantics and the saved-groups block —
//! stay wired the same way across all six consumers: Orders, Alerts, Assets, Core Status, Report,
//! Analytics. Prose: the module docs of `controls/core_quick.rs`, `controls/core_host.rs` and
//! `controls/core_combo.rs`.
//!
//! The picker offers exactly two things above its cores: the All row, which CLEARS the selection,
//! and the saved groups. Its group ACTIONS sit at the very bottom, past the core list, because
//! every row above the cores delays the list the picker exists for.
//!
//! Reaching an explicit full selection — the state a user removes ONE core from — is a saved
//! group's job: saving the implicit-All selection materializes every live core, and applying that
//! group yields the explicit set.
//!
//! `CoreComboHost` is a three-method adapter (`core_selection_pinned`, `core_selection_mut`,
//! `after_core_selection_change`), with every picker action applied ONCE through the private
//! `edit_selection` funnel in `core_host.rs`. The pure decisions behind a group click live in
//! `controls/core_groups.rs` and are covered by its own `tests.rs`; what belongs HERE is the
//! WIRING — that every consumer implements the adapter without reimplementing the funnel, and that
//! the menu builds its group block and its bottom actions in the right places.

use super::support::*;

// The Select all row and the exchange rows sharing one id list used to be its own invariant,
// guarding against a search filter narrowing the two independently. The search field is gone —
// `core_combo` now builds both the Select all row and the exchange sections straight from its one
// `cores` parameter, with no filtering step in between — so there is no second list left that
// could diverge from the first. Deliberately not restated here.

/// Every consumer implements `CoreComboHost` via the shared three-method adapter, and none
/// reimplements the pin guard or the apply logic itself.
///
/// Breakage this pins: a future author gives one consumer its own copy of the pin guard or the
/// apply, bypassing the funnel in `core_host.rs::edit_selection` that every picker action must
/// pass through so no action can forget the guard or the reload. Six near-copies of one decision
/// is the shape this file's own history says the picker drifts back into.
#[test]
fn every_consumer_implements_core_combo_host_without_reimplementing_the_funnel() {
    for (panel, module) in [
        ("Analytics", "analytics"),
        ("Orders", "panels/orders"),
        ("Alerts", "panels/alerts"),
        ("Assets", "panels/assets"),
        ("Core Status", "panels/core_status"),
        ("Report", "panels/report"),
    ] {
        let source = code_only(&read_module(module));
        assert!(
            source.contains("impl crate::controls::CoreComboHost for"),
            "{panel} must implement CoreComboHost through the shared trait"
        );
        let body = braced_body(&source, "impl crate::controls::CoreComboHost for");
        for method in [
            "fn core_selection_pinned(",
            "fn core_selection_mut(",
            "fn after_core_selection_change(",
        ] {
            assert!(
                body.contains(method),
                "{panel}'s CoreComboHost impl must define {method}"
            );
        }
        // No ban here on a consumer-local copy of the apply: `apply_core_group` is a free function
        // in `controls/core_groups.rs`, so a consumer duplicating that decision would inline it or
        // name it something else, and a ban on the name would read as coverage while
        // discriminating nothing. The funnel invariant is carried by the three REQUIRED methods
        // above -- a consumer that stops routing through them stops compiling against the trait.
    }
}

/// Every consumer performs its OWN post-selection work inside `after_core_selection_change`, or
/// a picker action (Select all, a group click, a single toggle) changes the selection while
/// leaving stale rows/totals on screen.
///
/// Breakage this pins: a consumer's `after_core_selection_change` drops the panel-specific
/// follow-up (a cache rebuild, a requery, a refresh) that the equivalent single-core `toggle_core`
/// path already performs — every picker action funnels through this one method now, so losing
/// the follow-up here breaks Select all, every group click, AND single-core toggling at once.
#[test]
fn every_consumer_does_its_own_post_selection_work_in_after_core_selection_change() {
    let cases: [(&str, &str, &[&str]); 6] = [
        (
            "Report",
            "panels/report",
            &["reconcile_strategy_core(", "request_requery("],
        ),
        ("Analytics", "analytics", &["core_selection_changed("]),
        ("Orders", "panels/orders", &["rebuild_cache("]),
        ("Assets", "panels/assets", &["rebuild_cache("]),
        ("Core Status", "panels/core_status", &["rebuild_cache("]),
        ("Alerts", "panels/alerts", &["refresh("]),
    ];
    for (panel, module, required_calls) in cases {
        let source = code_only(&read_module(module));
        let body = braced_body(&source, "fn after_core_selection_change(");
        for call in required_calls {
            assert!(
                body.contains(call),
                "{panel}'s after_core_selection_change must perform its own `{call}` follow-up, \
                 or every picker action leaves stale rows on screen"
            );
        }
    }
}

/// No consumer re-implements the All/one-core decision itself; every `toggle_core` routes through
/// the shared `toggle_core_selection`, and the two superseded per-consumer helpers stay gone.
///
/// Breakage this pins: a future author "inlines" the All/toggle logic straight into one consumer's
/// `toggle_core` instead of calling the shared decision — the six consumers would then drift apart
/// the next time the convention changes, exactly the bug this module exists to prevent.
#[test]
fn no_consumer_reimplements_the_toggle_decision() {
    for (panel, module) in [
        ("Analytics", "analytics"),
        ("Orders", "panels/orders"),
        ("Alerts", "panels/alerts"),
        ("Assets", "panels/assets"),
        ("Core Status", "panels/core_status"),
        ("Report", "panels/report"),
    ] {
        let source = code_only(&read_module(module));
        assert!(
            source.contains("fn toggle_core("),
            "{panel} must keep its own toggle_core entry point"
        );
        let body = braced_body(&source, "fn toggle_core(");
        assert!(
            body.contains("toggle_core_selection("),
            "{panel}'s toggle_core must route through the shared toggle_core_selection decision"
        );
    }

    let mut sources = Vec::new();
    rust_sources(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut sources,
    );
    for path in sources {
        let text = code_only(&fs::read_to_string(&path).unwrap());
        assert!(
            !text.contains("toggle_all_core_selection")
                && !text.contains("toggle_analytics_core_selection"),
            "{}: the superseded per-consumer toggle helpers must stay deleted",
            path.display()
        );
    }
}

// The unnamed exchange section staying a clickable batch row (like every named exchange) is
// pinned by `shell::shared_core_selectors_batch_exchange_changes_once`, scoped to `core_combo<`'s
// own body — deliberately not duplicated here.

// `core_combo_extras` (`core_host.rs`) builds the Select all handler AND the whole saved-groups
// block (apply/save/manage) now, all captured WEAKLY through `view.downgrade()`. That still
// matters: MoonUI stores every menu row's `on_click` closure at a refcounted menu level for as
// long as the menu stays open, so a strong `Entity<T>` capture in any of these handlers would
// close a `view -> menu-row-closure -> view` cycle and the view would never drop. This module's
// own doc comment already calls out the rule; `core_host.rs`'s module doc restates it for the
// group handlers specifically.

// `reset_core_search` and the `CoreSearchState` it cleared are gone with the search field itself —
// there is no query and no retained field left to clear. Deliberately not restated here.

/// A group row applies through the shared `apply_core_group` decision and computes no intent of
/// its own -- the Union/Replace choice comes from `GroupClick::from_secondary`, not a hand-rolled
/// branch on the click's modifier.
///
/// Breakage this pins (`controls/core_host.rs::apply_group_click`): a future author inlines
/// `if secondary { ... } else { ... }` instead of calling `GroupClick::from_secondary`, quietly
/// duplicating a decision that would then drift the next time the modifier convention changes.
#[test]
fn group_rows_apply_through_the_shared_decision_and_compute_no_intent_inline() {
    let host = code_only(&read_src("controls/core_host.rs"));
    let body = braced_body(&host, "fn apply_group_click<T: CoreComboHost>(");

    assert!(
        body.contains("apply_core_group("),
        "a group row click must apply through core_groups::apply_core_group"
    );
    assert!(
        body.contains("GroupClick::from_secondary("),
        "the Union/Replace intent must come from GroupClick::from_secondary, not be computed \
         inline"
    );
    assert!(
        !body.contains("if secondary"),
        "apply_group_click must not branch on `secondary` itself -- that recomputes the intent \
         GroupClick::from_secondary already owns"
    );
}

/// A clicked group row resolves by NAME at click time, never by the index the menu was built
/// with.
///
/// Breakage this pins (`controls/core_host.rs::apply_group_click`): resolving by position
/// instead of name would apply whatever group now sits at that index after the management modal
/// -- possibly in another window, since the saved list is application state -- reordered or
/// deleted one under an open menu.
#[test]
fn a_group_row_resolves_by_name_never_by_index() {
    let host = code_only(&read_src("controls/core_host.rs"));
    // Scoped to the click path, not the whole file: a name lookup ANYWHERE in `core_host.rs` would
    // otherwise satisfy this while the click itself went back to positions.
    let click = braced_body(&host, "fn apply_group_click<T: CoreComboHost>(");
    assert!(
        click.contains(".find(|group| group.name == name)"),
        "a clicked group row must resolve by NAME at click time"
    );
    let squashed: String = click.chars().filter(|c| !c.is_whitespace()).collect();
    // Every positional form, not just `.get(` -- indexing, `nth` and `skip` reach a position too,
    // and a ban that names one spelling is a ban on that spelling rather than on the mistake.
    for positional in [
        "core_groups.get(",
        "core_groups[",
        ".nth(",
        ".skip(",
        "core_groups.iter().enumerate()",
    ] {
        assert!(
            !squashed.contains(positional),
            "a group row resolved by POSITION ({positional}) applies whatever group now sits at \
             that index after a reorder under an open menu, instead of the one clicked"
        );
    }
}

/// A saved-group row shows a TICK when the current selection is exactly that group, and it gets
/// that answer from the pure `group_is_applied` rather than deciding inline.
///
/// This reverses the picker's earlier rule that a group row must never look like state, and the
/// distinction is worth keeping straight: an exchange heading TOGGLES, so it has no stable "on"
/// and keeps a `3/8` count instead; a group row APPLIES a fixed set, so "the selection IS this
/// group" is a well-defined fact about the present, not a promise about a click. Clicking an
/// already-ticked group is inert, so the tick can never contradict the gesture.
///
/// Breakage this pins (`controls/core_combo.rs::groups_block`): computing the tick inline — say as
/// `selected.contains(...)` over any member, or against the raw saved list instead of the members
/// this consumer can actually select — would tick a group whose scope the user does not have.
#[test]
fn a_group_row_ticks_only_when_the_selection_is_that_group() {
    let combo = code_only(&read_src("controls/core_combo.rs"));
    let body = braced_body(&combo, "fn groups_block(");
    assert!(
        body.contains("group_is_applied(&group.cores, &pickable, selected)"),
        "the group row's tick must come from the shared group_is_applied predicate, against the          pickable set -- not from an inline selection test"
    );
    assert!(
        body.contains(".checked(applied)"),
        "the group row must render that answer as its checked state"
    );
}

/// Groups persist in SETTINGS (`schema::SettingsFile`), never in `layout::WindowLayout` --
/// unlike per-window presentation state, a saved group is application-wide and must not fork
/// per host window.
///
/// Breakage this pins: a future author "helpfully" adds a per-window group cache to
/// `WindowLayout`, duplicating the one true list `schema::SettingsFile::core_groups` already
/// holds and letting the two drift.
#[test]
fn core_groups_persist_in_settings_not_layout() {
    let moon_core_src = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("moon-core")
        .join("src")
        .join("config");
    let schema = code_only(&fs::read_to_string(moon_core_src.join("schema.rs")).unwrap());
    let layout = code_only(&fs::read_to_string(moon_core_src.join("layout.rs")).unwrap());

    let settings_body = braced_body(&schema, "pub struct SettingsFile {");
    assert!(
        settings_body.contains("pub core_groups: Vec<CoreGroup>,"),
        "SettingsFile must own the saved core groups"
    );
    assert!(
        !layout.contains("core_groups"),
        "WindowLayout must never carry its own copy of the saved core groups"
    );
}

/// The saved-groups block sits between the picker's own top rows (All, Select all) and the first
/// exchange separator -- i.e. it renders as one unit above the core list, not interleaved with
/// or below the exchange sections.
///
/// Breakage this pins (`controls/core_combo.rs::core_combo`): moving the `groups_block(` call
/// after the exchange separator would bury the group affordances below fifty cores' worth of
/// exchange rows.
#[test]
fn the_groups_block_sits_above_the_exchange_separator() {
    let combo = code_only(&read_src("controls/core_combo.rs"));
    let body = braced_body(&combo, "pub(crate) fn core_combo<F, G>(");
    let groups_at = body
        .find("groups_block(")
        .expect("core_combo must call groups_block");
    let separator_at = body
        .find("if !sections.is_empty()")
        .expect("core_combo must emit the exchange separator");
    assert!(
        groups_at < separator_at,
        "groups_block must be appended BEFORE the exchange separator, not after"
    );

    // ...and the two management ACTIONS after every core row. They sit at the bottom because each
    // row above the core list delays the cores the picker exists for -- with them on top, one
    // saved group put five rows of chrome ahead of the first core.
    let actions_at = body
        .find("group_actions_block(")
        .expect("core_combo must call group_actions_block");
    assert!(
        separator_at < actions_at,
        "group_actions_block must be appended AFTER the exchange sections, at the menu's bottom"
    );
}

/// Both dialog-opening rows (Save, Manage) carry `.closes_menu(true)`, and the picker itself
/// keeps `close_on_select(false)` -- the exact combination that lets checkbox-style core/exchange
/// rows survive a click while the two rows that open a MODAL take the menu down with them.
///
/// Breakage this pins: dropping either half reintroduces the bug this fork's `closes_menu`
/// affordance exists to fix -- MoonUI defers popovers above the dialog layer, so a menu left
/// standing paints OVER the modal it just opened.
#[test]
fn dialog_rows_close_the_menu_while_the_menu_itself_survives_a_click() {
    let combo = code_only(&read_src("controls/core_combo.rs"));
    let actions_body = braced_body(&combo, "fn group_actions_block(");
    assert_eq!(
        actions_body.matches(".closes_menu(true)").count(),
        2,
        "both the Save row and the Manage row must carry .closes_menu(true)"
    );

    let core_combo_body = braced_body(&combo, "pub(crate) fn core_combo<F, G>(");
    assert!(
        core_combo_body.contains(".close_on_select(false)"),
        "the picker itself must stay close_on_select(false) so its checkbox rows survive a click"
    );
}

/// The removed controlled-dropdown apparatus (a mirrored open/close flag per consumer, driven by
/// its own `set_core_menu_open` on the trait) must not come back now that `.closes_menu(true)`
/// handles the one case it existed for.
///
/// Breakage this pins: a future author reintroduces a `core_menu_open` field on a consumer to
/// manage the dropdown by hand -- the exact apparatus `.closes_menu(true)` replaced, which this
/// module's own doc comment calls out as buying "a mirrored flag and a retained callback each"
/// for nothing.
#[test]
fn the_removed_controlled_dropdown_apparatus_stays_gone() {
    for (panel, module) in [
        ("Analytics", "analytics"),
        ("Orders", "panels/orders"),
        ("Alerts", "panels/alerts"),
        ("Assets", "panels/assets"),
        ("Core Status", "panels/core_status"),
        ("Report", "panels/report"),
    ] {
        let source = code_only(&read_module(module));
        assert!(
            !source.contains("core_menu_open"),
            "{panel} must not declare a core_menu_open field -- .closes_menu(true) replaced it"
        );
    }
    let host = code_only(&read_src("controls/core_host.rs"));
    assert!(
        !host.contains("set_core_menu_open"),
        "CoreComboHost must not declare set_core_menu_open -- the controlled-dropdown apparatus \
         stays gone"
    );
}

/// The Save row's enabled gate and the payload its click stores are the same predicate --
/// `saves_core` -- so the two can never disagree about which cores would be saved.
///
/// Breakage this pins (`controls/core_combo.rs::groups_block`): if the gate used a different
/// rule than the payload (e.g. checking only `selectable` instead of `saves_core`'s
/// selected-AND-configured rule), the Save row could read as enabled while producing an empty
/// group, or vice versa.
#[test]
fn the_save_row_gate_and_payload_share_one_predicate() {
    let combo = code_only(&read_src("controls/core_combo.rs"));
    let body = braced_body(&combo, "fn group_actions_block(");
    assert!(
        body.contains("saves_core(selected, &extras.configured"),
        "the Save row's enabled gate must call the shared saves_core predicate against the \
         extras' own configured set"
    );
    assert!(
        body.contains("saved_group_cores(&save_selected, &save_selectable, &save_configured"),
        "the Save click's payload must be built through saved_group_cores, the constructive half \
         of the same predicate"
    );
}

/// A group row's applicable count reads against `selectable` (what THIS consumer can pick), and
/// its dead count reads against `configured` (every uid in `AppConfig.servers`) -- the two sets
/// are not interchangeable.
///
/// Breakage this pins (`controls/core_combo.rs::groups_block`): swapping the two arguments would
/// make, e.g., Orders (scoped to one server group) report every live core outside that group as
/// "missing", while the trailing count a group's CLICK would actually produce reads against the
/// wrong set entirely.
#[test]
fn group_row_facts_count_applicable_against_selectable_and_dead_against_configured() {
    let combo = code_only(&read_src("controls/core_combo.rs"));
    let body = braced_body(&combo, "fn groups_block(");
    assert!(
        body.contains("applicable_count(&group.cores, &pickable)"),
        "the applicable count must be read against `pickable` (selectable), not configured"
    );
    assert!(
        body.contains("group_dead_count(&group.cores, &extras.configured)"),
        "the dead-member count must be read against the CONFIGURED cores, not this panel's \
         narrower selectable scope"
    );
}
