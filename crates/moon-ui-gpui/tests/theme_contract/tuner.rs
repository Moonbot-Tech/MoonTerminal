//! The Analytics tuner: mode buttons, virtualized tables, the field-checkbox mask, automatic
//! set composition and the machine bar, the distribution card, and row-click gestures.

use super::support::*;

/// `analytics/tuner/shared.rs::collapse_caret` must pass the `DownUp` pose explicitly, ON the
/// `MoonDisclosure` chain it builds.
///
/// `MoonDisclosure`'s own default is `RightDown` (collapsed points right, expanded points down —
/// the strategy tree's own convention). The plausible edit is deleting `.direction(..)` as
/// "redundant" now that a pose is set elsewhere; that silently switches both the KPI matrix's and
/// the Distribution card's collapse carets from the up/down convention (▼ collapsed, ▲ expanded)
/// to right/down (▶ collapsed, ▼ expanded), disagreeing with the tree next to them. Nothing else
/// reddens — it compiles, lays out identically and still toggles the card.
///
/// A bare identifier search over the whole function body is under-anchored: it stays green if
/// `.direction(MoonDisclosureDirection::DownUp)` is deleted from the chain while the identifier
/// survives anywhere else in the function (an unrelated `let` binding, a dead-code leftover from
/// a rebase), because it only requires the TEXT `MoonDisclosureDirection::DownUp` to appear
/// somewhere, not that the chain actually calls `.direction(..)` with it. Slicing to the
/// `MoonDisclosure::button(..)..into_any_element()` chain and requiring the CALL, not just the
/// identifier, closes that gap.
#[test]
fn the_tuner_collapse_caret_uses_the_down_up_pose() {
    let shared = read_src("analytics/tuner/shared.rs");
    let body = code_only(braced_body(&shared, "pub(super) fn collapse_caret("));
    let chain = chain_between(
        &body,
        "MoonDisclosure::button(",
        ".into_any_element()",
        "the tuner collapse caret's MoonDisclosure chain",
    );
    assert!(
        chain.contains(".direction(MoonDisclosureDirection::DownUp)"),
        "collapse_caret must call .direction(MoonDisclosureDirection::DownUp) on its \
         MoonDisclosure chain, not merely reference the identifier elsewhere in the function"
    );
}

/// The Tuning card header: mode-button order and a title that shares the buttons' box.
///
/// The plausible edits are alphabetizing the three labels, or dropping the explicit height
/// while simplifying the title to a bare `div`, making a 16px glyph sit low against an 18px pill.
#[test]
fn tuning_mode_buttons_keep_their_order_and_baseline() {
    let table = read_src("analytics/tuner/list/table.rs");
    let header = braced_body(&table, "fn strat_list_card(");
    let at = |needle: &str| {
        header
            .find(needle)
            .unwrap_or_else(|| panic!("{needle} must be rendered in the card header"))
    };
    let (filters, time, coins) = (at("\"sm-filters\""), at("\"sm-time\""), at("\"sm-coins\""));
    assert!(
        filters < time && time < coins,
        "the axis buttons must read filter, time, coin"
    );
    assert!(
        at("design::micro_control_h(cx)") < filters,
        "the matched height must sit on the TITLE, which precedes the first button — not on \
         some other element of the header"
    );
    assert_eq!(
        header.matches("mode_btn(").count(),
        3,
        "exactly three axis buttons — a fourth needs this test updated deliberately, not a \
         copy-pasted line the ordering assertion above would happily accept"
    );
}

/// The Tuning strategy list stays virtualized and reads its rows from the memo.
///
/// The plausible edit is rebuilding the list eagerly (`for g in rows`) while chasing a layout
/// bug, or recomputing the filter inline instead of through `ensure_visible`; either makes mouse
/// movement sort thousands of groups repeatedly.
#[test]
fn the_tuning_strategy_list_stays_virtualized() {
    let table = read_src("analytics/tuner/list/table.rs");
    // Scope every assertion to the list card's own body, so an unrelated scrollable sub-surface
    // elsewhere in this file cannot redden the test for a reason that has nothing to do with
    // the list — and so inserting a helper below it cannot silently widen the window.
    let card = braced_body(&table, "fn strat_list_card(");
    // Two substrings rather than one: rustfmt breaks the call across lines once its arguments
    // grow, and a test that reddens on a reflow is a test people learn to ignore.
    assert!(
        card.contains("MoonVirtualList::new(") && card.contains("\"an-strat-rows\""),
        "the strategy list must render through MoonVirtualList"
    );
    assert!(
        !card.contains(".overflow_y_scroll()"),
        "an eager scroll container would defeat the virtual list"
    );
    assert!(
        card.contains("ensure_visible(") && card.contains("visible_indices()"),
        "rows must come from the memoized index list, not a fresh filter+sort per render"
    );
    // The factory outlives the render, so a strong handle would leak the whole window — the
    // same cycle `moon_tree_closures_hold_weak_view_handles` guards for MoonTree. Asserting on
    // `weak.upgrade()` INSIDE the factory, not merely that a weak handle exists in the file:
    // `cx.listener` cannot appear in a `'static` factory at all, so banning it proves nothing.
    let factory = card
        .find("MoonVirtualList::new(")
        .map(|i| &card[i..])
        .expect("the virtual list must be built here");
    assert!(
        factory.contains("weak.upgrade()"),
        "the virtual row factory must reach the view through a WEAK handle"
    );
    assert!(
        table.contains("weak: &WeakEntity<AnalyticsView>"),
        "the row builder must take the weak handle rather than capturing the view"
    );
}

