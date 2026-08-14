//! Static contracts for updater startup, quit authority, and header placement.

use super::support::*;

/// Reconstructing a numeric version into a tag would rewrite historical `v0.21` metadata as the
/// nonexistent alias `v0.21.0`, breaking both helper validation and the exact download authority.
#[test]
fn update_manifest_preserves_the_exact_candidate_release_tag() {
    let update = code_only(&read_src("update.rs"));
    let transaction = braced_body(&update, "fn build_transaction(");
    assert!(
        transaction.contains("release_tag: candidate.release_tag().to_owned()")
            && !transaction.contains("candidate.version().tag()"),
        "the installer manifest must retain the exact immutable GitHub release tag"
    );
}

/// Returning a readiness timeout without stopping the exact helper would allow it to publish its
/// marker later and replace the executable after the UI already reported a retryable failure.
#[test]
fn failed_helper_readiness_terminates_before_transaction_cleanup() {
    let update = code_only(&read_src("update.rs"));
    let prepare = braced_body(&update, "fn prepare_install(candidate: AvailableRelease)");
    let failure = prepare
        .find("if let Err(error) = ready")
        .expect("helper readiness must retain an explicit failure branch");
    let terminate = prepare[failure..]
        .find("terminate_child(&mut helper)")
        .map(|offset| failure + offset)
        .expect("failed readiness must terminate the exact helper");
    let cleanup = prepare[failure..]
        .find("cleanup_abandoned_transaction(&transaction, &manifest_path);")
        .map(|offset| failure + offset)
        .expect("terminated helper transaction must be cleaned");
    assert!(
        failure < terminate && terminate < cleanup,
        "a failed pre-quit helper must be stopped before its transaction is retired"
    );
}

/// Letting `ready` alone authorize replacement would allow a late helper to replace the exe after
/// the UI timed out; the helper must require the old app's nonce-bound commit before parent exit.
#[test]
fn helper_replacement_requires_ready_then_commit_then_parent_exit() {
    let update = code_only(&read_src("update.rs"));
    let prepare = braced_body(&update, "fn prepare_install(candidate: AvailableRelease)");
    let helper = braced_body(&update, "fn run_helper(manifest_path: &Path, nonce: &str)");
    let ready_wait = prepare.find("wait_for_marker_or_exit(").unwrap();
    let commit_write = prepare
        .find("write_marker(&transaction.commit, &transaction.nonce)")
        .unwrap();
    let ready_write = helper
        .find("write_marker(&transaction.ready, nonce)")
        .unwrap();
    let commit_wait = helper
        .find("wait_for_marker(\n        &transaction.commit")
        .unwrap();
    let parent_wait = helper.find("wait_parent(parent)").unwrap();
    assert!(
        ready_wait < commit_write && ready_write < commit_wait && commit_wait < parent_wait,
        "replacement authority must cross the ready/commit handshake before parent exit"
    );
}

/// Moving helper dispatch into `startup::run` would open portable storage before untrusted helper
/// arguments are validated, while an updater-specific process exit would bypass persistence.
#[test]
fn update_process_modes_dispatch_before_gpui_and_quit_through_app() {
    let main = code_only(&read_src("main.rs"));
    let update = code_only(&read_src("update.rs"));
    let boot = code_only(&read_src("startup/boot.rs"));
    let main_body = braced_body(&main, "fn main() -> anyhow::Result<()>");
    let install = braced_body(
        &update,
        "pub(crate) fn start_install(entity: &Entity<Self>, cx: &mut App)",
    );
    let prepared = install
        .find("prepare_install(candidate)")
        .expect("installation must wait for helper readiness");
    let successful = install
        .find("Ok(_) => {")
        .expect("installation must branch on helper readiness");
    let failed = install
        .find("Err(error) => {")
        .expect("installation must retain a failure branch");
    let quit = install.find("cx.quit();").expect("ready helper must quit");
    assert!(
        main_body.contains("update::dispatch_process_mode()?")
            && main_body.contains("startup::run(receipt)")
            && prepared < successful
            && successful < quit
            && quit < failed
            && !install.contains("process::exit")
            && boot.contains("cx.on_app_quit("),
        "hidden modes must dispatch before GPUI and successful readiness must retain App::quit persistence"
    );
}

