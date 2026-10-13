//! Health-strip membership. A chip that drifts from `row_matches` shows a count the filter
//! cannot reproduce, and a 24-hour boundary that is off by one millisecond hides or invents
//! an alarm.

use moon_core::feed::{ConnFault, ConnFaultKind, ConnStatus, CoreTimeOffsetStatus};
use moon_core::session::{CoreStartupStatus, CoreSysStatus};

use super::{HealthCounts, HealthFilter, Tone, Verdict, health_counts, row_matches, verdict};
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

fn counts_of(rows: &[CoreStatusRow], filter: HealthFilter) -> usize {
    rows.iter().filter(|row| row_matches(row, filter)).count()
}

/// One failed core is a bad strip: the offline chip is 1, attention is that same core, and
/// it is not online. Zero on the other chips must stay a stored 0.
#[test]
fn one_offline_core_is_bad() {
    let rows = [row("CORE-A1", ConnStatus::Failed(String::new()))];
    let counts = health_counts(&rows, &[], 1_700_000_000_000);
    assert_eq!(
        counts,
        HealthCounts {
            offline_failed: 1,
            ping_high: 0,
            version_behind: 0,
            api_expiring: 0,
            alarms_24h: 0,
        }
    );
    assert_eq!(
        verdict(&rows, &counts, 2),
        Verdict {
            tone: Tone::Bad,
            attention: 1,
            total: 1,
            online: 0,
            servers: 2,
        }
    );
}

/// A core that is still connecting, and has no fault, is not "all online". The strip stays
/// a warning and attention covers at least the cores that are not Ready.
#[test]
fn a_connecting_core_with_no_fault_is_a_warning() {
    let rows = [
        row("CORE-A1", ConnStatus::Ready),
        row("CORE-A2", ConnStatus::Connecting),
    ];
    let counts = health_counts(&rows, &[], 0);
    assert_eq!(counts.offline_failed, 0);
    let summary = verdict(&rows, &counts, 1);
    assert_eq!(summary.tone, Tone::Warn);
    assert_eq!(summary.online, 1);
    assert_eq!(summary.total, 2);
    assert!(summary.attention >= summary.total - summary.online);
}

/// Ping with nobody offline is a warning, not a failure. Painting it red would train the
/// operator to ignore the alarm colour.
#[test]
fn ping_alone_is_a_warning() {
    let mut core = row("CORE-A1", ConnStatus::Ready);
    core.ping_warn = true;
    let rows = [core];
    let counts = health_counts(&rows, &[], 0);
    assert_eq!(counts.offline_failed, 0);
    assert_eq!(counts.ping_high, 1);
    assert_eq!(verdict(&rows, &counts, 1).tone, Tone::Warn);
    assert_eq!(verdict(&rows, &counts, 1).online, 1);
}

/// A clean Ready fleet is Ok, including a core that still carries the previous attempt's
/// fault. That fault is cleared at Ready and must not keep the strip red.
#[test]
fn a_clean_fleet_is_ok_and_a_ready_fault_is_not_offline() {
    let mut stale = row("CORE-A1", ConnStatus::Ready);
    stale.fault = Some(fault());
    let rows = [stale, row("CORE-A2", ConnStatus::Ready)];
    let counts = health_counts(&rows, &[], 0);
    assert_eq!(counts.offline_failed, 0);
    assert!(!row_matches(&rows[0], HealthFilter::Offline));
    assert_eq!(
        verdict(&rows, &counts, 1),
        Verdict {
            tone: Tone::Ok,
            attention: 0,
            total: 2,
            online: 2,
            servers: 1,
        }
    );
}

/// No rows and no episodes report zeros. Dropping a zero would make the strip look like the
/// count had not been computed.
#[test]
fn nothing_to_count_is_zero() {
    let counts = health_counts(&[], &[], 1_700_000_000_000);
    assert_eq!(
        counts,
        HealthCounts {
            offline_failed: 0,
            ping_high: 0,
            version_behind: 0,
            api_expiring: 0,
            alarms_24h: 0,
        }
    );
    assert_eq!(verdict(&[], &counts, 0).tone, Tone::Ok);
}

/// The open-closed 24-hour window. Including the exact far edge recounts yesterday's alarm;
/// excluding `now` hides an episode that started this millisecond.
#[test]
fn alarms_count_only_inside_the_open_24h_window() {
    let day: i64 = 24 * 60 * 60 * 1000;
    let now = 1_700_000_000_000;
    let starts = [now - day, now - day + 1, now, now + 1, now - day - 1];
    assert_eq!(health_counts(&[], &starts, now).alarms_24h, 2);
}

/// Yesterday's alarms do not turn a clean fleet into a warning. Tone comes from cores, not
/// from the episode log.
#[test]
fn alarm_history_does_not_change_the_tone() {
    let rows = [row("CORE-A1", ConnStatus::Ready)];
    let now = 1_700_000_000_000;
    let counts = health_counts(&rows, &[now], now);
    assert_eq!(counts.alarms_24h, 1);
    assert_eq!(verdict(&rows, &counts, 1).tone, Tone::Ok);
}

/// Chip totals are `row_matches` totals, a core on two chips is one attention, and a
/// connecting retry that still holds its fault is offline while a connecting core with no
/// fault is not.
#[test]
fn chip_counts_follow_row_matches_and_attention_is_distinct_cores() {
    let mut failed = row("CORE-A1", ConnStatus::Failed(String::new()));
    failed.ping_warn = true;
    failed.version_behind = Some(771);
    let mut ping_and_version = row("CORE-A2", ConnStatus::Ready);
    ping_and_version.ping_warn = true;
    ping_and_version.version_behind = Some(770);
    let mut notice = row("CORE-A3", ConnStatus::Ready);
    notice.api_notice = true;
    let mut warn = row("CORE-A4", ConnStatus::Ready);
    warn.api_warn = true;
    let clean = row("CORE-A5", ConnStatus::Ready);
    let dropped = row("CORE-A6", ConnStatus::Disconnected);
    let mut retry = row("CORE-A7", ConnStatus::Connecting);
    retry.fault = Some(fault());
    let connecting = row("CORE-A8", ConnStatus::Stage("init".to_string()));
    let rows = [
        failed,
        ping_and_version,
        notice,
        warn,
        clean,
        dropped,
        retry,
        connecting,
    ];

    let counts = health_counts(&rows, &[], 0);
    assert_eq!(
        counts.offline_failed,
        counts_of(&rows, HealthFilter::Offline)
    );
    assert_eq!(counts.ping_high, counts_of(&rows, HealthFilter::Ping));
    assert_eq!(
        counts.version_behind,
        counts_of(&rows, HealthFilter::Version)
    );
    assert_eq!(
        counts.api_expiring,
        counts_of(&rows, HealthFilter::ApiExpiry)
    );
    assert_eq!(counts.offline_failed, 3);
    assert_eq!(counts.ping_high, 2);
    assert_eq!(counts.version_behind, 2);
    assert_eq!(counts.api_expiring, 2);
    assert!(!row_matches(&rows[7], HealthFilter::Offline));
    assert!(row_matches(&rows[6], HealthFilter::Offline));

    let summary = verdict(&rows, &counts, 4);
    assert_eq!(summary.tone, Tone::Bad);
    assert_eq!(summary.attention, 6);
    assert_eq!(summary.total, 8);
    assert_eq!(summary.online, 4);
    assert_eq!(summary.servers, 4);
}
