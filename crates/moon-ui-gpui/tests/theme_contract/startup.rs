//! Static startup contracts for persistent background work in the binary-only UI crate.

use super::support::*;

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
