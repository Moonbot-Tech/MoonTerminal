//! The shared core-picker's affordances (the Select all action, unified All-row semantics) stay
//! wired the same way across all six consumers: Orders, Alerts, Assets, Core Status, Report,
//! Analytics. Prose: `controls/core_quick.rs`'s module doc and `controls/core_combo.rs`'s module
//! doc.
//!
//! The search field and its `CoreSearchState` were removed at the owner's request; nothing here
//! covers it because there is nothing left to cover. `Invert` followed it out for the same reason
//! (both removals are the owner's call, not a defect): with only Select all left, `QuickAction`
//! collapsed from an enum to nothing, and its two entry points (`apply_quick_action` /
//! `quick_action_preview`) became the direct `select_all_cores` / `select_all_preview`.

use super::support::*;

/// The Select all row is an ACTION, never rendered as state.
///
/// Breakage this pins (`controls/core_combo.rs:select_all_row`): a future author adds
/// `.checked(...)` to the row, turning a one-shot batch action into something that reads as a
/// persistent toggle — the row would then look ticked/unticked instead of firing once.
#[test]
fn select_all_row_is_an_action_not_checkable_state() {
    let combo = code_only(&read_src("controls/core_combo.rs"));
    let row = braced_body(&combo, "fn select_all_row(");

    assert!(
        row.contains("MoonMenuItem::action_label("),
        "the Select all row must be built as an action item"
    );
    assert!(
        !row.contains(".checked("),
        "the Select all row must never carry a checked state"
    );
}

// The Select all row and the exchange rows sharing one id list used to be its own invariant,
// guarding against a search filter narrowing the two independently. The search field is gone —
// `core_combo` now builds both the Select all row and the exchange sections straight from its one
// `cores` parameter, with no filtering step in between — so there is no second list left that
// could diverge from the first. Deliberately not restated here.

/// Every consumer's `select_all_cores` trait method actually applies the click, through the shared
/// decision.
///
/// The needle is the QUALIFIED call, `crate::controls::select_all_cores(`, not the bare
/// `select_all_cores(` — the consumer's OWN trait method carries that exact name too now that
/// `QuickAction` collapsed to a single action, so slicing the body from the signature `fn
/// select_all_cores(` and then searching it for the unqualified name would trivially match the
/// signature line itself and could never fail. The qualified path is the shared decision in
/// `core_quick.rs`, re-exported at `crate::controls::select_all_cores`, and does not appear in any
/// method's own signature.
///
/// Breakage this pins: a consumer implements the `CoreComboHost` trait method (so it compiles and
/// the menu opens) but its body never calls the shared decision, leaving the wired-up Select all
/// row silently inert for that view.
#[test]
fn every_consumer_routes_select_all_through_the_shared_decision() {
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
            source.contains("fn select_all_cores("),
            "{panel} must implement CoreComboHost::select_all_cores"
        );
        let body = braced_body(&source, "fn select_all_cores(");
        assert!(
            body.contains("crate::controls::select_all_cores("),
            "{panel}'s select_all_cores must call the shared crate::controls::select_all_cores \
             decision"
        );
    }
}

/// Every consumer performs its OWN post-selection work inside `select_all_cores`, or the action
/// changes the selection while leaving stale rows/totals on screen.
///
/// Breakage this pins: a consumer's `select_all_cores` calls the shared decision but drops the
/// panel-specific follow-up (a cache rebuild, a requery, a refresh) that the equivalent single-core
/// `toggle_core` path already performs — the click would change `sel_cores` with nothing on screen
/// reacting to it.
#[test]
fn every_consumer_does_its_own_post_selection_work_in_select_all_cores() {
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
        let body = braced_body(&source, "fn select_all_cores(");
        for call in required_calls {
            assert!(
                body.contains(call),
                "{panel}'s select_all_cores must perform its own `{call}` follow-up, or the action \
                 leaves stale rows on screen"
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

// `core_combo_extras` used to retain an open/close callback and a header builder across frames,
// which is why capturing them WEAKLY mattered: MoonUI held both for as long as the popup state
// lived, so a strong `view.clone()` there would have closed a `view -> popup state -> closure ->
// view` cycle. With the search field gone, `core_combo_extras` builds only the Select all handler
// now, and that handler lives inside a per-frame `MoonMenuItem` closure — the same kind this
// module's doc comment already calls out as safe to capture strongly, since MoonUI does not retain
// a menu row past the frame that built it. There is no longer a retained capture for a weak-handle
// invariant to protect. Deliberately not restated here.

// `reset_core_search` and the `CoreSearchState` it cleared are gone with the search field itself —
// there is no query and no retained field left to clear. Deliberately not restated here.
