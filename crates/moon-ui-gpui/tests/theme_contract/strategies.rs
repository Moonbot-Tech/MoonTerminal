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
    let startup = read_startup();
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

/// Strategies owns a persisted `active_only` filter edited from the search row, but the retired
/// toolbar/parameter symbols and active-dependent parameter skipping must stay gone. The current
/// toggle is built in `settings.rs` and writes through `write_pref`; reviving a retired symbol
/// would give one preference a second, unpersisted authority or hide dependency-inactive fields
/// again.
#[test]
fn persisted_active_filter_does_not_restore_retired_controls_or_hide_params() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("strategies");
    let mut sources = Vec::new();
    rust_sources(&root, &mut sources);

    let forbidden = [
        ["only", "active"].join("_"),
        ["only", "active", "params"].join("_"),
        ["flt", "active"].join("-"),
        ["params", "only", "active"].join("-"),
    ];
    let mut leftovers = Vec::new();
    for path in sources {
        let text = fs::read_to_string(&path)
            .unwrap_or_else(|err| panic!("failed to read {}: {err}", path.display()));
        for forbidden in &forbidden {
            if text.contains(forbidden) {
                leftovers.push(format!("{} contains {forbidden}", path.display()));
            }
        }
    }
    assert!(
        leftovers.is_empty(),
        "retired active-only toolbar and parameter controls must stay decommissioned:\n{}",
        leftovers.join("\n")
    );

    let tree = read_src("strategies/tree/mod.rs");
    let tree_panel = code_only(braced_body(&tree, "pub(super) fn tree_panel("));
    let toggle = code_only(braced_body(
        &read_src("strategies/settings.rs"),
        "pub(super) fn active_only_toggle(",
    ));
    assert!(
        tree_panel.contains("self.active_only_toggle("),
        "the pane must render the active-only toggle it delegates to settings.rs"
    );
    // Twice: the label and the mark are separate hit targets, and a half that stopped writing
    // through the setter would look alive while persisting nothing.
    assert_eq!(
        toggle.matches("set_active_only(").count(),
        2,
        "both halves of the filter-row toggle must write through the persisted setter"
    );
    assert!(
        !toggle.contains("prefs.active_only ="),
        "the toggle must never assign the resolved preference directly"
    );

    let params = read_src("strategies/params.rs");
    let params_panel = code_only(braced_body(&params, "pub(super) fn params_panel("));
    let active_at = params_panel
        .find("let active = self.rules.field_active")
        .expect("the parameter pane must still compute dependency activity");
    let field_row_at = params_panel[active_at..]
        .find("self.field_row(")
        .map(|offset| active_at + offset)
        .expect("the dependency activity must still reach field_row");
    assert!(
        !params_panel[active_at..field_row_at].contains("continue;"),
        "dependency-inactive fields must be rendered and disabled by field_row, never skipped"
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
    // Dropping the shape alone is not enough since the adapter became cacheable: a keyboard
    // expansion moves nothing `tree::cache::data_sig` hashes, so a surviving entry would let the
    // next frame skip the push entirely and leave the tree where the keyboard put it.
    assert!(
        body.contains("this.tree_cache = None;"),
        "the same event must drop the adapter cache, or the re-push it schedules never happens"
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
    let body = braced_body(&src, "fn build_core_root(");
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
        !open_only.contains("NodeData::Core") && !open_only.contains("MoonTreeItem::new(cid"),
        "the core's own row must be emitted OUTSIDE that branch, or every collapsed core vanishes"
    );
    assert!(
        body.contains("NodeData::Core") && body.contains("MoonTreeItem::new(cid"),
        "every listed core must emit its own row, pruned or not"
    );
}