/// Moving the button into the ticker cluster changes its hand-positioned offset; separating the
/// divider predicate leaves an empty rule whenever no update exists.
#[test]
fn update_button_and_divider_share_the_pre_ticker_header_cluster() {
    let chrome = code_only(&read_src("chrome/terminal_chrome.rs"));
    let spacer = chrome.find("terminal-header-spacer-drag").unwrap();
    let button = chrome.find("terminal-update-tooltip").unwrap();
    let ticker = chrome
        .find("design::ticker_visible(cx, chrome_width)")
        .unwrap();
    assert!(
        spacer < button
            && button < ticker
            && chrome[spacer..ticker].contains("update_state.visible().then")
            && chrome[button..ticker].contains("design::chrome_divider(cx, p)"),
        "the coupled update button/divider must stay after the drag spacer and before ticker"
    );
}

/// Constructing or starting an updater per Shell would let separate windows scan, download, and
/// replace concurrently instead of observing the one process authority.
#[test]
fn every_shell_observes_the_one_backend_updater() {
    let main = code_only(&read_src("main.rs"));
    let boot = code_only(&read_src("startup/boot.rs"));
    let shell = code_only(&read_src("shell/init.rs"));
    let chrome = code_only(&read_src("chrome/terminal_chrome.rs"));
    assert!(
        main.contains("updater: Entity<update::UpdateController>")
            && boot.matches("UpdateController::new()").count() == 1
            && boot
                .matches("UpdateController::start_polling(&updater, cx)")
                .count()
                == 1
            && shell.contains("let updater = backend.read(cx).updater.clone();")
            && shell.contains("cx.observe(&updater")
            && !shell.contains("start_polling")
            && !chrome.contains("start_polling"),
        "one backend-owned updater must repaint every group header"
    );
}

/// Removing the Windows/FireTest gates would spend GitHub quota in unsupported or diagnostic
/// processes; arming the timer before awaiting the scan would permit overlapping requests.
#[test]
fn recurring_update_scans_are_gated_idempotent_and_sequential() {
    let boot = code_only(&read_src("startup/boot.rs"));
    let update = code_only(&read_src("update.rs"));
    let start = braced_body(
        &update,
        "pub(crate) fn start_polling(entity: &Entity<Self>, cx: &mut App)",
    );
    let discovery = start.find("ReleaseDiscovery::new(identity)").unwrap();
    let scan_loop = start.find("loop {").unwrap();
    let scan = start.find("discovery.scan()").unwrap();
    let timer = start.find("executor.timer(wait).await").unwrap();
    assert!(
        boot.contains(
            "if firetest_config.is_none() {\n        #[cfg(windows)]\n        crate::update::UpdateController::start_polling(&updater, cx);"
        ) && start.contains("claim_polling(&mut this.polling_started)")
            && start.contains("polling_continues_after(&this.state)")
            && start.matches("ReleaseDiscovery::new(identity)").count() == 1
            && discovery < scan_loop
            && scan_loop < scan
            && scan < timer
            && !start.contains("install_generation"),
        "one Windows-only non-FireTest loop must await every scan before arming its next timer"
    );
}

/// Delaying health until after migration permits an exe rollback against state already rewritten
/// by the new schema, while acknowledging it in process dispatch proves too little startup code.
#[test]
fn resumed_update_is_accepted_before_portable_storage_changes() {
    let startup = code_only(&read_src("startup.rs"));
    let health = startup
        .find("crate::update::acknowledge_healthy(startup_update.as_ref())?;")
        .unwrap();
    let migration = startup
        .find("moon_core::config::paths::migrate_bundle_data();")
        .unwrap();
    let config_load = startup.find("unlock::start(").unwrap();
    assert!(
        health < migration && health < config_load,
        "health acceptance must precede every portable migration and configuration unlock"
    );
}

