//! Uniform-column decisions. A wrong inclusion puts a column back on the table, or hides a
//! column that one core disagrees with.

use moon_core::feed::{ConnFault, ConnFaultKind, ConnStatus, CoreTimeOffsetStatus};
use moon_core::session::core_update::{CoreUpdateOutcome, CoreUpdatePhase, UpdateFailure};
use moon_core::session::{CoreStartupStatus, CoreSysStatus};

use super::uniform_columns;
use crate::backend::core_warn::LatencySeverity;
use crate::panels::core_status::model::{ApiKeyState, CoreStatusRow};

fn row(name: &str, status: ConnStatus) -> CoreStatusRow {
    // The facts translate "no expiry" and "not set"; the first lookup must not run on the
    // 2 MiB test stack.
    crate::test_locale::warm();
    CoreStatusRow {
        id: 1,
        name: name.to_string(),
        status,
        sys: CoreSysStatus::default(),
        startup: CoreStartupStatus::default(),
        time_offset: CoreTimeOffsetStatus::default(),
        fault: None,
        mode_suggestion: None,
        endpoint: None,
        ping_warn: false,
        ping_sev: LatencySeverity::Normal,
        exch_sev: LatencySeverity::Normal,
        api_key: ApiKeyState::Unknown,
        api_warn: false,
        api_notice: false,
        api_quota: None,
        api_quota_warn: false,
        server_version: None,
        server_version_suffix: None,
        version_behind: None,
        update: None,
    }
}

fn fault() -> ConnFault {
    ConnFault {
        kind: ConnFaultKind::ConnectTimedOut { timeout_ms: 15_000 },
        identity: Default::default(),
        startup: CoreStartupStatus::default(),
    }
}

/// Fill the candidate fields and the build with shared reported values.
fn reporting(name: &str) -> CoreStatusRow {
    let mut row = row(name, ConnStatus::Ready);
    row.time_offset.offset_secs = Some(7_200);
    row.sys.logical_cpu_count = Some(2);
    row.api_key = ApiKeyState::Days(45);
    row.api_quota = Some(1_000);
    row.server_version = Some(771);
    row.server_version_suffix = Some("R3".to_string());
    row
}

/// No rows means there is nothing to agree on. Emitting a fact would print "same for all"
/// above an empty table.
#[test]
fn empty_rows_emit_no_facts() {
    assert!(uniform_columns(&[]).is_empty());
}

/// One reporting core is not a consensus. The column has to stay, or a second core that
/// later disagrees would find the column already hidden.
#[test]
fn a_single_known_value_is_not_uniform() {
    let rows = [reporting("CORE-A1"), row("CORE-A2", ConnStatus::Ready)];
    assert!(uniform_columns(&rows).is_empty());
}

/// Two cores that report the same candidate values collapse them in line order, and the text
/// is the cell text. A blank third core does not count as a disagreement.
#[test]
fn each_candidate_is_detected_and_unknown_rows_are_ignored() {
    let rows = [
        reporting("CORE-A1"),
        reporting("CORE-A2"),
        row("CORE-A3", ConnStatus::Ready),
    ];
    let facts = uniform_columns(&rows);
    let got: Vec<(&str, &str)> = facts
        .iter()
        .map(|fact| (fact.key, fact.text.as_str()))
        .collect();
    assert_eq!(
        got,
        vec![
            ("tz_off", "UTC+02:00"),
            ("cpus", "2"),
            ("api_key", "45"),
            ("api_quota", "1000"),
        ]
    );
}

/// One core with a different clock brings only that column back. The other three stay on the
/// line, which is the whole point of comparing per column.
#[test]
fn a_differing_core_brings_that_column_back() {
    let mut other = reporting("CORE-A3");
    other.time_offset.offset_secs = Some(0);
    let rows = [reporting("CORE-A1"), reporting("CORE-A2"), other];
    let keys: Vec<&str> = uniform_columns(&rows)
        .into_iter()
        .map(|fact| fact.key)
        .collect();
    assert_eq!(keys, vec!["cpus", "api_key", "api_quota"]);
}

