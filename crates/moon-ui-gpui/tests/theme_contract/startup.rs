//! Static startup contracts for persistent background work in the binary-only UI crate.

use super::support::*;

/// Removing startup reconciliation, the setter's dirty write, or the background snapshot dispatch
/// would leave first-run and upgraded profiles on UTC after the next reboot even though detection
/// worked.
#[test]
fn startup_detects_and_persists_an_untouched_profiles_system_zone() {
    let startup = code_only(&read_src("startup.rs"));
    let clock = code_only(&read_src("chrome/clock.rs"));
    let backend = code_only(&read_src("backend/mod.rs"));
    let reconcile = braced_body(
        &clock,
        "pub(crate) fn reconcile_clock_zone(backend: &Entity<Backend>, cx: &mut App)",
    );
    let setter = braced_body(&backend, "pub(crate) fn set_header_clock_zone(");

    assert!(
        startup.contains("crate::chrome::clock::reconcile_clock_zone(&backend, cx);")
            && reconcile.contains("iana_time_zone::get_timezone().ok()")
            && reconcile
                .contains("b.set_header_clock_zone(&target.zone_id, target.offset_min, bcx)")
            && setter.contains("self.layout.header_clock_zone = Some(zone.to_string())")
            && setter.contains("self.layout_dirty = true")
            && startup.contains("snapshot = snapshot.with_layout(backend.layout.clone())")
            && startup.contains("coordinator.dispatch(snapshot)"),
        "normal startup must detect, store, dirty, and flush the exact IANA zone for the next reboot"
    );
}

/// `startup.rs:dispatch_live_persistence` must not call any blocking layout writer; adding one restores
/// the user-visible 2-3 second GPUI freeze instead of enqueueing an immutable worker snapshot.
#[test]
fn live_layout_classic_and_auto_persistence_only_dispatch_to_the_worker() {
    let startup = code_only(&read_src("startup.rs"));
    let coordinator = code_only(&read_src("persistence/coordinator.rs"));
    let dispatch = braced_body(
        &startup,
        "fn dispatch_live_persistence(backend: &mut Backend, coordinator: &mut PersistenceCoordinator)",
    );
    assert!(
        dispatch.contains("coordinator.poll()")
            && dispatch.contains("coordinator.is_in_flight()")
            && dispatch.contains("backend.layout.clone()")
            && dispatch.contains("backend.dock_states.clone()")
            && dispatch.contains("backend.detached.clone()")
            && dispatch.contains("backend.auto_dock_topology.clone()")
            && dispatch.contains("snapshot = snapshot.with_auto(topology)")
            && dispatch.contains("coordinator.dispatch(snapshot)"),
        "the live path must poll one acknowledgement and enqueue complete immutable authorities"
    );
    assert!(
        !dispatch.contains(".save()")
            && !dispatch.contains("window_state_persist::save_all")
            && !dispatch.contains("auto_dock_persist::save")
            && coordinator.contains("thread::Builder::new()")
            && coordinator.contains("sink.save_layout(layout)")
            && coordinator.contains("sink.save_classic(docks, detached)")
            && coordinator.contains("sink.save_auto(topology)"),
        "layout, Classic, and Auto file I/O must execute only inside the standard-thread sink"
    );
}

