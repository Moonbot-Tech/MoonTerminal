//! Strategies-window contracts for folder paths, copy/reveal behavior, refresh routing, MoonTree
//! ownership, and per-frame tree construction.

use super::support::*;

/// A strategy copy goes to the core root, sits beside its source, and is revealed to the user.
///
/// Plausible edits this catches: inheriting `row.folder_path` lets the receiving core reinterpret
/// the flat path described by `strategies::tree::ops::path_segments`; dropping the reveal hides
/// the copy from an open Strategies window, while dropping the notification leaves its destination
/// unstated in this trade-derived view.
#[test]
fn analytics_copy_places_and_reveals_the_new_strategy() {
    let save = read_src("analytics/tuner/save.rs");
    // Scoped to the one function: whole-file greps would let an edit move the anchor while an
    // unrelated match elsewhere in the file kept this green.
    let body = braced_body(&save, "fn create_strategy_copy(");
    assert!(
        body.contains("insert_after: Some((cid, row.id))"),
        "the copy must be placed after the strategy it was copied from"
    );
    assert!(
        body.contains("folder_path: String::new()") && !body.contains("folder_path: row."),
        "the copy must be sent to the core root, never inheriting its source's folder"
    );
    assert!(
        body.contains("crate::strategies::reveal_name(")
            && body.contains("\"analytics.copy_created_root\""),
        "the copy must be revealed in the Strategies window and named where it went"
    );
}

/// A `folder_path` is split only by `strategies::tree::ops::path_segments`.
///
/// Plausible edit this catches: a second hand-written split can disagree with the owning helper
/// and address a folder the tree does not show.
///
/// The scan covers the whole crate and flags the obvious same-line `folder_path` plus `.split`
/// form. It skips test modules, where spelling a deliberately invalid split is legitimate.
#[test]
fn folder_paths_are_split_only_by_path_segments() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut sources = Vec::new();
    rust_sources(&root, &mut sources);

    let owner = Path::new("strategies").join("tree").join("ops.rs");
    let mut violations = Vec::new();
    for path in sources {
        if path.ends_with(&owner) || path.ends_with("tests.rs") {
            continue;
        }
        let text = fs::read_to_string(&path)
            .unwrap_or_else(|err| panic!("failed to read {}: {err}", path.display()));
        for (line_ix, line) in text.lines().enumerate() {
            if line.contains("folder_path") && line.contains(".split") {
                violations.push(format!(
                    "{}:{}: {}",
                    path.display(),
                    line_ix + 1,
                    line.trim()
                ));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "a folder path must be split through tree::ops::path_segments, never by hand:\n{}",
        violations.join("\n")
    );
}

/// A revealed strategy is expanded to AND scrolled to, not merely selected.
///
/// The plausible edit is dropping the `pending_scroll` hand-off as redundant "because the row
/// is already selected". Selection paints a highlight; only `scroll_to_item` brings the row on
/// screen, and on a core with a hundred strategies a selected row the user cannot see reads
/// exactly like a copy that was saved somewhere else.
#[test]
fn a_revealed_strategy_is_expanded_and_scrolled_into_view() {
    let selection = read_src("strategies/selection.rs");
    let window = read_src("strategies/mod.rs");
    let sync = braced_body(&selection, "fn sync_pending_select(");
    assert!(
        sync.contains("expand_path(") && sync.contains("pending_scroll = Some(key)"),
        "an echoed-back strategy must have its folder chain expanded and be queued for scroll"
    );
    assert!(
        window.contains("pending_scroll.take()"),
        "render must drain the queued scroll alongside a direct goto"
    );
    // The paste-target precedence rests on a strategy selection retiring any folder selection.
    // One setter makes that structural instead of four copies a fifth site could omit.
    let focus = braced_body(&selection, "fn focus_strategy(");
    assert!(
        focus.contains("self.selected_folder = None;"),
        "focusing a strategy must retire the folder selection"
    );
}

/// `startup.rs` must feed both report commit classes through the dedicated revision gate rather
/// than the global Backend; adding `mark_backend_dirty` repaints every shell for each report
/// burst, while bypassing the gate makes historical catch-up rescan Report and Analytics every
/// five to ten seconds.
#[test]
fn report_commits_use_a_dedicated_revision_channel() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let startup = fs::read_to_string(root.join("startup.rs")).unwrap();
    let analytics = fs::read_to_string(root.join("analytics").join("mod.rs")).unwrap();
    let report = fs::read_to_string(root.join("panels").join("report").join("state.rs")).unwrap();

    assert!(
        startup.contains("coord_report_revision.update(cx, |_, cx| cx.notify());")
            && analytics.contains("cx.observe(&report_revision")
            && report.contains("cx.observe(&report_revision"),
        "report-derived consumers must observe only the dedicated revision entity"
    );
    let commit_block = startup
        .split("consume_report_commit(coord_report_immediate_dirty.as_deref()")
        .nth(1)
        .expect("startup must consume the immediate report edge");
    let commit_block = commit_block
        .split("b.maybe_diag_open_first_market")
        .next()
        .unwrap();
    assert!(
        commit_block.contains("consume_report_commit(coord_report_background_dirty.as_deref()")
            && commit_block.contains("let revision = report_revision_gate.observe("),
        "both report commit classes must enter the shared revision gate"
    );
    assert!(
        !commit_block.contains("mark_backend_dirty"),
        "report commits must not notify the global Backend entity"
    );
}