/// A measured zero is a real offset. Folding it into "never measured" would hide a UTC fleet
/// or, worse, treat UTC and "no reading" as the same fact.
#[test]
fn a_measured_zero_offset_is_a_known_value() {
    let mut a = row("CORE-A1", ConnStatus::Ready);
    let mut b = row("CORE-A2", ConnStatus::Ready);
    a.time_offset.offset_secs = Some(0);
    b.time_offset.offset_secs = Some(0);
    let facts = uniform_columns(&[a, b]);
    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0].key, "tz_off");
    assert_eq!(facts[0].text, "UTC+00:00");
}

/// An offline core renders dashes. Its retained CPU count must not veto a value the live
/// cores agree on, and two offline cores that happen to agree are not two reporting cores.
#[test]
fn offline_retained_values_are_not_reports() {
    let mut live_a = row("CORE-A1", ConnStatus::Ready);
    let mut live_b = row("CORE-A2", ConnStatus::Ready);
    live_a.sys.logical_cpu_count = Some(16);
    live_b.sys.logical_cpu_count = Some(16);
    let mut dropped = row("CORE-A3", ConnStatus::Disconnected);
    dropped.sys.logical_cpu_count = Some(4);
    let facts = uniform_columns(&[live_a, live_b, dropped]);
    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0].key, "cpus");
    assert_eq!(facts[0].text, "16");

    let mut failed_a = row("CORE-A1", ConnStatus::Failed(String::new()));
    let mut failed_b = row("CORE-A2", ConnStatus::Failed(String::new()));
    failed_a.sys.logical_cpu_count = Some(16);
    failed_b.sys.logical_cpu_count = Some(16);
    assert!(uniform_columns(&[failed_a, failed_b]).is_empty());
}

/// A core that is still connecting is not the offline dash, so its CPU counts toward
/// agreement. A Ready core that still holds the previous attempt's fault counts too.
#[test]
fn connecting_still_reports_and_a_ready_fault_does_not_erase_the_value() {
    let mut live = row("CORE-A1", ConnStatus::Ready);
    let mut connecting = row("CORE-A2", ConnStatus::Connecting);
    live.sys.logical_cpu_count = Some(8);
    connecting.sys.logical_cpu_count = Some(8);
    let agreed = uniform_columns(&[live.clone(), connecting]);
    assert_eq!(agreed.len(), 1);
    assert_eq!(agreed[0].text, "8");

    let mut different = row("CORE-A3", ConnStatus::Connecting);
    different.sys.logical_cpu_count = Some(4);
    assert!(uniform_columns(&[live.clone(), different]).is_empty());

    let mut ready_fault = row("CORE-A2", ConnStatus::Ready);
    ready_fault.sys.logical_cpu_count = Some(8);
    ready_fault.fault = Some(fault());
    let facts = uniform_columns(&[live, ready_fault]);
    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0].text, "8");
}

/// Matching builds must keep their per-core column at narrow and wide widths, including bare
/// builds and named builds. Re-adding version to the uniform candidates hides this column.
#[test]
fn matching_builds_keep_the_version_column() {
    use std::collections::{HashMap, HashSet};

    use crate::panels::core_status::table::projected_column_keys;

    for suffix in [None, Some(""), Some("R3")] {
        let mut rows = [reporting("CORE-A1"), reporting("CORE-A2")];
        for row in &mut rows {
            row.server_version_suffix = suffix.map(str::to_string);
        }
        let facts = uniform_columns(&rows);
        let uniform: Vec<&str> = facts.iter().map(|fact| fact.key).collect();
        assert!(!uniform.contains(&"version"));
        for width in [300.0, 720.0, 1920.0] {
            let keys = projected_column_keys(
                &rows,
                &HashSet::new(),
                &uniform,
                &HashMap::new(),
                width,
                &HashMap::new(),
            );
            assert!(keys.contains(&"version"), "{suffix:?} at {width}px");
        }
    }
}