/// Grouping ON must preserve canonical venue headings, while grouping OFF emits the same canonical
/// core roots directly. Reusing reported captions would split one venue, and retaining an exchange
/// wrapper in the flat branch would leave grouping visually enabled after the checkbox is cleared.
#[test]
fn strategy_tree_groups_visible_cores_by_venue_identity() {
    let window = read_src("strategies/mod.rs");
    let render = code_only(braced_body(
        &window,
        "fn render(&mut self, window: &mut Window",
    ));
    assert!(render.contains("session.core_venues()"));
    assert!(
        render.contains("tree::cache::data_sig(self, store, &cores, venues)")
            && render.contains("tree::moon::build(self, store, &cores, venues)"),
        "render must pass the same canonical venue map into cache invalidation and tree building"
    );

    let tree = read_src("strategies/tree/moon.rs");
    let build = code_only(braced_body(&tree, "pub(crate) fn build("));
    let grouped = braced_body(&build, "if view.prefs.group_by_venue {");
    assert!(grouped.contains("crate::core_order::exchange_sections("));
    assert!(grouped.contains("crate::controls::venue_section_label("));
    assert!(grouped.contains("section_children.is_empty()"));
    assert!(
        grouped.contains(".folder(false)") && grouped.contains(".disabled(true)"),
        "exchange headings must expose children without becoming selectable/collapsible folders"
    );
    assert!(
        build.contains("} else {")
            && build.contains("for (core, core_name) in cores.iter()")
            && build.contains("items.push(root)"),
        "grouping OFF must emit canonical core roots directly without an Exchange wrapper"
    );
    assert!(!grouped.contains("items.push(root)"));
    assert!(!build.contains(".reported ==") && !build.contains(".reported.cmp("));
    let id = code_only(braced_body(&tree, "fn id_exchange("));
    assert!(id.contains("venue.id.code") && id.contains("venue.id.dex"));
    assert!(id.contains("x:unknown"));
    let drop_dest = code_only(braced_body(&tree, "fn drop_dest("));
    assert!(drop_dest.contains("NodeData::Exchange") && drop_dest.contains("=> None"));
    let row = code_only(braced_body(&tree, "fn render_row("));
    assert!(row.contains("NodeData::Exchange") && row.contains("exchange_row("));

    let cache = read_src("strategies/tree/cache.rs");
    let sig = code_only(braced_body(&cache, "pub(crate) fn data_sig("));
    assert!(sig.contains("venues_digest("));
}

/// Raw shortcuts belong only to the focused Strategies tree, and both keyboard and footer Copy
/// must prefer the last clicked folder/core over a stale strategy selection.
#[test]
fn strategy_tree_shortcuts_require_exact_focus_and_share_copy_dispatch() {
    let window = read_src("strategies/mod.rs");
    assert_eq!(window.matches(".on_key_down(").count(), 1);

    let ui = read_src("strategies/tree/ui.rs");
    let keys = code_only(braced_body(&ui, "pub(crate) fn handle_tree_key("));
    let guard = keys
        .find("!self.focus.is_focused(window)")
        .expect("the tree shortcut handler must guard exact focus");
    let dispatch = keys
        .find("copy_tree_target(")
        .expect("Ctrl+C must use the shared folder-first dispatcher");
    assert!(guard < dispatch);
    let copy = code_only(braced_body(&ui, "fn copy_tree_target("));
    assert!(copy.find("selected_folder(self)") < copy.find("copy_selection(cx)"));
    let toolbar = code_only(braced_body(&ui, "pub(super) fn selection_toolbar("));
    assert!(toolbar.contains("copy_tree_target(") && toolbar.contains("disabled(!can_copy)"));
    assert!(toolbar.contains("default_target(") && toolbar.contains("paste_into("));

    let tree = read_src("strategies/tree/moon.rs");
    let core_folder = code_only(braced_body(&tree, "fn core_folder_row("));
    assert!(core_folder.contains("window.focus(&this.focus, cx)"));
    assert!(core_folder.contains("Some((*c, String::new()))"));
    let strategy = code_only(braced_body(&tree, "fn strategy_row("));
    assert!(strategy.contains("window.focus(&this.focus, cx)"));
}