/// `analytics/mod.rs:refresh_visible_report_data` must route a stale Strategies
/// base through `strategy_base_data`; replacing it with `reload_summary` incurs
/// the full charts/rankings scan on every high-rate report refresh.
#[test]
fn strategies_refresh_uses_the_compact_base() {
    let analytics = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src")
            .join("analytics")
            .join("mod.rs"),
    )
    .unwrap();
    assert!(
        analytics.contains("self.reload_strategy_base(true, true, show_overlay, cx)")
            && analytics.contains("moon_core::db::analytics::strategy_base_data(&q, read_cores)"),
        "Strategies refresh must not pay for the full Summary surface"
    );
}

/// A closure handed to `MoonTree` must never capture a STRONG handle to the view that owns the
/// tree state.
///
/// `MoonTree` moves both the row renderer and the row decorators into the long-lived
/// `MoonTreeState` on every frame (MoonUI `tree.rs`, `impl RenderOnce for Tree`). When that state
/// is a field of the same view, a strong capture closes the cycle
/// `View -> tree_state -> closure -> View`: the view never drops, so its `on_release` never runs
/// and its subscriptions are never released.
///
/// The same ownership cycle prevents a Settings view from clearing its draft in `on_release`.
/// `strategies/tree/moon.rs` has no reopen gate, so the corresponding cycle would instead leak a
/// view and its subscriptions on every open/close cycle.
///
/// The plausible edit this catches: someone reads the `upgrade()` dance as ceremony and
/// "simplifies" it back to `cx.entity()`, which compiles and behaves correctly for one session.
#[test]
fn moon_tree_closures_hold_weak_view_handles() {
    let text = read_src("strategies/tree/moon.rs");

    assert!(
        text.contains("cx.entity().downgrade()"),
        "tree_moon.rs must downgrade the view handle before handing closures to MoonTree"
    );
    assert!(
        !text.contains("let view = cx.entity();"),
        "tree_moon.rs captures a STRONG view handle: MoonTree retains its renderer and decorators \
         in MoonTreeState, which this view owns, so the view can never drop"
    );
}

/// The window pushes its forest into `MoonTreeState` only when the tree shape changes.
///
/// Each push costs two full deep clones of the item forest inside MoonUI (`set_items` and
/// `set_expanded` each rebuild the flattened entries, and `TreeItem::clone` is recursive), and
/// this window renders on every keystroke in a parameter field, every hover, and every scroll.
///
/// The plausible edit this catches: moving one of the three calls out of the guard "to be safe"
/// silently adds per-frame deep clones, with no visible symptom in a small account and severe input
/// lag in a large account.
#[test]
fn the_tree_is_pushed_into_moon_tree_state_only_when_its_shape_changed() {
    let src = read_src("strategies/mod.rs");
    let render = braced_body(&src, "fn render(&mut self, window: &mut Window");

    assert!(
        render.contains("tree::moon::shape_sig(") && render.contains("last_tree_shape = Some("),
        "the push must be gated on the tree shape signature, which is then remembered"
    );
    // Brace-balanced, not positional: "appears after the guard starts" would also hold for a call
    // moved just AFTER the block, which is exactly the regression this test exists to catch.
    let guard = braced_body(render, "if self.last_tree_shape != Some(shape)");
    for call in ["set_items(", "set_force_expanded(", "set_expanded("] {
        assert_eq!(
            render.matches(call).count(),
            1,
            "{call} must appear exactly once in the render root"
        );
        assert!(
            guard.contains(call),
            "{call} must sit INSIDE the shape guard, not beside it"
        );
    }
}