/// One failed attempt keeps the version column, even when every core reports the same build.
/// Hiding it would remove the only retry for that attempt. The other agreed columns stay on
/// the line.
#[test]
fn a_failed_update_keeps_version_visible() {
    let mut failed = reporting("CORE-A1");
    failed.update = Some(CoreUpdatePhase::Done(CoreUpdateOutcome::Failed(
        UpdateFailure::Timeout,
    )));
    let keys: Vec<&str> = uniform_columns(&[failed, reporting("CORE-A2")])
        .into_iter()
        .map(|fact| fact.key)
        .collect();
    assert!(!keys.contains(&"version"));
    assert_eq!(keys, vec!["tz_off", "cpus", "api_key", "api_quota"]);
}

/// Unknown is not a third API-key value. Two perpetual keys agree, and the line says "no
/// expiry" rather than the bare infinity glyph; a dated key beside them brings the column back.
#[test]
fn api_key_ignores_unknown_and_compares_the_classified_state() {
    // The expected text is translated; hold one locale for the whole test.
    let _locale = crate::test_locale::force("en");
    let mut a = row("CORE-A1", ConnStatus::Ready);
    let mut b = row("CORE-A2", ConnStatus::Ready);
    let unknown = row("CORE-A3", ConnStatus::Ready);
    a.api_key = ApiKeyState::Perpetual;
    b.api_key = ApiKeyState::Perpetual;
    let facts = uniform_columns(&[a.clone(), b, unknown]);
    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0].key, "api_key");
    assert_eq!(
        facts[0].text,
        rust_i18n::t!("core_status.same.no_expiry").to_string()
    );

    let mut dated = row("CORE-A2", ConnStatus::Ready);
    dated.api_key = ApiKeyState::Days(1);
    assert!(uniform_columns(&[a, dated]).is_empty());
}

/// The key cell keeps its text on a dropped core, so a dated key there is on screen and keeps
/// the column, while a dropped core with the same perpetual key still agrees.
#[test]
fn api_key_compares_what_a_dropped_core_still_displays() {
    let mut a = row("CORE-A1", ConnStatus::Ready);
    let mut b = row("CORE-A2", ConnStatus::Ready);
    a.api_key = ApiKeyState::Perpetual;
    b.api_key = ApiKeyState::Perpetual;
    let mut dropped = row("CORE-A3", ConnStatus::Disconnected);
    dropped.api_key = ApiKeyState::Days(5);
    let keys = |rows: &[CoreStatusRow]| -> Vec<&'static str> {
        uniform_columns(rows)
            .into_iter()
            .map(|fact| fact.key)
            .collect()
    };
    assert!(!keys(&[a.clone(), b.clone(), dropped.clone()]).contains(&"api_key"));
    dropped.api_key = ApiKeyState::Perpetual;
    assert!(keys(&[a, b, dropped]).contains(&"api_key"));
}

/// Answered cores that publish no quota put "not set" on the line, so the vanished quota column
/// is explained. Cores that have not answered yet say nothing about a quota.
#[test]
fn an_absent_quota_on_answered_cores_reads_not_set() {
    // The expected text is translated; hold one locale for the whole test.
    let _locale = crate::test_locale::force("en");
    let mut a = reporting("CORE-A1");
    let mut b = reporting("CORE-A2");
    a.api_quota = None;
    b.api_quota = None;
    let facts = uniform_columns(&[a.clone(), b.clone()]);
    let quota = facts
        .iter()
        .find(|fact| fact.key == "api_quota")
        .expect("quota fact");
    assert_eq!(
        quota.text,
        rust_i18n::t!("core_status.same.not_set").to_string()
    );

    let silent = row("CORE-A2", ConnStatus::Ready);
    assert!(
        uniform_columns(&[a.clone(), silent])
            .iter()
            .all(|fact| fact.key != "api_quota"),
        "one answered core is not a consensus"
    );
    let mut quoted = reporting("CORE-A3");
    quoted.api_quota = Some(10);
    assert!(
        uniform_columns(&[a, b, quoted])
            .iter()
            .all(|fact| fact.key != "api_quota"),
        "a core with a quota keeps the column"
    );
}