/// `AnalyticsView::new` must retain one strategy-list scroll handle and `strat_list_card` must
/// reuse it; constructing the handle in render or omitting `track_scroll` sends the list back to
/// row zero whenever a background valuation refresh replaces the strategy data.
#[test]
fn the_tuning_strategy_list_retains_scroll_across_refreshes() {
    let analytics = read_src("analytics/mod.rs");
    let table = read_src("analytics/tuner/list/table.rs");
    let card = braced_body(&table, "fn strat_list_card(");

    assert_eq!(
        analytics
            .matches("MoonVirtualListScrollHandle::new()")
            .count(),
        1,
        "AnalyticsView must construct exactly one retained strategy scroll handle"
    );
    assert!(
        analytics.contains("strat_scroll: MoonVirtualListScrollHandle::new(),"),
        "the sole strategy scroll handle must be initialized with AnalyticsView state"
    );
    let list = card
        .find("\"an-strat-rows\"")
        .map(|index| &card[index..])
        .expect("the strategy virtual list must retain its stable id");
    assert!(
        list.contains(".track_scroll(&self.strat_scroll)"),
        "the strategy virtual list must reuse AnalyticsView's retained scroll handle"
    );
    assert!(
        !card.contains("MoonVirtualListScrollHandle::new()"),
        "render must not replace the retained strategy scroll handle"
    );
}

/// Changing the strategy list back to hover-only scrolling must fail this assertion; the user must
/// be able to see the vertical position and scrollbar affordance without first finding it.
#[test]
fn the_strategy_scrollbar_is_always_visible() {
    let table = read_src("analytics/tuner/list/table.rs");
    let card = braced_body(&table, "fn strat_list_card(");
    assert!(
        card.contains(".scrollbar_visibility(MoonScrollbarVisibility::Always)"),
        "the strategy virtual list must expose its scrollbar continuously"
    );
}

/// Removing or moving `select_for_report` after `open_strategy_report` must fail this assertion;
/// otherwise the first native click can toggle the highlight off behind the standalone Report.
#[test]
fn double_click_restores_selection_before_opening_the_report() {
    let table = read_src("analytics/tuner/list/table.rs");
    let row = braced_body(&table, "fn strategy_row(");
    let select = row
        .find("this.select_for_report(&key, &name, cx)")
        .expect("double-click must preserve the clicked selection");
    let open = row
        .find("this.open_strategy_report(&key, name, window, cx)")
        .expect("double-click must open the scoped Report");
    assert!(
        select < open,
        "selection must be restored before the window opens"
    );
}

/// The Tuning coin table stays virtualized and reaches the view weakly.
///
/// Replacing `MoonVirtualList` with eager `v_flex().children(rows)`, or wrapping the card in
/// `overflow_y_scroll`, makes each shared-view repaint build up to `MAX_ROWS` coin rows. Capturing
/// a strong view handle in the retained factory additionally leaks the Analytics window.
#[test]
fn the_tuning_coin_table_stays_virtualized() {
    let coins = read_src("analytics/tuner/coins/mod.rs");
    // Scoped to the card's own body: another scrollable sub-surface in this file (the picker)
    // must not redden the test for a reason that has nothing to do with the table.
    let card = braced_body(&coins, "fn coins_card(");
    assert!(
        card.contains("MoonVirtualList::new(") && card.contains("\"an-coin-rows\""),
        "the coin table must render through MoonVirtualList"
    );
    assert!(
        !card.contains(".overflow_y_scroll()"),
        "an eager scroll container would defeat the virtual list"
    );
    assert!(
        card.contains("rows::rows_for("),
        "rows must come from the memoized cache, not a fresh filter+sort per render"
    );
    // The factory outlives the render, so a strong handle would leak the whole window. Asserted
    // INSIDE the factory: `cx.listener` cannot appear in a `'static` factory at all, so banning
    // it proves nothing.
    let factory = card
        .find("MoonVirtualList::new(")
        .map(|i| &card[i..])
        .expect("the virtual list must be built here");
    assert!(
        factory.contains("weak.upgrade()"),
        "the virtual row factory must reach the view through a WEAK handle"
    );
    // Scoped to the row builder rather than banned file-wide: a short-lived `cx.entity()` in some
    // other method here is legitimate, and only the row's own handlers outlive the frame.
    let row = braced_body(&coins, "fn coin_row(");
    assert!(
        !row.contains("cx.entity()"),
        "a coin row's handlers outlive the render; they must capture the weak handle only"
    );
    assert!(
        coins.contains("weak: &WeakEntity<AnalyticsView>"),
        "the row builder must take the weak handle rather than capturing the view"
    );
}

/// The joint threshold search must leave its own window usable while it runs.
///
/// The busy overlay appears 150 ms into a batch and SWALLOWS CLICKS, so a search that raised it
/// would bury the Stop button it depends on — and this search runs for as long as the user asks
/// it to. The plausible edit is "make the long search look busy like everything else": passing
/// `true` to `spawn_db`, or calling `op_started` directly.
#[test]
fn the_joint_threshold_search_leaves_its_window_usable() {
    let actions = read_src("analytics/tuner/filter/actions.rs");
    let body = actions
        .split_once("fn suggest_into_v1(")
        .expect("filter/actions.rs must contain suggest_into_v1")
        .1
        .split("\n    }\n")
        .next()
        .unwrap();
    let spawn_args = body
        .split_once("self.spawn_db(")
        .expect("suggest_into_v1 must run its search through spawn_db")
        .1
        .trim_start();
    assert!(
        spawn_args.starts_with("false,"),
        "the joint search must not raise the blocking overlay over its own Stop button"
    );
    assert!(
        !body.contains("op_started("),
        "the joint search must not enter the blocking-overlay accounting by hand either"
    );
}