/// Navigating to a row must not depend on whether this frame pushed the forest.
///
/// A reveal expands its target's core and folder chain in the WINDOW's own expansion state before
/// render runs, so the row is always in the forest MoonTree holds — with or without a push.
///
/// The plausible edit this catches: moving `scroll_to_item` inside the shape guard makes
/// "reveal in Strategies" silently do nothing whenever the tree happens to be unchanged, which is
/// the common case for a second reveal of a row in an already-open core.
#[test]
fn navigation_is_independent_of_the_shape_guard() {
    let src = read_src("strategies/mod.rs");
    let render = braced_body(&src, "fn render(&mut self, window: &mut Window");
    assert!(
        render.contains("scroll_to_item"),
        "render must scroll to the navigation target"
    );
    let guard = braced_body(render, "if self.last_tree_shape != Some(shape)");
    assert!(
        !guard.contains("scroll_to_item"),
        "the scroll must sit OUTSIDE the shape guard, or a reveal onto an unchanged tree does \
         nothing"
    );

    // The invariant the scroll rests on: both goto producers open the target's chain themselves.
    let selection = read_src("strategies/selection.rs");
    for producer in ["fn drain_goto(", "fn sync_pending_select("] {
        let body = braced_body(&selection, producer);
        assert!(
            body.contains("expanded_cores.insert(core)") && body.contains("expand_path("),
            "{producer} must expand its target's core and folder chain, or the row is not in \
             the forest when render tries to scroll to it"
        );
    }
}

/// The per-frame row pass runs on the prepared filter, never on the stored one.
///
/// `StrategyFilter::matches` prepares and lowercases the search text for a single-row query. The
/// tree instead prepares once before walking strategies so its query normalization cost is
/// independent of row count.
///
/// The plausible edit this catches: calling `view.filter.matches` inside the loop compiles and
/// behaves identically but adds one query allocation per row per frame.
#[test]
fn the_per_frame_row_pass_uses_the_prepared_filter() {
    let src = read_src("strategies/tree/moon.rs");
    let body = braced_body(&src, "pub(crate) fn build(");
    assert!(
        body.contains("view.filter.prepare()"),
        "build must prepare the filter once per frame"
    );
    assert!(
        !body.contains("view.filter.matches(") && !body.contains("view.filter.counts("),
        "build must not evaluate the stored filter per row"
    );
}

/// MoonTree's keyboard expansion is reconciled with the window-owned expansion state.
///
/// `MoonTreeState` expands and collapses entries from keyboard input without updating
/// `expanded_cores` or `expanded_folders`, which remain authoritative. The event subscription
/// invalidates the shape cache so the next frame reasserts the window-owned state.
///
/// The plausible edit this catches: dropping the subscription as redundant leaves the tree and the
/// window disagreeing about which nodes are open, and under the collapsed-subtree pruning in
/// `tree::moon::build` a keyboard-expanded node would have no children to show at all.
#[test]
fn tree_state_events_reassert_the_windows_own_expansion() {
    let src = read_src("strategies/state.rs");
    let body = braced_body(&src, "pub(super) fn new(");
    assert!(
        body.contains("cx.subscribe(&tree_state")
            && body.contains("MoonTreeEvent")
            && body.contains("this.last_tree_shape = None;"),
        "a MoonTreeEvent subscription must drop the cached shape so the next frame re-pushes"
    );
    // An undetached `Subscription` is dropped where it is built, tearing the subscription down at
    // construction — every grep above still matches while the mechanism is silently dead.
    // Bounded by the next statement in `new`, not by the first `;` — the closure body has its own.
    let after = body
        .split_once("cx.subscribe(&tree_state")
        .expect("the tree-state subscription must exist")
        .1;
    let sub = after.split("cx.observe(").next().unwrap_or(after);
    assert!(
        sub.contains(".detach()"),
        "the subscription must be detached, or it is dropped before it can ever fire"
    );
}

/// A collapsed core builds no subtree — but still renders its own row, and a search still prunes
/// nothing.
///
/// Search force-expands every node, so pruning while searching would make search stop finding rows
/// inside collapsed cores. And the subtree is the ONLY thing skipped: dropping the core's own row
/// would make every collapsed core vanish from the tree.
///
/// The plausible edits this catches: dropping the `searching` disjunct from the open test, or
/// moving the core row inside the `core_open` branch.
#[test]
fn a_collapsed_core_skips_its_subtree_but_keeps_its_row() {
    let src = read_src("strategies/tree/moon.rs");
    let body = braced_body(&src, "pub(crate) fn build(");
    assert!(
        body.contains("searching || view.expanded_cores.contains(&core)"),
        "an open core must include the search-forced case"
    );
    let call_at = body
        .find("build_core_subtree(")
        .expect("the subtree builder must be called");
    // Brace-balanced from the guard nearest the call, so "inside the branch" and "just after it"
    // cannot be confused — the second is what would make every collapsed core disappear.
    let guard_at = body[..call_at]
        .rfind("if core_open {")
        .expect("the subtree must be built only for an open core");
    let open_only = braced_body(&body[guard_at..], "if core_open {");
    assert!(
        open_only.contains("build_core_subtree("),
        "the subtree must be built inside the open-core branch"
    );
    assert!(
        !open_only.contains("NodeData::Core") && !open_only.contains("items.push("),
        "the core's own row must be emitted OUTSIDE that branch, or every collapsed core vanishes"
    );
    assert!(
        body.contains("NodeData::Core") && body.contains("items.push("),
        "every listed core must emit its own row, pruned or not"
    );
}