/// A selected folder must keep its amber surface under the pointer; restoring an unconditional
/// neutral hover makes the row appear unselected until the pointer leaves.
#[test]
fn selected_folder_hover_does_not_replace_its_surface() {
    let tree = read_src("strategies/tree/moon.rs");
    let row = code_only(braced_body(&tree, "fn core_folder_row("));
    assert!(row.contains(".when(selected,"));
    assert!(
        row.contains(".when(!selected,") && row.contains("s.hover("),
        "neutral hover must be installed only for an unselected folder row"
    );
    assert!(!row.contains(".when(selected, |s| s.bg(") || !row.contains("\n.hover("));
}

/// The search row must end with the active-only toggle and the gear pinned right of a flexible
/// input, and the filter row below it must wrap fitted triggers while keeping the expand/collapse
/// caret in an atomic right-aligned cluster. Restoring fixed trigger widths, letting the cluster
/// split, or giving the search input a fixed width recreates horizontal overflow in a narrow
/// Strategies pane.
#[test]
fn strategy_filter_rows_wrap_fitted_controls_and_pin_their_right_hand_controls() {
    let tree = read_src("strategies/tree/mod.rs");
    let panel = code_only(braced_body(&tree, "pub(super) fn tree_panel("));
    assert_eq!(
        panel.matches(".flex_wrap()").count(),
        2,
        "both the search row and the filter row below it must wrap"
    );
    let input_at = panel
        .find("MoonInput::new(\"strat-search\")")
        .expect("the search row must host the search input");
    let cluster_at = panel
        .find(".ml_auto()")
        .map(|at| at + ".ml_auto()".len())
        .expect("the caret cluster must be right-aligned");
    // The input yields its width to the two controls beside it instead of pushing them out of the
    // pane, which a `w_full` input did — but keeps a floor, so the row wraps before the field is
    // squeezed to nothing.
    let slot = &panel[..input_at];
    assert!(
        slot.contains(".flex_1()") && slot.contains("min_w(design::ui_px(cx, SEARCH_MIN_W))"),
        "the search input must sit in a flexible slot with a scaled minimum width"
    );
    let toggle_at = panel
        .find("self.active_only_toggle(")
        .expect("the active-only toggle must sit in the search row");
    let gear_at = panel
        .find(".child(settings)")
        .expect("the settings popover must sit in the search row");
    assert!(
        input_at < toggle_at && toggle_at < gear_at && gear_at < cluster_at,
        "search row order must read input -> active-only -> gear, all above the filter row"
    );
    let cluster = &panel[cluster_at..];
    assert!(cluster.contains(".flex_none()"));
    assert!(
        cluster.contains("MoonButton::new(\"expand-all\")"),
        "the expand/collapse caret must be inside the atomic cluster"
    );

    let kind = braced_body(&tree, "fn combo_kind(");
    let direction = braced_body(&tree, "fn combo_dir(");
    let ui = read_src("strategies/tree/ui.rs");
    let create = braced_body(&ui, "pub(super) fn create_dropdown(");
    for (body, bounds) in [
        (kind, ".fit_trigger_width(96.0, 116.0)"),
        (direction, ".fit_trigger_width(96.0, 128.0)"),
        (create, ".fit_trigger_width(96.0, 110.0)"),
    ] {
        assert!(
            body.contains(bounds),
            "missing fitted trigger bounds {bounds}"
        );
        assert!(!body.contains(".trigger_width_scaled("));
    }
}