/// Swallowing a durable health-marker failure would let startup rewrite portable state while the
/// helper can still roll the executable back to a version that may not understand that state.
#[test]
fn resumed_update_health_failure_stops_before_storage() {
    let startup = code_only(&read_src("startup.rs"));
    let update = code_only(&read_src("update.rs"));
    assert!(
        startup.contains("crate::update::acknowledge_healthy(startup_update.as_ref())?;")
            && update.contains(
                "pub(crate) fn acknowledge_healthy(receipt: Option<&StartupUpdate>) -> anyhow::Result<()>"
            )
            && !update.contains("update healthy acknowledgement failed"),
        "health-marker errors must abort startup before any portable storage access"
    );
    let acknowledge = braced_body(
        &update,
        "pub(crate) fn acknowledge_healthy(receipt: Option<&StartupUpdate>) -> anyhow::Result<()>",
    );
    assert!(
        acknowledge.find("write_marker(").unwrap()
            < acknowledge
                .find("schedule_completed_staging_cleanup(")
                .unwrap(),
        "staging cleanup may start only after the durable acceptance marker exists"
    );
}

/// Propagating any post-ready error out of the helper would leave no process running; accepting a
/// target without image/path binding would let a forged manifest address another executable.
#[test]
fn helper_owns_liveness_and_binds_both_process_images() {
    let update = code_only(&read_src("update.rs"));
    let helper = braced_body(
        &update,
        "fn run_helper(manifest_path: &Path, nonce: &str) -> anyhow::Result<()>",
    );
    let recovery = braced_body(&update, "fn recover_viable_target(");
    let install = braced_body(&update, "fn install_after_parent_exit(");
    let identity = braced_body(
        &update,
        "fn validate_helper_identity(transaction: &UpdateTransaction) -> anyhow::Result<()>",
    );
    let parent = braced_body(
        &update,
        "fn open_parent(pid: u32, target: &Path) -> anyhow::Result<ParentHandle>",
    );
    assert!(
        helper.contains("let result = install_after_parent_exit")
            && helper.contains("wait_parent(parent)?")
            && helper.contains("recover_viable_target(&transaction, manifest_path)")
            && recovery.contains("restore_backup(&transaction.target, &transaction.backup)?")
            && recovery.contains("transaction.original_size")
            && recovery.contains("--moonterminal-update-recovery")
            && identity.contains("same_file_path(&current, &transaction.staged)")
            && identity.contains("transaction.expected_sha256")
            && parent.contains("QueryFullProcessImageNameW")
            && parent.contains("same_file_path(&image, target)"),
        "every post-ready failure must recover a verified old target and both process images must be bound"
    );
    assert!(
        install.contains("if healthy.is_ok()")
            && install.find("cleanup_accepted_transaction(").unwrap()
                < install.find("terminate_child(&mut child)").unwrap()
            && install.find("terminate_child(&mut child)").unwrap()
                < install.find("Err(error)").unwrap(),
        "only a healthy child may retire rollback authority; every failed child must be stopped"
    );
    assert!(
        !update.contains("PARENT_EXIT_TIMEOUT")
            && update.contains("WaitForSingleObject(handle, u32::MAX)"),
        "once ready is published, the helper must preserve the old process until shutdown completes"
    );
}

/// A planted incoming link must not be followed, and backup cleanup cannot race the helper's
/// health read as it did when a timer owned cleanup.
#[test]
fn helper_uses_exclusive_mutation_files_and_consumes_health_before_cleanup() {
    let update = code_only(&read_src("update.rs"));
    let copy = braced_body(
        &update,
        "fn copy_exclusive(source: &Path, destination: &Path)",
    );
    let install = braced_body(&update, "fn install_after_parent_exit(");
    assert!(
        copy.contains(".create_new(true)")
            && install.find("wait_for_marker_or_exit(").unwrap()
                < install.find("cleanup_accepted_transaction(").unwrap()
            && !update.contains("schedule_healthy_cleanup")
            && !update.contains("sleep(Duration::from_secs(3))"),
        "incoming must be exclusive and only the helper that consumed health may retire backup"
    );
    let staging_cleanup = braced_body(&update, "fn schedule_completed_staging_cleanup(");
    let remove_plain = braced_body(&update, "fn remove_plain_file(path: &Path)");
    assert!(
        staging_cleanup.contains("remove_plain_file(&staged)")
            && remove_plain.contains("fs::symlink_metadata(path)")
            && remove_plain.contains("!metadata.file_type().is_symlink()")
            && staging_cleanup.contains("fs::remove_dir(&directory)"),
        "the relaunched app must remove only the plain staged helper and its now-empty directory"
    );
}
