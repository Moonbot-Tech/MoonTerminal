//! The strategies window: folder-path splitting, copy placement and reveal, the refresh
//! channel, and the weak handles a long-lived `MoonTree` state must hold.

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

/// `startup.rs` must notify the dedicated report revision entity rather than the
/// global Backend; restoring `mark_backend_dirty` repaints every shell for each
/// report burst and makes Report/Analytics wake fan-out scale with window count.
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
        .split("consume_report_commit(coord_report_dirty.as_deref()")
        .nth(1)
        .expect("startup must consume the committed-report edge");
    let commit_block = commit_block
        .split("b.maybe_diag_open_first_market")
        .next()
        .unwrap();
    assert!(
        !commit_block.contains("mark_backend_dirty"),
        "a report commit must not notify the global Backend entity"
    );
}

/// `analytics/mod.rs:refresh_visible_report_data` must route a stale Strategies
/// base through `strategy_base_data`; replacing it with `reload_summary` restores
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
/// This already shipped once as a user-visible bug — the Settings window silently refused to
/// reopen, because its `on_release` was what cleared the draft that `settings::open` gates on.
/// `strategies/tree_moon.rs` has the same shape with a different symptom: `strategies::open` has
/// no such gate, so it leaks a view plus its subscriptions on every open/close cycle instead.
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