/// `startup.rs:apply_persistence_ack` must re-dirty failures, while
/// `startup.rs:dispatch_live_persistence` must not clear a later mutation from a stale success;
/// deleting either rule loses the latest layout or Classic authority after restart.
#[test]
fn failed_or_stale_background_persistence_keeps_dirty_state_for_retry() {
    let startup = code_only(&read_src("startup.rs"));
    let dispatch = braced_body(
        &startup,
        "fn dispatch_live_persistence(backend: &mut Backend, coordinator: &mut PersistenceCoordinator)",
    );
    let apply = braced_body(
        &startup,
        "fn apply_persistence_ack(backend: &mut Backend, acknowledgement: PersistenceAck)",
    );
    assert!(
        apply.contains("if failed.layout {")
            && apply.contains("backend.layout_dirty = true;")
            && apply.contains("if failed.classic {")
            && apply.contains("backend.dock_dirty = true;")
            && apply.contains("backend.detached_dirty = true;")
            && apply.contains("if failed.auto {")
            && apply.contains("backend.auto_dock_dirty = true;"),
        "a failed acknowledgement must re-dirty every affected authority"
    );
    assert!(
        dispatch.find("coordinator.poll()").unwrap()
            < dispatch.find("coordinator.is_in_flight()").unwrap()
            && dispatch.find("backend.layout_dirty = false;").unwrap()
                < dispatch.find("coordinator.dispatch(snapshot)").unwrap(),
        "an acknowledgement is consumed before dispatch, while accepted snapshot flags are cleared before later mutations can be observed"
    );
    let after_poll = dispatch
        .split_once("if let Some(acknowledgement) = coordinator.poll() {")
        .unwrap()
        .1
        .split_once("if coordinator.is_in_flight() {")
        .unwrap()
        .0;
    assert!(
        !after_poll.contains("backend.layout_dirty = false;")
            && !after_poll.contains("backend.dock_dirty = false;")
            && !after_poll.contains("backend.detached_dirty = false;"),
        "a completed older request must never clear a mutation made while that request was in flight"
    );

    assert!(
        !startup.contains("crate::persistence::auto_dock_persist::save"),
        "startup must not bypass the serial worker for Auto topology writes"
    );
}

/// `startup.rs` quit handling must call `PersistenceCoordinator::shutdown` with every full
/// authority; making Auto conditional on its cleared-on-enqueue dirty flag can lose a failed live
/// write, while restoring any direct saver can race the worker's atomic temp paths on exit.
#[test]
fn quit_serializes_the_latest_full_snapshot_behind_live_work() {
    let startup = code_only(&read_src("startup.rs"));
    let coordinator = code_only(&read_src("persistence/coordinator.rs"));
    let quit = braced_body(&startup, "cx.on_app_quit(");
    let shutdown = braced_body(
        &coordinator,
        "pub(crate) fn shutdown(&mut self, snapshot: PersistenceSnapshot) -> PersistenceAck",
    );
    assert!(
        quit.contains("PersistenceSnapshot::empty()")
            && quit.contains(".with_layout(b.layout.clone())")
            && quit.contains(".with_classic(b.dock_states.clone(), b.detached.clone())")
            && quit.contains("snapshot = snapshot.with_auto(topology)")
            && !quit.contains("b.auto_dock_dirty")
            && quit.contains(".shutdown(final_persistence)"),
        "quit must capture and send the latest complete layout, Classic, and allowed Auto authority"
    );
    assert!(
        !quit.contains("b.layout.save()")
            && !quit.contains("window_state_persist::save_all")
            && !quit.contains("auto_dock_persist::save")
            && shutdown.contains("WorkerCommand::Shutdown(snapshot, final_ack_tx)")
            && shutdown.contains("final_ack_rx.recv()")
            && shutdown.contains("worker.join()")
            && shutdown.contains("fallback_snapshot.retain_classes(failed)"),
        "quit must serialize through the worker and retain a quit-only fallback for failed final classes"
    );
}