/// The restart box must show the same bounded count that Search will execute.
///
/// The binary crate offers no public GPUI surface for driving this input. The pure parser test
/// proves the canonical value, while this source contract proves Blur/Enter writes that value
/// back through the real input state. Removing `canonical_iters` or returning to `subscribe`
/// leaves an over-limit number visible even though persistence and Search silently clamp it.
#[test]
fn the_restart_box_canonicalizes_its_executed_count() {
    let shell = read_src("analytics/tuner/shell.rs");
    let body = braced_body(&shell, "fn shell_cfg_input(");
    assert!(
        body.contains("subscribe_in("),
        "`shell_cfg_input` needs the window-aware subscription to rewrite finished input"
    );
    let normalization = body
        .split_once("let value = if matches!(")
        .expect("the input callback must choose whether to canonicalize")
        .1
        .split_once("match (kind, which)")
        .expect("normalization must happen before the setting is committed")
        .0;
    for needle in [
        "TunerKind::Filter",
        "CfgInput::Restarts",
        "MoonInputEvent::Blur",
        "MoonInputEvent::PressEnter",
        "canonical_iters(&raw)",
    ] {
        assert!(
            normalization.contains(needle),
            "restart Blur/Enter normalization must contain {needle:?}"
        );
    }
    let writeback = normalization
        .split_once("if value != raw {")
        .expect("a changed canonical value must be written back")
        .1;
    assert!(
        writeback.contains("input.set_value(value.clone(), window, cx)"),
        "the canonical restart count must replace the visible raw input"
    );
}

/// Manual v1 edits must retire a suggestion started from an older draft. Removing
/// either call lets a late Filter/Time sweep silently overwrite the user's input.
#[test]
fn manual_v1_edits_retire_filter_and_time_suggestions() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("analytics")
        .join("tuner");
    let filter = fs::read_to_string(root.join("filter").join("mod.rs")).unwrap();
    let filter_actions = fs::read_to_string(root.join("filter").join("actions.rs")).unwrap();
    let time = fs::read_to_string(root.join("time").join("grid.rs")).unwrap();
    fn method<'a>(source: &'a str, signature: &str) -> &'a str {
        source
            .split_once(signature)
            .unwrap_or_else(|| panic!("expected method `{signature}`"))
            .1
            .split("\n    }\n")
            .next()
            .unwrap()
    }

    for (source, signature, invalidation) in [
        (
            &filter,
            "fn commit_bound(",
            "self.tuner.invalidate_suggest();",
        ),
        (
            &filter,
            "fn apply_bounds(",
            "self.tuner.invalidate_suggest();",
        ),
        (
            &filter_actions,
            "fn copy_v2_to_v1(",
            "self.tuner.invalidate_suggest();",
        ),
        (
            &filter_actions,
            "fn clear_variant(",
            "self.tuner.invalidate_suggest();",
        ),
        (
            &time,
            "fn commit_time(",
            "self.time_tuner.invalidate_suggest();",
        ),
        (
            &time,
            "fn clear_field(",
            "self.time_tuner.invalidate_suggest();",
        ),
        (
            &time,
            "fn set_v1_cell(",
            "self.time_tuner.invalidate_suggest();",
        ),
    ] {
        assert!(
            method(source, signature).contains(invalidation),
            "{signature} must retire the in-flight suggestion before changing v1"
        );
    }
}

/// `tuner_query` must scope strategies by the READ filter, never the write authority.
///
/// Mutation: swapping `self.read_core_ids()` for `self.action_core_ids()` reads unchanged on
/// every rail except Auto+Overview, where the core dropdown is unpinned but write access still
/// stays confined to the group. There, a selected out-of-group strategy row would drop out of
/// `q.strategies`, so the query comes back empty and the tuner silently analyses EVERY strategy
/// the (unpinned) core filter admits while the page still shows one row selected.
#[test]
fn tuner_query_scopes_strategies_by_the_read_filter() {
    let filter = read_src("analytics/tuner/filter/mod.rs");
    let body = code_only(braced_body(&filter, "fn tuner_query("));
    assert!(
        body.contains("self.visible_target_keys(self.read_core_ids())"),
        "`tuner_query` must scope `q.strategies` through `read_core_ids`, the READ authority; \
         scoping through `action_core_ids` would silently widen the analysis on Auto+Overview"
    );
}

/// `selected_targets` feeds Save/Copy/purge and must scope by the WRITE authority, never the
/// read filter.
///
/// Mutation: swapping `self.action_core_ids()` for `self.read_core_ids()` reads unchanged on
/// every rail except Auto+Overview, where the core dropdown is unpinned. There, a destructive
/// write would reach a target outside the focused Auto group — this is the access-control
/// boundary the filter/action split exists to hold.
#[test]
fn selected_targets_scopes_by_the_write_authority() {
    let tuner = read_src("analytics/tuner/mod.rs");
    let body = code_only(braced_body(&tuner, "fn selected_targets(&self)"));
    assert!(
        body.contains("self.targets_visible_in(self.action_core_ids())"),
        "`selected_targets` must scope through `action_core_ids`, the WRITE authority; scoping \
         through `read_core_ids` would let Save/Copy reach a target outside the focused group"
    );
}

