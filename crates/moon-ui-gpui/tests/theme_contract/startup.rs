//! Static startup contracts for persistent background work in the binary-only UI crate.

use super::support::*;

/// Removing startup reconciliation, the setter's dirty write, or the debounced layout flush would
/// leave first-run and upgraded profiles on UTC after the next reboot even though detection worked.
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
            && startup.contains("if b.layout_dirty {")
            && startup.contains("b.layout.save();"),
        "normal startup must detect, store, dirty, and flush the exact IANA zone for the next reboot"
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