/// Strategies settings must own exactly two optional layout preferences and one dirty/write path;
/// bypassing `write_pref` would make popup edits or explicit reveals diverge across restart.
#[test]
fn strategies_settings_own_restore_persistence_and_reveal_visibility() {
    let settings = read_src("strategies/settings.rs");
    let state = read_src("strategies/state.rs");
    let layout = read_src("../../moon-core/src/config/layout.rs");
    let selection = read_src("strategies/selection.rs");
    let dialogs = read_src("strategies/tree/dialogs.rs");
    let dnd = read_src("strategies/tree/dnd.rs");
    let locales = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("locales")
            .join("strategies.yml"),
    )
    .expect("read Strategies locales");

    for id in ["group-by-venue", "active-only"] {
        assert!(
            settings.contains(&format!("id: \"{id}\"")),
            "missing row id {id}"
        );
    }
    for key in [
        "strat.settings.title",
        "strat.settings.display",
        "strat.settings.group_by_venue",
        // Still consumed after active-only left the popup: it is the toggle's tooltip, the only
        // place the preference still spells itself out in full.
        "strat.settings.active_only",
        "strat.active_only",
    ] {
        assert!(settings.contains(key), "settings UI does not consume {key}");
        assert!(
            locales.contains(&format!("{key}:")),
            "locales do not define {key}"
        );
    }
    let src_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut production_sources = Vec::new();
    rust_sources(&src_root, &mut production_sources);
    for field in ["strategies_group_by_venue", "strategies_active_only"] {
        let declaration = format!("pub {field}: Option<bool>");
        assert!(
            layout.contains(&declaration),
            "{field} must remain an optional layout preference"
        );
        let at = layout
            .find(&declaration)
            .unwrap_or_else(|| panic!("missing {field} declaration"));
        let attrs = &layout[at.saturating_sub(120)..at];
        assert!(
            attrs.contains("#[serde(default, deserialize_with = \"de_lenient\")]"),
            "{field} must use optional generic lenient decoding"
        );
        assert_eq!(
            settings
                .matches(&format!("layout.{field} = Some(value)"))
                .count(),
            1,
            "{field} must have one Strategies-owned store closure"
        );
        let assignments = production_sources
            .iter()
            .map(|path| {
                fs::read_to_string(path)
                    .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()))
                    .matches(&format!(".{field} = Some("))
                    .count()
            })
            .sum::<usize>();
        assert_eq!(
            assignments, 1,
            "{field} must have exactly one production writer"
        );
    }
    assert!(!settings.contains("de_lenient_bool"));

    let write = code_only(braced_body(&settings, "fn write_pref("));
    assert!(write.contains("if (row.read)(&self.prefs) == value"));
    assert!(write.contains("store(&mut backend.layout, value)"));
    assert!(write.contains("backend.layout_dirty = true"));
    assert!(write.contains("self.filter.active_only = self.prefs.active_only"));
    assert!(!settings.contains("strategies_open"));

    let restore = code_only(braced_body(&state, "pub(super) fn new("));
    assert!(restore.contains("StrategiesPrefs::restore(&backend.read(cx).layout)"));
    assert!(restore.contains("active_only: prefs.active_only"));
    assert!(restore.contains("settings_open: false"));

    let disable = code_only(braced_body(
        &settings,
        "pub(super) fn disable_active_only_for_reveal(",
    ));
    assert!(disable.contains("self.set_active_only(false, cx)"));
    let set_active = code_only(braced_body(&settings, "fn set_active_only("));
    assert!(
        set_active.contains("self.write_pref(&ACTIVE_ONLY, value, cx)"),
        "the reveal path and the filter-row toggle must share one persisted writer"
    );
    for function in ["fn clear_filters_for_reveal(", "fn queue_pending_name("] {
        assert!(
            braced_body(&selection, function).contains("disable_active_only_for_reveal(cx)"),
            "{function} must clear active-only through the persisted setter"
        );
    }
    assert!(dialogs.contains("queue_pending_name(core, name, cx)"));
    assert!(dnd.contains("queue_pending_name(core, name, cx)"));

    let panel = code_only(braced_body(
        &read_src("strategies/tree/mod.rs"),
        "pub(super) fn tree_panel(",
    ));
    assert!(panel.contains("settings_popover(") && panel.contains("settings_trigger("));
    assert!(!panel.contains("MoonCheckbox::new"));
    // Scoped to the popup's own group builder: asserting it against the whole file would now be
    // satisfied by the search-row toggle, and the popup could quietly lose every checkbox.
    assert!(code_only(braced_body(&settings, "fn pref_group(")).contains("MoonCheckbox::new"));
    assert!(settings.contains(".close_on_content_click(false)"));
    assert!(settings.contains("fn settings_content_width("));
    assert!(settings.contains(".content_width(settings_content_width(cx))"));
    let measured_width = code_only(braced_body(&settings, "fn settings_content_width("));
    assert!(measured_width.contains("design::ui_text_width("));
    assert!(measured_width.contains("COMPACT_CHECKBOX_MARK + COMPACT_CHECKBOX_GAP"));
    assert!(measured_width.contains("popup_group_inset_px(cx)"));
    assert!(!settings.contains("const CONTENT_WIDTH"));
    for tuner_symbol in [
        "strat_active_only",
        "an-strat-active",
        "analytics.strat.active_only",
    ] {
        assert!(
            !settings.contains(tuner_symbol),
            "Strategies settings must not reuse Analytics Tuner symbol {tuner_symbol}"
        );
    }
}

