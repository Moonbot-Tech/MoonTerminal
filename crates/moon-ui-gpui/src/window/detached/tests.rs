//! Which detached panels get a window back, and which records simply wait for one.

// NOT `use super::*`: the parent imports `gpui::*`, whose `test` macro shadows `#[test]`.
use super::{DetachedSpec, deduplicated, save_all_to_path, should_reopen};
use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};

/// Sequence making every unwritable test path unique inside one process.
static NEXT_TEMP_PATH: AtomicU64 = AtomicU64::new(1);

/// Return a destination below a path that is deliberately a file, not a directory.
///
/// Returns:
///     Unique destination plus the blocking parent file to remove after the assertion.
fn unwritable_path() -> (std::path::PathBuf, std::path::PathBuf) {
    let sequence = NEXT_TEMP_PATH.fetch_add(1, Ordering::Relaxed);
    let blocker = std::env::temp_dir().join(format!(
        "moonterminal-blocked-detached-parent-{}-{sequence}",
        std::process::id()
    ));
    std::fs::write(&blocker, b"not a directory").expect("blocking test file must be written");
    (blocker.join("detached.json"), blocker)
}

/// A detached panel is reopened only against a live owner window, and never on top of a state that
/// already accounts for it: an open window, a queued repin, or a dock that will restore it.
#[test]
fn a_panel_is_reopened_only_when_nothing_else_accounts_for_it() {
    let spec = |group: &str, panel: &str| DetachedSpec::new(group.to_string(), panel.to_string());
    let live: HashSet<&str> = ["G1"].into_iter().collect();
    let auto = HashSet::new();
    let open: HashSet<(&str, &str)> = [("G1", "log")].into_iter().collect();
    let repins: HashSet<(&str, &str)> = [("G1", "report")].into_iter().collect();
    let docked: HashSet<(&str, &str)> = [("G1", "alerts")].into_iter().collect();

    assert!(should_reopen(
        &spec("G1", "orders"),
        &live,
        &auto,
        &open,
        &repins,
        &docked
    ));
    // A window is already open for it: a second one would orphan the first, which could then never
    // repin, because the handle map would name only the newer window.
    assert!(!should_reopen(
        &spec("G1", "log"),
        &live,
        &auto,
        &open,
        &repins,
        &docked
    ));
    // Its group has no live window to own it — `spawn` would produce a top-level window that no
    // longer dies with its group.
    assert!(!should_reopen(
        &spec("G2", "orders"),
        &live,
        &auto,
        &open,
        &repins,
        &docked
    ));
    // The user just closed it and its repin has not drained yet: that request means "put the panel
    // back in the dock", so reopening the window would fight it.
    assert!(!should_reopen(
        &spec("G1", "report"),
        &live,
        &auto,
        &open,
        &repins,
        &docked
    ));
    // A stale spec whose panel the restorable dock will bring back anyway; reopening it would show
    // the panel twice, once as a tab and once as a window.
    assert!(!should_reopen(
        &spec("G1", "alerts"),
        &live,
        &auto,
        &open,
        &repins,
        &docked
    ));
}

/// Whatever a settings save does to windows, the detachment RECORD survives it: a deactivated or
/// renamed group must not cost the user a panel, because the dock no longer holds it either. This
/// is the regression that motivated the rule — the teardown used to repin every detached panel and
/// delete its spec from `detached.json`.
#[test]
fn a_detached_record_outlives_a_windowless_group() {
    let orders = DetachedSpec::new("G3".to_string(), "orders".to_string());
    let no_groups: HashSet<&str> = HashSet::new();
    let no_panels: HashSet<(&str, &str)> = HashSet::new();

    assert!(!should_reopen(
        &orders, &no_groups, &no_groups, &no_panels, &no_panels, &no_panels
    ));
    // ...and once its group has a window again, the same spec is what brings the panel back.
    let live: HashSet<&str> = ["G3"].into_iter().collect();
    assert!(should_reopen(
        &orders, &live, &no_groups, &no_panels, &no_panels, &no_panels
    ));
}

/// Catches removing the Auto-group exclusion from `window/detached.rs:should_reopen`; a settings
/// reconcile would reopen Classic detached windows on top of Auto's in-window panel instances.
#[test]
fn auto_suspends_classic_detached_windows_without_deleting_their_specs() {
    let spec = DetachedSpec::new("G1".to_string(), "orders".to_string());
    let live: HashSet<&str> = ["G1"].into_iter().collect();
    let auto: HashSet<&str> = ["G1"].into_iter().collect();
    let no_auto: HashSet<&str> = HashSet::new();
    let no_panels: HashSet<(&str, &str)> = HashSet::new();

    assert!(!should_reopen(
        &spec, &live, &auto, &no_panels, &no_panels, &no_panels,
    ));
    assert!(should_reopen(
        &spec, &live, &no_auto, &no_panels, &no_panels, &no_panels,
    ));
}

/// Changing `detached.rs:save_all_to_path` to return success after an atomic-write error must
/// fail: both startup flushes would clear `detached_dirty` and lose the detachment on restart.
#[test]
fn detached_save_reports_an_atomic_write_failure() {
    let (path, blocker) = unwritable_path();
    let saved = save_all_to_path(&[], &path);
    std::fs::remove_file(blocker).expect("blocking test file must be removed");
    assert!(!saved);
    assert!(!path.exists());
}

