//! Uniform-column decisions. A wrong inclusion puts a column back on the table, or hides a
//! column that one core disagrees with.

use moon_core::feed::{ConnFault, ConnFaultKind, ConnStatus, CoreTimeOffsetStatus};
use moon_core::session::core_update::{CoreUpdateOutcome, CoreUpdatePhase, UpdateFailure};
use moon_core::session::{CoreStartupStatus, CoreSysStatus};

use super::uniform_columns;
use crate::backend::core_warn::LatencySeverity;
use crate::panels::core_status::model::{ApiKeyState, CoreStatusRow};

fn row(name: &str, status: ConnStatus) -> CoreStatusRow {
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

/// Fill the five candidate fields with one shared reported value.
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

/// Two cores that report the same five values collapse all five, in line order, and the text
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
            ("version", "7.71 R3"),
        ]
    );
}

/// One core with a different clock brings only that column back. The other four stay on the
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
    assert_eq!(keys, vec!["cpus", "api_key", "api_quota", "version"]);
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

/// A missing suffix and a blank suffix are the same build the cell prints. A different letter
/// is a different build, even on the same number.
#[test]
fn version_agreement_follows_the_printed_build_not_a_raw_suffix() {
    let mut bare = reporting("CORE-A1");
    let mut blank = reporting("CORE-A2");
    bare.server_version_suffix = None;
    blank.server_version_suffix = Some("  ".to_string());
    let facts = uniform_columns(&[bare, blank]);
    let version = facts
        .iter()
        .find(|fact| fact.key == "version")
        .expect("version");
    assert_eq!(version.text, "7.71");

    let mut letter = reporting("CORE-A1");
    let mut other = reporting("CORE-A2");
    letter.server_version_suffix = Some(" R3 ".to_string());
    other.server_version_suffix = Some("R2".to_string());
    assert!(
        uniform_columns(&[letter, other])
            .iter()
            .all(|fact| fact.key != "version")
    );
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

/// Unknown is not a third API-key value. Two perpetual keys agree on the infinity glyph; a
/// dated key beside them brings the column back.
#[test]
fn api_key_ignores_unknown_and_compares_the_classified_state() {
    let mut a = row("CORE-A1", ConnStatus::Ready);
    let mut b = row("CORE-A2", ConnStatus::Ready);
    let unknown = row("CORE-A3", ConnStatus::Ready);
    a.api_key = ApiKeyState::Perpetual;
    b.api_key = ApiKeyState::Perpetual;
    let facts = uniform_columns(&[a.clone(), b, unknown]);
    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0].key, "api_key");
    assert_eq!(facts[0].text, "\u{221e}");

    let mut dated = row("CORE-A2", ConnStatus::Ready);
    dated.api_key = ApiKeyState::Days(1);
    assert!(uniform_columns(&[a, dated]).is_empty());
}