/// Every checkbox that changes the tuner's field selection must also persist it.
///
/// `moon-ui-gpui` is a binary crate, so the click handlers live in GPUI closures no integration
/// test can call — the wiring can only be pinned at the source level. The pure encode/restore
/// tests in `filter/state/tests.rs` stay green whether or not a handler ever writes to the layout.
///
/// The plausible edit is adding or reworking a checkbox path and mutating `tuner.enabled` without
/// the persist call, which reads as working until the window is reopened and the change is gone.
#[test]
fn tuner_field_checkboxes_persist_every_change() {
    let filter = read_src("analytics/tuner/filter/mod.rs");

    // The invariant is STRUCTURAL, not arithmetic: the mask has exactly two writers, and both
    // persist. Counting writes against persist calls would have been coincidental — the master
    // checkbox writes inside a `for` loop, so one persist correctly answers N writes, and a new
    // write added inside an already-persisting handler would pass unnoticed.
    for (writer, body) in [
        (
            "fn set_field_enabled(",
            braced_body(&filter, "fn set_field_enabled("),
        ),
        (
            "fn set_all_fields_enabled(",
            braced_body(&filter, "fn set_all_fields_enabled("),
        ),
    ] {
        assert!(
            body.contains("persist_tuner_fields(cx)"),
            "{writer} writes the checkbox mask, so it must persist it"
        );
    }
    // Nothing outside those two funnels may WRITE the mask — that is what makes the persist
    // structural instead of a rule each future handler has to remember. Reads (`.checked(...)`)
    // are unrestricted, hence the assignment test rather than a bare substring count.
    let in_funnels = enabled_mask_writes(braced_body(&filter, "fn set_field_enabled("))
        + enabled_mask_writes(braced_body(&filter, "fn set_all_fields_enabled("));
    assert_eq!(
        in_funnels, 2,
        "each funnel must write the mask exactly once"
    );
    assert_eq!(
        enabled_mask_writes(&filter),
        in_funnels,
        "the checkbox mask must be written only through `set_field_enabled` / \
         `set_all_fields_enabled`, never inline in a click handler"
    );

    // The stored value must be the column ids, never a positional mask: `FIELDS` is presentation
    // order, so a saved position would tick different boxes after any reorder.
    let shell = read_src("analytics/tuner/shell.rs");
    assert!(
        shell.contains("fn persist_tuner_fields(") && shell.contains("self.tuner.enabled_cols()"),
        "persistence must store the enabled COLUMN IDS, not indices into FIELDS"
    );
}

/// Mutations of the tuner's checkbox mask in `src`; reads (`.checked(..)`, `.iter()`) excluded.
///
/// Counts every shape a write can take, not just the indexed one. An indexed assignment is what
/// the two funnels happen to use TODAY, and a checker that recognized only that would wave
/// through `tuner.enabled = chosen` or `tuner.enabled.fill(false)` — precisely the forms an
/// automatic composition would reach for if it ever decided to tick the boxes itself.
fn enabled_mask_writes(src: &str) -> usize {
    /// Methods that mutate the vector in place.
    const MUTATORS: [&str; 9] = [
        ".fill(",
        ".clear(",
        ".push(",
        ".iter_mut(",
        ".resize(",
        ".truncate(",
        ".retain(",
        ".extend(",
        ".copy_from_slice(",
    ];
    src.match_indices("tuner.enabled")
        .filter(|(at, _)| {
            let after = &src[at + "tuner.enabled".len()..];
            // Whole-vector assignment, or a mutating method on it.
            if after.starts_with(" = ") || MUTATORS.iter().any(|m| after.starts_with(m)) {
                return true;
            }
            // Indexed assignment: `tuner.enabled[..] = ..`.
            after.starts_with('[')
                && after
                    .find(']')
                    .and_then(|close| after.get(close + 1..))
                    .is_some_and(|tail| tail.starts_with(" = "))
        })
        .count()
}

/// Automatic composition reports which fields it chose; it never ticks the user's checkboxes.
///
/// The checkboxes mean "this field MAY take part", which is the user's statement about their
/// strategy. A composition that wrote them back would silently replace that statement with its
/// own every run, and the next all-fields search — a different question — would then be scoped by
/// a set the user never chose. What the composition picked is visible where it belongs: in which
/// fields came back carrying thresholds.
///
/// The wiring can only be pinned at the source level, since the mask lives on a GPUI view no
/// integration test in this binary crate can construct.
///
/// The plausible edit is "the result would be clearer if the boxes reflected the chosen set" —
/// which reads as an improvement, costs the user their own selection, and no runtime test in
/// this crate would notice.
#[test]
fn auto_composition_never_writes_the_field_checkboxes() {
    for file in [
        "analytics/tuner/filter/actions.rs",
        "analytics/tuner/shell.rs",
    ] {
        assert_eq!(
            enabled_mask_writes(&read_src(file)),
            0,
            "{file} must not write the checkbox mask: the composition reports its set through \
             the thresholds it fills in, never by re-ticking the user's boxes"
        );
    }
}

/// The composed set is reported in the same pinned block as the held-back figure, and reaching
/// it must not depend on there being a holdout.
///
/// Two failures this pins, both of which leave a plausible-looking window:
///
/// The first is `split_summary` going back to `let holdout = split.holdout.as_ref()?;`. With the
/// train share at 100% there is no holdout, so that early return would drop the composed set as
/// well — the user asks the tuner to choose the fields, it does, and the window says nothing
/// about what it chose.
///
/// The second is moving the composed line out of this block, into the scrolling grid above.
/// "Four fields chosen" reads as a result on its own; separated from the number that says
/// whether those fields survived a period nobody fitted them on, it is the confident-looking
/// half of a verdict.
#[test]
fn the_composed_set_is_reported_beside_the_holdout() {
    let filter = read_src("analytics/tuner/filter/mod.rs");
    let body = braced_body(&filter, "fn split_summary(");
    assert!(
        !body.contains("split.holdout.as_ref()?"),
        "the composed set must survive a period with nothing held back, so the summary may not \
         return early on a missing holdout"
    );
    for needle in ["split.composed", "split_holdout", "compose_set"] {
        assert!(
            body.contains(needle),
            "`split_summary` must render {needle}: the set and the held-back figure are one \
             verdict and belong in one block"
        );
    }
    let decision_end = body
        .find("let fields: Vec<String>")
        .expect("the decision match must precede composed-field details");
    let decisions = [
        ("ComposeDecision::ReducedSet", "compose_decision_reduced"),
        ("ComposeDecision::AllAllowedFields", "compose_decision_all"),
        (
            "ComposeDecision::NoAdditionalFilters",
            "compose_decision_none",
        ),
    ];
    let positions: Vec<usize> = decisions
        .iter()
        .map(|(decision, _)| {
            body.find(decision)
                .unwrap_or_else(|| panic!("`split_summary` must handle {decision}"))
        })
        .collect();
    for (index, ((decision, text), start)) in decisions.iter().zip(&positions).enumerate() {
        let end = positions.get(index + 1).copied().unwrap_or(decision_end);
        assert!(
            body[*start..end].contains(text),
            "`split_summary` must map {decision} to {text}, or the textual result can describe \
             a different search path than the core selected"
        );
    }
    assert!(
        body.contains("analytics.tuner.compose_result"),
        "the selected path needs its own explicit result line"
    );
    assert!(
        body.contains("split.compose_skipped")
            && body.contains("analytics.tuner.compose_decision_all_direct"),
        "when comparison cannot run, the summary must still name the all-fields path that did"
    );
}