/// The footer must be one Action-size row whose five labels collapse together after measuring the
/// current locale and staged text; restoring either old v-flex stack recreates the cramped layout.
#[test]
fn strategy_footer_is_one_atomic_action_row() {
    let tree = read_src("strategies/tree/mod.rs");
    let action = code_only(braced_body(&tree, "fn action_bar("));
    assert!(
        !action.contains("v_flex()")
            && !action.contains(".flex_wrap()")
            && !action.contains("MoonButtonSize::Micro")
    );
    assert!(
        action.contains("let show_labels") && action.contains("ui::footer_labels_fit("),
        "action_bar must compute one shared footer-density decision"
    );
    assert!(action.contains("self.panels.tree_w") && action.contains("pane.footer_label_width"));
    // The measurement moved behind the pane cache — a hover repaint rebuilds this row 34-58 times a
    // second and one full label set is ~40 uncached glyph advances — but the density decision must
    // still come from measuring the CURRENT locale and staged text, never from a stored constant.
    let pane_cache = read_src("strategies/tree/pane_cache.rs");
    let measured = code_only(braced_body(&pane_cache, "fn footer_label_width("));
    assert!(measured.contains("design::ui_text_width("));
    assert!(
        measured.contains("t!(\"strat.staged\""),
        "the staged label is part of the width the footer collapses against"
    );
    assert!(action.contains("staged_label") && action.contains("design::chrome_divider("));
    // Substrings rather than the whole call: rustfmt decides where this one breaks, and a contract
    // that pins the line layout fails on formatting instead of on behaviour.
    assert!(action.contains("selection_toolbar(") && action.contains("!cores.is_empty()"));
    for icon in ["icons/play.svg", "icons/pause.svg"] {
        assert!(action.contains(icon), "missing {icon}");
    }
    assert!(action.matches(".tooltip(").count() >= 2);
    assert!(action.find("start-checked") < action.find("stop-checked"));

    let ui = read_src("strategies/tree/ui.rs");
    let selection = code_only(braced_body(&ui, "pub(super) fn selection_toolbar("));
    assert!(!selection.contains("v_flex()") && !selection.contains(".flex_wrap()"));
    assert!(!selection.contains("176.0") && !selection.contains("MoonButtonSize::Micro"));
    assert!(selection.matches("MoonButtonSize::Action").count() >= 3);
    for icon in ["icons/copy.svg", "icons/inbox.svg", "icons/delete.svg"] {
        assert!(selection.contains(icon), "missing {icon}");
    }
    assert!(selection.matches(".tooltip(").count() >= 3);
    assert!(selection.find("sel-copy") < selection.find("sel-paste"));
    assert!(selection.find("sel-paste") < selection.find("sel-delete"));
    assert!(
        action.contains(".padding_x(7.0)") && selection.contains(".padding_x(7.0)"),
        "labeled Action footer buttons must keep text off the border"
    );
}