/// `detached.rs:deduplicated` must retain one spec per stable identity; removing it lets two loaded
/// records spawn windows whose handle map can own only the later one.
#[test]
fn duplicate_detached_records_collapse_before_respawn() {
    let mut first = DetachedSpec::new("G1".into(), "orders".into());
    first.x = 100;
    let mut duplicate = first.clone();
    duplicate.x = 900;
    let unique = DetachedSpec::new("G1".into(), "report".into());

    let normalized = deduplicated(vec![first, duplicate, unique]);
    assert_eq!(normalized.len(), 2);
    assert_eq!(normalized[0].panel, "orders");
    assert_eq!(normalized[0].x, 100);
    assert_eq!(normalized[1].panel, "report");
    let source = include_str!("../detached.rs");
    assert!(source.contains("deduplicated(serde_json::from_str"));
    assert!(source.contains("deduplicated(backend.read(cx).detached.clone())"));
}

/// Deleting `cx.notify()` from `DetachedWindow::new` must fail: an owned native close would queue
/// the repin but leave Log absent until unrelated Backend traffic or a workspace-mode toggle.
#[test]
fn owned_window_release_queues_removes_and_then_wakes_shell() {
    let source = include_str!("../detached.rs");
    let release = source
        .split("cx.on_release(move |this, app|")
        .nth(1)
        .and_then(|tail| tail.split(".detach();").next())
        .expect("DetachedWindow must retain its native release observer");
    let owns = release
        .find("if !owns_panel")
        .expect("release must revalidate ownership");
    let queue = release
        .find("b.repin_request.push")
        .expect("owned release must queue a repin");
    let remove = release
        .find("b.detached_panel_windows.remove")
        .expect("owned release must remove its stale handle");
    let notify = release
        .find("cx.notify()")
        .expect("owned release must wake Shell's Backend observer");
    assert!(owns < queue && queue < remove && remove < notify);
}

/// Removing the `user_close` guard from `DetachedWindow::on_release` must fail. A detached panel is
/// an OS-owned child of its group window, so application exit destroys it BEFORE the group window
/// whose close unregisters it: without the guard that release reads as user-driven, docks the panel
/// and deletes its `DetachedSpec`, and the detachment does not survive the restart. The flag is fed
/// by `on_window_should_close`, which on Windows every user-driven close routes through.
#[test]
fn a_release_the_user_did_not_ask_for_stays_silent() {
    let source = include_str!("../detached.rs");
    assert!(
        source.contains("window.on_window_should_close(cx"),
        "the user-close signal must come from the platform close request"
    );
    let release = source
        .split("cx.on_release(move |this, app|")
        .nth(1)
        .and_then(|tail| tail.split(".detach();").next())
        .expect("DetachedWindow must retain its native release observer");
    let guard = release
        .find("if !this.user_close.get()")
        .expect("release must ignore a close the user did not request");
    let queue = release
        .find("b.repin_request.push")
        .expect("a user-driven release must queue a repin");
    assert!(guard < queue, "the guard must precede queueing the repin");

    // Seeding the flag `true` everywhere would restore the bug on the shipping platforms, where the
    // hook is authoritative. Only a platform whose frame button bypasses it (Linux) may start true.
    let seed = source
        .split("let user_close = std::rc::Rc::new(std::cell::Cell::new(")
        .nth(1)
        .and_then(|tail| tail.split("));").next())
        .expect("the flag must be seeded from the platform's close semantics");
    assert!(
        seed.contains("target_os = \"windows\"") && seed.contains("target_os = \"macos\""),
        "Windows and macOS route every user close through the hook and must start from false"
    );
}

/// Replacing the timer await in `detached.rs:respawn_all` with `cx.defer` must fail: GPUI may drain
/// every deferred open in one turn, recreating the multi-window Classic transition stall.
#[test]
fn detached_respawn_yields_and_revalidates_each_spec_before_opening() {
    let source = include_str!("../detached.rs");
    let body = source
        .split("pub(crate) fn respawn_all")
        .nth(1)
        .expect("respawn_all must exist");
    let code: String = body
        .lines()
        .filter(|line| !line.trim_start().starts_with("///"))
        .collect();
    assert!(
        !code.contains("cx.defer"),
        "respawn must not use same-turn defer"
    );
    let loop_start = code
        .find("for spec in specs")
        .expect("specs must retain stable order");
    let timer = code[loop_start..]
        .find("executor.timer(DETACHED_RESPAWN_YIELD).await")
        .map(|offset| loop_start + offset)
        .expect("each spec must yield through a real timer");
    let revalidate = code[loop_start..]
        .find("should_reopen_now(backend.read(app), &spec)")
        .map(|offset| loop_start + offset)
        .expect("each yielded spec must be checked against live Backend state");
    let open = code[loop_start..]
        .find("spawn(app, &backend, &mut spec, None, None)")
        .map(|offset| loop_start + offset)
        .expect("an authorized spec must open once");
    assert!(timer < revalidate && revalidate < open);
}

/// Removing the `Backend::quitting` gate from `detached.rs:should_reopen_now` must fail: a yielded
/// Classic respawn could otherwise create a native panel after the final shutdown snapshot began.
#[test]
fn detached_respawn_refuses_native_creation_during_shutdown() {
    let source = include_str!("../detached.rs");
    let guard = source
        .split("fn should_reopen_now")
        .nth(1)
        .and_then(|tail| tail.split("const DETACHED_RESPAWN_YIELD").next())
        .expect("post-yield detached authority guard must remain bounded");
    assert!(guard.contains("if backend.quitting"));
    assert!(guard.contains("return false;"));
}