/// Replacing the invalid-load persistence lock with an ordinary first-run seed must fail: merely
/// opening Auto would overwrite the damaged `auto_dock.json` before the user edits the topology.
#[test]
fn invalid_auto_dock_waits_for_an_explicit_user_topology_change() {
    let persistence = code_only(&read_src("persistence/auto_dock_persist.rs"));
    let backend = code_only(&read_src("backend/mod.rs"));
    let shell_init = code_only(&read_src("shell/init.rs"));
    let shell_workspace = code_only(&read_src("shell/workspace.rs"));
    let startup_state = braced_body(
        &persistence,
        "pub(crate) fn into_startup_state(self) -> AutoDockStartupState",
    );
    let reconcile = braced_body(&backend, "pub(crate) fn reconcile_auto_dock_topology(");
    let user_setter = braced_body(&backend, "pub(crate) fn set_auto_dock_topology(");

    assert!(
        startup_state.contains("Self::InvalidOrUnreadable => AutoDockStartupState {")
            && startup_state.contains("automatic_persistence_allowed: false"),
        "invalid or unreadable Auto data must start with automatic persistence locked"
    );
    assert!(
        reconcile.contains("if self.auto_dock_automatic_persistence_allowed {")
            && reconcile.contains("self.auto_dock_dirty = true;"),
        "programmatic seed and repair may dirty only an automatically writable startup state"
    );
    assert!(
        user_setter.contains("self.auto_dock_automatic_persistence_allowed = true;")
            && user_setter.contains("self.auto_dock_dirty = true;"),
        "a distinct user topology change must unlock and dirty the protected authority"
    );
    assert!(
        shell_init.contains("if this.applying_auto_topology {")
            && shell_workspace
                .matches("backend.reconcile_auto_dock_topology(")
                .count()
                == 2,
        "programmatic topology installs must stay on the recovery-aware reconcile path"
    );
}

/// Removing the live Classic name set from Auto temporary-panel construction must fail: stale
/// `docks.json` plus `detached.json` would resolve two local panel identities under one name.
#[test]
fn live_classic_panel_names_outrank_stale_detached_records_in_auto() {
    let workspace = code_only(&read_src("shell/workspace.rs"));
    let names = braced_body(&workspace, "fn auto_only_detached_panel_names(");
    let apply = braced_body(&workspace, "pub(super) fn apply_workspace_mode(");

    assert!(
        names.contains("classic_panel_names.iter().cloned().collect::<HashSet<_>>()")
            && names.contains("spec.group == group && accounted.insert(spec.panel.clone())"),
        "temporary Auto panels must exclude names already owned by the live Classic dock"
    );
    assert!(
        apply.contains("let classic_panel_names = classic.panel_names();")
            && apply.contains("auto_only_detached_panel_names("),
        "Auto entry must derive temporary panels from the live captured layout, not persisted Rc state"
    );
}

/// Removing the `firetest_config.is_none()` guard would let a diagnostic FireTest run create and
/// prune durable settings or strategy backups even though its other persistence paths are gated.
#[test]
fn firetest_does_not_start_the_daily_backup_scheduler() {
    let source = read_src("startup.rs");
    let run = code_only(fn_body(
        &source,
        "pub(crate) fn run() -> anyhow::Result<()> {",
    ));
    assert!(
        run.contains("AppConfig::load(uid_floor, firetest_config.is_none())?;"),
        "schema migration backups must receive the same FireTest exclusion"
    );
    let gate = run
        .split_once("if firetest_config.is_none() {")
        .expect("normal startup must explicitly exclude FireTest from persistent backup work")
        .1
        .split_once('}')
        .map(|(body, _)| body)
        .expect("the FireTest exclusion must have a bounded body");

    assert!(
        gate.contains("moon_core::backups::start_daily(&cfg);"),
        "the daily backup scheduler must start only inside the FireTest exclusion"
    );
    assert_eq!(
        run.matches("moon_core::backups::start_daily(&cfg);")
            .count(),
        1,
        "an unguarded second scheduler start would let FireTest create backups"
    );
}

/// Reintroducing any backup call in the Settings Save handler would violate daily-only settings
/// retention and recreate snapshots on every click.
#[test]
fn settings_save_does_not_create_a_backup() {
    let source = read_src("settings/apply.rs");
    let save = code_only(fn_body(
        &source,
        "pub(super) fn save(&mut self, cx: &mut Context<Self>) {",
    ));

    for forbidden in ["backup_due", "backup_now", "snapshot", "save_with_snapshot"] {
        assert!(
            !save.contains(forbidden),
            "Settings Save must not call backup creation path {forbidden}"
        );
    }
    assert!(save.contains("candidate.save();"));
}