/// Every window field the tree build reads is an input to the cache signature.
///
/// The adapter is now rebuilt only when `tree::cache::data_sig` moves, because a hover repaint was
/// paying 1.0-1.3 ms to rebuild an identical tree ~85 times a second. That trade is only safe while
/// the signature sees everything the build does: a field read by `build` and missed by the
/// signature renders a tree frozen at whatever it looked like when the last hashed input changed —
/// a checkbox that stays where the user left it after the core disagreed, a folder count stuck
/// mid-session. Both look like data bugs, not like a cache.
///
/// So this walks the build's own source for `view.<field>` reads and requires each one to appear in
/// the signature. The plausible edit it catches: adding a filter, an expansion set or a highlight
/// to the tree and not to `data_sig` — which compiles, renders correctly on the frame it is
/// introduced, and goes stale on the next hover.
#[test]
fn the_tree_cache_signature_covers_every_input_the_build_reads() {
    let src = read_src("strategies/tree/moon.rs");
    let cache = read_src("strategies/tree/cache.rs");

    // Whitespace-stripped so a field reached across a line break (`view\n    .deleted`) is found
    // the same way as one written inline — the formatter decides which of the two it is.
    let scanned: String = [
        "pub(crate) fn build(",
        "fn build_core_root(",
        "fn build_core_subtree(",
        "fn convert_node(",
    ]
    .iter()
    .map(|f| braced_body(&src, f))
    .collect::<Vec<_>>()
    .join("\n");
    let packed: String = scanned.chars().filter(|c| !c.is_whitespace()).collect();

    // Accessors read state indirectly, so they stand in for the field behind them.
    fn behind(field: &str) -> &str {
        match field {
            "drag_ids_for_core" => "sel",
            "ui_folder_paths" => "ui_folders",
            // Loaded in the background; the map itself is too big to hash, so an explicit counter
            // represents it. `deleted_gen` deliberately does NOT: it moves when a load STARTS.
            "deleted" => "deleted_rev",
            other => other,
        }
    }

    let mut missing = Vec::new();
    let mut checked = 0;
    for chunk in packed.split("view.").skip(1) {
        let field: String = chunk
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        if field.is_empty() {
            continue;
        }
        checked += 1;
        let expected = behind(&field);
        if !cache.contains(&format!("view.{expected}")) {
            missing.push(format!("{field} (expected view.{expected} in data_sig)"));
        }
    }
    assert!(
        checked >= 8,
        "the scan found only {checked} field reads — the build was renamed or split and this test \
         is now checking nothing"
    );
    missing.sort();
    missing.dedup();
    assert!(
        missing.is_empty(),
        "these inputs are read by the tree build but absent from the cache signature, so the tree \
         would render stale after they change:\n{}",
        missing.join("\n")
    );

    // The store side has no `view.` form to scan for, and the captions carry both numbers. Matched
    // as CODE, not as any occurrence: the prose here names the counters it rejects, so a substring
    // search over the whole file would go green on a comment.
    let sig_body = code_only(braced_body(&cache, "pub(crate) fn data_sig("));
    for read in [
        "view.prefs.group_by_venue",
        "view.filter.active_only",
        "venues_digest(",
    ] {
        assert!(
            sig_body.contains(read),
            "the tree cache must retain preference/venue input {read}"
        );
    }
    for read in ["cd.strategies_rev", "open_orders_digest(cd)"] {
        assert!(
            sig_body.contains(read),
            "the signature must read {read}, or the tree freezes while the core keeps talking"
        );
    }
    // `orders_table_rev` advances for every order batch, including a price moving on an untouched
    // order — keying on it made a busy account miss on roughly half its frames.
    assert!(
        !sig_body.contains("orders_table_rev"),
        "the open-order digest, not the batch revision: the tree renders counts, not batches"
    );
    // The Deleted folder's caption comes from the dictionary and a language switch is only a
    // repaint, which moves nothing else the signature reads.
    assert!(
        cache.contains("rust_i18n::locale()"),
        "the active locale must be an input, or a language switch leaves the Deleted row translated"
    );

    // A counter nothing increments is worse than no counter: it reads as covered.
    let versions = read_src("strategies/versions.rs");
    let ensure = braced_body(&versions, "pub(super) fn ensure_deleted(");
    assert!(
        ensure.contains("deleted_rev = this.deleted_rev.wrapping_add(1)"),
        "deleted_rev must advance where `deleted` is actually replaced"
    );

    // And the render path has to consult the cache at all.
    let window = read_src("strategies/mod.rs");
    let render = braced_body(&window, "fn render(&mut self, window: &mut Window");
    assert!(
        render.contains("tree::cache::data_sig(") && render.contains("TreeCache::store("),
        "render must key the build on the signature and retain the result"
    );
}