/// Every strategy-list sort click must persist its stable key and direction.
///
/// The plausible regression is retaining `toggle_sort_key` and `cx.notify()` while removing the
/// backend write during a cleanup. Sorting would look correct for the rest of the process, all
/// pure ordering tests would stay green, and the next restart would silently return to Profit.
#[test]
fn strategy_sort_clicks_persist_through_the_layout() {
    let list = read_src("analytics/tuner/list/mod.rs");
    let table = read_src("analytics/tuner/list/table.rs");
    let analytics = read_src("analytics/mod.rs");
    let layout_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("moon-core")
        .join("src")
        .join("config")
        .join("layout.rs");
    let layout = fs::read_to_string(&layout_path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", layout_path.display()));
    let toggle = braced_body(&list, "fn toggle_sort(");

    assert!(
        toggle.contains("toggle_sort_key(")
            && toggle.contains("analytics_strat_sort")
            && toggle.contains("layout_dirty = true"),
        "the one strategy-sort funnel must update view state, save the exact choice, and dirty \
         the layout"
    );
    assert!(
        table.contains("this.toggle_sort(key, cx)"),
        "every sortable strategy header must reach the persisted funnel"
    );
    assert!(
        analytics.contains("restore_strat_sort(") && analytics.contains("analytics_strat_sort"),
        "AnalyticsView must restore the saved sort instead of hard-coding Profit"
    );
    assert!(
        layout.contains("pub analytics_strat_sort: Option<(String, bool)>")
            && layout.contains("deserialize_with = \"de_lenient\""),
        "the layout field must preserve key+direction without making malformed TOML fatal"
    );
}

/// The composition switch persists on every change, like every other search setting.
///
/// Same hazard as the field checkboxes: the window is reopened often, and a mode that silently
/// resets to the cheaper search would have the user believe they composed a set when they ran
/// the all-fields search. Pinned at the source level for the same reason — the handler lives on a
/// GPUI view this crate cannot construct in a test.
#[test]
fn the_compose_switch_persists_every_change() {
    let shell = read_src("analytics/tuner/shell.rs");
    assert!(
        shell.contains("fn persist_tuner_compose("),
        "the composition switch needs a persist wrapper of its own"
    );
    let writes = shell.matches("tuner.compose = ").count();
    assert!(writes > 0, "the switch must be written somewhere");
    assert_eq!(
        shell.matches("persist_tuner_compose(cx)").count(),
        writes,
        "every write of the composition switch must persist it"
    );
}

/// The filter-search popup must present settings as a form, not as one technical text stream.
///
/// Breakage this pins: flattening the composition block back into the generic two-column row.
/// The checkbox would lose MoonUI's own clickable label, the work budget would return to one
/// wrapping sentence, and the three groups would again be visually indistinguishable. Returning
/// the width to 210 or removing the height-bound scroll would make the redesigned form unreadable
/// or unreachable in a narrow group window; omitting either adaptive beam bound or the seed count
/// would hide the real work policy.
#[test]
fn the_filter_search_popup_keeps_its_grouped_composition_block() {
    let shell = read_src("analytics/tuner/shell.rs");
    let content = braced_body(&shell, "fn filter_settings_content(");
    let popover = braced_body(&shell, "fn filter_search_settings(");
    let budget_labels = braced_body(&shell, "fn compose_budget_labels(");

    assert!(
        shell.contains("const SETTINGS_POPUP_W: f32 = 250.0;"),
        "the grouped form needs the reviewed 250-unit content width"
    );
    assert!(
        content.contains(".max_h(popup_max_h)") && content.contains(".overflow_y_scroll()"),
        "all settings must remain reachable when the popup is taller than the viewport"
    );
    assert!(
        content.contains("analytics.tuner.compose_help") && content.contains(".tooltip("),
        "the detailed multi-seed and adaptive-beam explanation must remain attached as a tooltip"
    );
    assert!(
        budget_labels.contains("budget.beam_width_min")
            && budget_labels.contains("budget.beam_width_max")
            && budget_labels.contains("budget.seed_groups")
            && budget_labels.contains("analytics.tuner.compose_budget_beam")
            && budget_labels.contains("analytics.tuner.compose_budget_seeds"),
        "the composition card must display the production adaptive beam and seed budget"
    );

    let sections = [
        "analytics.tuner.cfg_search_section",
        "analytics.tuner.cfg_validation_section",
        "analytics.tuner.cfg_repeat_section",
    ];
    let mut previous = None;
    for section in sections {
        let position = content
            .find(section)
            .unwrap_or_else(|| panic!("the settings popup must render the {section} group"));
        if let Some(previous) = previous {
            assert!(
                position > previous,
                "settings groups must remain in search, validation, reproducibility order"
            );
        }
        previous = Some(position);
    }
    for needle in [
        "analytics.tuner.compose_short",
        "MoonTag::new()",
        "compose_budget_labels(&budget)",
    ] {
        assert!(
            content.contains(needle),
            "the contained composition setting must keep {needle}"
        );
    }
    let checkbox = content
        .find("MoonCheckbox::new(SharedString::from(\"tun-cfg-compose-f\"))")
        .expect("the composition block must use MoonUI's checkbox");
    let handler = content[checkbox..]
        .find(".on_change(")
        .map(|offset| checkbox + offset)
        .expect("the composition checkbox must keep its change handler");
    assert!(
        content[checkbox..handler].contains(".label(")
            && content[checkbox..handler].contains("analytics.tuner.compose_toggle"),
        "the composition checkbox must own its localized clickable label"
    );
    assert!(
        popover.contains("content_width_font(SETTINGS_POPUP_W)")
            && popover.contains("overlay_closable(false)")
            && popover.contains("close_on_content_click(false)"),
        "the wider form must keep font-scaled sizing and nested-dropdown dismissal protection"
    );
}

/// The tuner's expensive settings are hidden, not merely narrowed, on a machine below the bar.
///
/// One predicate — `moon_core`'s `heavy_search_supported` — decides all three: whether the
/// composition switch is rendered, whether a saved composition setting is honoured, and how many
/// depths the dropdown offers. They must not drift apart, because each half of a disagreement is
/// a lie: a switch rendered where the search ignores it silently runs the all-fields search under
/// the composed label, and a setting honoured where no switch exists changes what the run button
/// does with nothing on screen to explain it.
///
/// Pinned at the source level because the switch lives on a GPUI view this crate cannot build,
/// and because a test machine only ever exercises ONE side of the bar.
///
/// The plausible edit: rendering the composition row unconditionally and greying it out instead
/// — a reasonable-looking accessibility instinct that puts the feature back in front of exactly
/// the users it was gated away from.
#[test]
fn the_heavy_search_settings_are_hidden_below_the_core_bar() {
    let shell = read_src("analytics/tuner/shell.rs");
    let state = read_src("analytics/tuner/filter/state.rs");

    let gate = shell
        .find("composition_budget(")
        .expect("the composition row must be gated on a budget this machine actually has");
    let switch = shell
        .find("tun-cfg-compose-f")
        .expect("the composition switch must exist");
    assert!(
        gate < switch,
        "the composition switch must be rendered INSIDE the budget check, not beside it"
    );
    assert!(
        shell.matches("tun-cfg-compose-f").count() == 1,
        "one composition switch only, or a second one could escape the gate"
    );

    assert!(
        state.contains("saved_compose && heavy_search_supported()"),
        "a composition setting saved on a bigger machine must not switch the feature on here"
    );
    // Scoped to the accessor itself: the two names appearing SOMEWHERE in the file would also
    // be satisfied by an `edge_options` that ignored the machine entirely.
    assert!(
        braced_body(&state, "fn edge_options()").contains("edges_max()"),
        "the offered quantile depths must be derived from the search's own ceiling"
    );
    assert!(
        braced_body(&shell, "fn filter_settings_content(").contains("edge_options()"),
        "the depth dropdown must build its items from the machine's own option set"
    );
}

/// The distribution card's collapse is a display lens: it persists and repaints, and it must not
/// gate the read behind it.
///
/// `TunerState::needs_reload` counts `hist_dirty`, so suppressing the histogram read while
/// collapsed would leave that flag permanently set — the reload gate would re-fire every frame,
/// and expanding the card would show a spinner where the user left a chart.
///
/// The plausible edit is "why compute a histogram nobody is looking at?" — skipping
/// `request_hist` on the flag while chasing a performance number.
#[test]
fn the_distribution_card_collapse_is_a_display_lens_only() {
    let hist = read_src("analytics/tuner/filter/hist.rs");
    let analytics = read_src("analytics/mod.rs");

    assert!(
        hist.contains("an-tuner-hist-collapse") && hist.contains("toggle_hist_collapsed(cx)"),
        "the distribution card must carry a caret wired to the toggle"
    );
    // Round-trip, asserted on the two IDENTIFIERS rather than on exact statements: a rustfmt
    // reflow or a renamed local must not redden a guard whose subject is the data flow.
    let toggle = braced_body(&analytics, "fn toggle_hist_collapsed(");
    assert!(
        toggle.contains("analytics_hist_collapsed") && toggle.contains("layout_dirty = true"),
        "toggling must write the layout field and mark it dirty, or the choice dies with the process"
    );
    assert!(
        analytics.contains("saved_hist_collapsed"),
        "the window must seed itself from the persisted flag rather than starting fresh"
    );
    // The read must stay outside the flag. The request and the staleness rule live in
    // `filter/mod.rs` and `filter/state.rs`; neither may learn that the card can be collapsed.
    for (rel, src) in [
        (
            "analytics/tuner/filter/mod.rs",
            read_src("analytics/tuner/filter/mod.rs"),
        ),
        (
            "analytics/tuner/filter/state.rs",
            read_src("analytics/tuner/filter/state.rs"),
        ),
    ] {
        assert!(
            !src.contains("hist_collapsed"),
            "{rel} must not gate the histogram read on the collapse — `needs_reload` counts \
             `hist_dirty`, so a suppressed read leaves the reload gate firing every frame"
        );
    }
}

/// A strategy-row click resolves its gesture through the pure `row_click_intent`, so the
/// Shift-over-Ctrl precedence is decided somewhere a test can reach.
///
/// The plausible edit is inlining the branches back into the closure while adding a fourth
/// gesture; the precedence rule then lives only inside a `'static` GPUI callback, and reversing
/// it — so a Ctrl-built selection cannot be extended with Shift — leaves every test green.
#[test]
fn strategy_row_clicks_route_through_the_pure_gesture_decision() {
    let table = read_src("analytics/tuner/list/table.rs");

    // Asserted on the call and the three arms, not on the local variable names the call reads
    // from: the subject is "the precedence decision lives somewhere a test can reach", and the
    // `shift_takes_precedence_over_the_multi_select_modifier` unit test proves the rule itself.
    assert!(
        table.contains("row_click_intent(") && table.contains(".shift"),
        "the row click must read the Shift modifier and delegate the precedence decision"
    );
    for arm in [
        "RowClick::Range => this.select_range(",
        "RowClick::Multi => this.toggle_multi(",
        "RowClick::Single => this.select_single(",
    ] {
        assert!(
            table.contains(arm),
            "every gesture must reach its transition — missing `{arm}`"
        );
    }
}

/// A tuner card's header clips its own text before it ever pushes the collapse caret out.
///
/// Breakage this pins: removing the clipping box or restoring a second grow spacer in
/// `analytics/tuner/shared.rs:card` would hide the caret in a narrow card.
#[test]
fn a_tuner_card_header_clips_its_text_before_its_accessory() {
    let shared = read_src("analytics/tuner/shared.rs");
    let card = braced_body(&shared, "pub(super) fn card(");

    for needle in [".min_w_0()", ".overflow_hidden()", ".whitespace_nowrap()"] {
        assert!(
            card.contains(needle),
            "the header's text box must carry `{needle}` or it cannot yield width at all"
        );
    }
    assert_eq!(
        card.matches(".flex_1()").count(),
        1,
        "exactly one grow candidate — the text box; a second one stops it eating the middle and \
         the accessory drifts past the card's clip box"
    );
    assert!(
        card.contains(".child(div().flex_none().child(acc))"),
        "the accessory must sit OUTSIDE the clipping box and never shrink"
    );
    assert_eq!(
        card.matches(".flex_none()").count(),
        4,
        "exactly four pinned elements: the card root (it must not shrink inside a scrolling \
         column), the title, the subtitle, and the accessory — the two texts so the box clips \
         them by order rather than shrinking both, the accessory so it never yields"
    );
    assert!(
        !card.contains(".flex_wrap()"),
        "the header is a fixed-height strip — wrapping would grow it and push the body down"
    );
}

/// A tuner shell toolbar's title clips itself before it ever pushes the trailing buttons out.
///
/// Breakage this pins: removing the title's clipping styles or restoring the trailing spacer in
/// `analytics/tuner/shell.rs:shell_toolbar` would hide Copy/Save in a narrow card.
#[test]
fn a_tuner_shell_title_clips_itself_before_its_trailing_buttons() {
    let shell = read_src("analytics/tuner/shell.rs");
    let toolbar = braced_body(&shell, "pub(super) fn shell_toolbar(");

    for needle in [".min_w_0()", ".overflow_hidden()", ".whitespace_nowrap()"] {
        assert!(
            toolbar.contains(needle),
            "the toolbar's title box must carry `{needle}` or it cannot yield width at all"
        );
    }
    assert_eq!(
        toolbar.matches(".flex_1()").count(),
        1,
        "exactly one grow candidate — the title itself; a trailing spacer would take the grow \
         away from the title and let it push the Copy/Save buttons past the card's clip edge"
    );
    assert!(
        !toolbar.contains(".child(div().flex_1());"),
        "the old trailing spacer must not return after the title"
    );
}

/// Deleting one strategy from Analytics must never stop the whole core's trading.
///
/// `apply_strategies`' third argument is a CORE-WIDE start/stop, not a per-strategy switch. The
/// plausible edit is "unifying" this call with the Strategies window's `apply_start_stop`, which
/// legitimately passes `Some(false)` because there the user asked to stop the checked strategies.
/// Here that would stop every strategy on the core.
#[test]
fn strategy_purge_never_stops_the_whole_core() {
    let purge = read_src("analytics/purge.rs");
    let body = code_only(braced_body(&purge, "async fn run("));

    assert!(
        body.contains("apply_strategies(core_uid, vec![(sid, false)], None)"),
        "the disable step must pass `None` as start_stop"
    );
    // Scoped to the call itself, not to the whole body: `Some(true)` / `Some(false)` legitimately
    // appear when the sequence matches on an `Option`, and a bare substring ban would fail on
    // those while proving nothing about the argument that matters.
    assert_eq!(
        body.matches("apply_strategies(").count(),
        1,
        "one disable call — a second one would escape the assertion above"
    );
}

/// The report hide, disable, and strategy delete run in the order the confirmation promises.
///
/// The plausible edit is firing the three confirmed actions together while simplifying the waits —
/// which reads as a harmless cleanup and is not: the delete command is High priority while the
/// other two are Sliced, so it reaches the core FIRST and the strategy is deleted while still
/// enabled and still holding its trades.
#[test]
fn strategy_purge_hides_trades_before_disabling_and_deleting() {
    let purge = read_src("analytics/purge.rs");
    let body = code_only(braced_body(&purge, "async fn run("));
    let at = |needle: &str| {
        body.find(needle)
            .unwrap_or_else(|| panic!("{needle} must be part of the purge sequence"))
    };

    // Asserted on the step names and the commands, never on the shape of the code around them: a
    // later refactor of the sequence's plumbing must not redden a test about its ORDER.
    assert!(
        at("PurgeStep::Rows") < at("PurgeStep::Disable")
            && at("PurgeStep::Disable") < at("PurgeStep::Delete"),
        "trades are hidden, then the strategy is disabled, then it is deleted"
    );
    assert!(
        at("apply_strategies") < at("delete_strategy"),
        "the delete may not be issued before the disable"
    );
    assert!(
        body.contains("await_strategy("),
        "the confirmable strategy actions must be waited for; a fire-and-forget sequence loses \
         its order on the wire because the delete travels at a higher priority than the other two"
    );
}

/// `analytics/purge.rs:PurgeRun::run`: moving the folder decision before the disappearance wait or
/// disconnecting it from `remaining` could send a folder-wide delete from stale UI evidence.
///
/// The pre-delete placement snapshot must also feed the conditional strategy command; otherwise a
/// queued move can change which folder is emptied before the target is deleted.
#[test]
fn strategy_purge_requests_empty_folder_only_after_strategy_disappearance() {
    let purge = read_src("analytics/purge.rs");
    let body = code_only(braced_body(&purge, "async fn run("));
    let delete_strategy = body
        .find("session.delete_strategy_if_unchanged(core_uid, sid, before_delete)")
        .expect("the purge must guard deletion with its pre-delete placement snapshot");
    let final_delete = &body[delete_strategy..];
    let wait_for_absence = final_delete
        .find("strategy.is_none()")
        .expect("the purge must wait until the strategy disappears");
    let fresh_snapshot = final_delete
        .find("let cleanup = self.inspect_strategies(cx, PurgeStep::Delete, |remaining|")
        .expect("the purge must inspect fresh placements after strategy disappearance");
    let empty_folder_decision = final_delete
        .find("deletable_folder_after(deleted_folder.as_deref(), remaining)")
        .expect("the empty-folder decision must consume the captured folder and fresh rows");
    let placement_projection = final_delete
        .find("(strategy.id, strategy.folder_path.clone())")
        .expect("the fresh rows must produce the feed-thread placement guard");
    let cleanup_binding = final_delete
        .find("if let Some((folder, expected_placements)) = cleanup")
        .expect("only a successful empty-folder decision may expose the request values");
    let delete_folder = final_delete
        .find("self.send_empty_folder(cx, folder, expected_placements)")
        .expect("the conditional request must consume the fresh decision values");

    assert!(
        wait_for_absence < fresh_snapshot
            && fresh_snapshot < empty_folder_decision
            && empty_folder_decision < placement_projection
            && placement_projection < cleanup_binding
            && cleanup_binding < delete_folder,
        "strategy disappearance must precede the fresh placement read, empty-folder decision, \
         and conditional folder command"
    );
}

/// `analytics/purge.rs:AnalyticsView::confirm_purge`: removing the refresh calls from the
/// `FolderSend` outcome would leave already-deleted trades and strategy visible after only the
/// optional folder request failed.
#[test]
fn strategy_purge_refreshes_after_an_empty_folder_queue_failure() {
    let purge = read_src("analytics/purge.rs");
    let body = code_only(braced_body(&purge, "fn confirm_purge("));
    let start = body
        .find("Err(PurgeStop::FolderSend)")
        .expect("folder queue failure must have a distinct outcome");
    let end = body[start..]
        .find("Err(PurgeStop::Failed")
        .map(|offset| start + offset)
        .expect("the ordinary failure arm must follow the folder failure arm");
    let arm = &body[start..end];

    assert!(
        arm.contains("this.mark_report_data_stale()")
            && arm.contains("this.request_report_refresh(false, cx)"),
        "a folder queue failure still needs the successful purge's Analytics refresh"
    );
}

/// A row step confirms the ids IT sent, never a globally empty re-read.
///
/// The strategy is still live during the first pass, so trades keep closing into it while the
/// batch is in flight. Waiting for the whole re-read to come back empty would therefore time out
/// on rows the step never sent — a correct purge reporting a false failure on exactly the busy
/// strategies the user most wants gone. The outer `PURGE_PASSES` loop is what picks those up.
///
/// The plausible edit is "the step is done when there is nothing left", which reads as the
/// stronger condition and is unreachable on a trading core.
///
/// Pinned at the source level: the wait is an `async` method on a private struct driven by an
/// `AsyncApp` and a live core session, and this binary crate exposes no library target a test
/// could construct one from.
#[test]
fn a_row_step_waits_only_for_the_ids_it_sent() {
    let purge = read_src("analytics/purge.rs");
    let wait = code_only(braced_body(&purge, "async fn await_rows_gone("));
    let pass = code_only(braced_body(&purge, "async fn purge_rows("));

    assert!(
        wait.contains("sent.contains("),
        "the wait must test the re-read against the ids the pass actually sent"
    );
    assert!(
        !wait.contains(".is_empty()"),
        "waiting for a globally empty re-read never completes on a strategy that is still \
         closing trades"
    );
    assert!(
        pass.contains("await_rows_gone(cx, step, &sent"),
        "each pass must hand its own sent set to the wait, not a shared or recomputed one"
    );
    assert!(
        pass.contains("for pass in 0..PURGE_PASSES"),
        "the rows that arrive DURING a pass are picked up by the bounded outer loop, which is \
         what makes the per-batch wait sufficient"
    );
}

/// The row menu and its confirmation belong to MoonUI's Root, not to a panel-owned overlay.
///
/// The plausible edit is hand-rolling an absolutely positioned menu or modal while chasing a
/// z-order bug, which is exactly what the Root ownership rule exists to prevent.
#[test]
fn the_strategy_row_menu_and_dialog_go_through_moonui_root() {
    let menu = read_src("analytics/tuner/list/menu.rs");
    let purge = read_src("analytics/purge.rs");

    assert!(
        menu.contains("window.open_moon_context_menu("),
        "the row menu opens through MoonUI's context-menu extension"
    );
    assert!(
        purge.contains("window.open_unique_moon_dialog("),
        "the confirmation opens as a unique MoonUI dialog"
    );
    for (name, source) in [("menu.rs", &menu), ("purge.rs", &purge)] {
        assert!(
            !source.contains(".absolute()"),
            "{name} must not position its own overlay"
        );
    }
}

/// Right-clicking a row opens a menu; it must not also re-scope the whole tuner.
///
/// The plausible edit is copying the left-click handler's dispatch into the right-click one so
/// "the row you act on is the row that is selected" — which reloads the suggestion scope, the
/// time grid and the coin tables as a side effect of opening a menu.
#[test]
fn right_clicking_a_strategy_row_does_not_move_the_selection() {
    let table = read_src("analytics/tuner/list/table.rs");
    let handler = braced_body(&table, "on_mouse_down(MouseButton::Right,");
    let handler = code_only(handler);

    assert!(
        handler.contains("open_strategy_row_menu("),
        "the right button opens the row menu"
    );
    for banned in [
        "select_single(",
        "select_range(",
        "toggle_multi(",
        "select_for_report(",
    ] {
        assert!(
            !handler.contains(banned),
            "{banned} must not run from the right-click handler"
        );
    }
}