/// A core/folder row's `ElementId` is the node's own tree id, never its rendered caption.
///
/// GPUI keeps `pending_mouse_down` in element state addressed by `ElementId` (fork
/// `elements/div.rs`), and a click is only delivered when press and release find the SAME state. A
/// caption-derived id ("core  3/10  (2)") changes identity the moment a counter moves, so any
/// repaint between press and release — which a trading core produces continuously — discards the
/// click and the folder silently fails to expand.
///
/// The plausible edit this catches: rebuilding the id from the row text "so it reads nicely in the
/// inspector", which compiles, looks correct, and only misbehaves on a live account.
#[test]
fn a_core_folder_rows_element_id_is_its_node_id() {
    let src = read_src("strategies/tree/moon.rs");
    let row = code_only(braced_body(&src, "fn core_folder_row("));
    assert!(
        row.contains(".id(row_id)"),
        "the core/folder row must take its ElementId from the node id passed in"
    );
    assert!(
        !row.contains("format!(\"strat-tree-cf-"),
        "a caption-derived ElementId changes whenever a count moves and drops the click"
    );
    // The id has to be the node's, not another per-frame string: `render_row` reads it from the
    // MoonTree entry, which is exactly the `c:`/`f:`/`d:` identity `build` assigned.
    let render = code_only(braced_body(&src, "fn render_row("));
    assert!(
        render.contains("let node_id = entry.item().id().clone();"),
        "render_row must source the row id from the tree entry's node id"
    );
}

/// `core_folder_row`'s expand/collapse marker must stay a passive `MoonDisclosure::glyph`.
///
/// Plausible edit: swap `MoonDisclosure::glyph(expanded)` for `MoonDisclosure::button(id, ..)` so
/// the marker "looks" clickable on its own. In this fork `should_insert_hitbox` inserts a hitbox
/// for a cursor, a hover style OR a listener — `button` installs all three — so the marker would
/// then take a hitbox of its own and swallow the click meant for the whole core/folder row, which
/// is what actually expands/selects on click. Clicking anywhere else on the row still works, so
/// this is invisible without clicking the marker itself.
#[test]
fn a_core_folder_row_marker_stays_passive() {
    let src = read_src("strategies/tree/moon.rs");
    let body = code_only(braced_body(&src, "fn core_folder_row("));
    assert!(
        body.contains("MoonDisclosure::glyph(expanded)"),
        "the core/folder row marker must stay a passive MoonDisclosure::glyph, not ::button"
    );
    assert!(
        !body.contains("MoonDisclosure::button("),
        "an interactive MoonDisclosure::button here would take a hitbox and swallow the row's click"
    );
}

/// The Strategies window seeds its tree expansion from the Auto workspace's selected core, not
/// from an unconditional empty set.
///
/// Plausible edit: reverting construction to the literal `expanded_cores: HashSet::new()` — the
/// window would then always open fully collapsed even with a concrete Auto core selected on the
/// rail, and the user has to re-find that server by hand every time.
#[test]
fn strategies_window_seeds_expansion_from_the_auto_workspace() {
    let src = code_only(&read_src("strategies/state.rs"));
    assert!(
        src.contains("initial_expanded_cores("),
        "construction must seed its expanded cores through initial_expanded_cores"
    );
    assert!(
        !src.contains("expanded_cores: HashSet::new()"),
        "construction must not hard-code an empty expansion, or the Auto rail seed is dead code"
    );
}
