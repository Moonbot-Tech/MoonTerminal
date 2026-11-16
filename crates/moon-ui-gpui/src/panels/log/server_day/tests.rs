use std::time::Instant;

use moon_remote::core_cmds::LogDay;

use super::{DayFailure, DayMenu, DayRow, day_menu, megabytes};
use crate::backend::core_cmds::{CmdError, CmdPhase, DayLog, LogPull, LogPullOutcome};

/// Two server days, the newer one first, as the backend keeps them.
fn two_days() -> Vec<LogDay> {
    vec![
        LogDay {
            date: "2026-01-03".into(),
            bytes: 1_048_576,
        },
        LogDay {
            date: "2026-01-02".into(),
            bytes: 52_428_800,
        },
    ]
}

/// A command that is under way.
fn running() -> CmdPhase {
    CmdPhase::Running {
        since: Instant::now(),
        seen: None,
    }
}

/// The rows of a menu that lists days.
fn rows(menu: DayMenu) -> Vec<DayRow> {
    match menu {
        DayMenu::Days { rows, .. } => rows,
        other => panic!("expected a day list, got {other:?}"),
    }
}

/// Without a login nothing else may show: a cached listing would offer days the terminal can no
/// longer fetch, and the row that adds the login would be missing.
#[test]
fn a_missing_login_outranks_everything_cached() {
    let known = DayLog {
        days: Some(two_days()),
        ..DayLog::default()
    };
    assert_eq!(
        day_menu(false, Some(&CmdPhase::Done), &known, &[], "CORE_A"),
        DayMenu::NoLogin
    );
}

/// The first open has nothing to list yet; an empty menu would look like "the server has no logs".
#[test]
fn the_first_answer_is_waited_for() {
    let nothing = DayLog::default();
    assert_eq!(
        day_menu(true, None, &nothing, &[], "CORE_A"),
        DayMenu::Asking
    );
    assert_eq!(
        day_menu(true, Some(&running()), &nothing, &[], "CORE_A"),
        DayMenu::Asking
    );
}

/// A stored day is marked by ITS OWN core's pulled file — not by the live copy of that day, and not
/// by another core's pull.
#[test]
fn only_this_cores_pulled_file_marks_a_day_stored() {
    let known = DayLog {
        days: Some(two_days()),
        ..DayLog::default()
    };
    let files = [
        "2026-01-03_CORE_A.log".to_string(),
        "2026-01-02_CORE_A.server.log".to_string(),
        "2026-01-03_CORE_B.server.log".to_string(),
    ];
    let listed = rows(day_menu(
        true,
        Some(&CmdPhase::Done),
        &known,
        &files,
        "CORE_A",
    ));
    assert_eq!(
        listed,
        [
            DayRow {
                date: "2026-01-03".into(),
                bytes: 1_048_576,
                stored: false,
                pulling: false,
            },
            DayRow {
                date: "2026-01-02".into(),
                bytes: 52_428_800,
                stored: true,
                pulling: false,
            },
        ]
    );
}

/// The day being pulled is marked for as long as the pull itself is under way — also after a
/// listing queued ahead of it has reported the command done — and not a moment after it lands: a
/// finished pull that still read "pulling" would keep every row disabled for good.
#[test]
fn a_day_reads_as_pulling_exactly_while_its_pull_runs() {
    let with = |outcome| DayLog {
        days: Some(two_days()),
        pull: Some(LogPull {
            date: "2026-01-02".into(),
            outcome,
        }),
        ..DayLog::default()
    };
    for phase in [running(), CmdPhase::Done] {
        let pulling = with(LogPullOutcome::Running);
        let during = rows(day_menu(true, Some(&phase), &pulling, &[], "CORE_A"));
        assert_eq!(
            during.iter().map(|row| row.pulling).collect::<Vec<_>>(),
            [false, true]
        );
    }
    let landed = with(LogPullOutcome::Saved {
        file: "2026-01-02_CORE_A.server.log".into(),
    });
    let after = rows(day_menu(
        true,
        Some(&CmdPhase::Done),
        &landed,
        &[],
        "CORE_A",
    ));
    assert!(after.iter().all(|row| !row.pulling));
}

/// A day that vanished between the listing and the pull must be named, with the list still there
/// to pick another.
#[test]
fn an_absent_day_is_named_above_the_list() {
    let absent = DayLog {
        days: Some(two_days()),
        pull: Some(LogPull {
            date: "2026-01-01".into(),
            outcome: LogPullOutcome::Absent,
        }),
        ..DayLog::default()
    };
    let DayMenu::Days { rows, absent } =
        day_menu(true, Some(&CmdPhase::Done), &absent, &[], "CORE_A")
    else {
        panic!("the list must stay");
    };
    assert_eq!(absent.as_deref(), Some("2026-01-01"));
    assert_eq!(rows.len(), 2);
}

/// Each failure keeps its own wording: "the server is down" and "no core on that port" send the
/// user to different places.
#[test]
fn a_failure_replaces_the_list_with_its_own_reason() {
    let known = DayLog {
        days: Some(two_days()),
        ..DayLog::default()
    };
    for (error, expected) in [
        (CmdError::Ssh("refused".into()), DayFailure::Server),
        (CmdError::Timeout, DayFailure::Server),
        (CmdError::NoCore { port: 3302 }, DayFailure::NoCore(3302)),
        (
            CmdError::Refused("the log arrived damaged".into()),
            DayFailure::Refused("the log arrived damaged".into()),
        ),
    ] {
        assert_eq!(
            day_menu(true, Some(&CmdPhase::Failed(error)), &known, &[], "CORE_A"),
            DayMenu::Failed(expected)
        );
    }
}

/// A small day must not read as "0 MB", and a large one needs no decimals.
#[test]
fn sizes_keep_one_decimal_only_below_ten_megabytes() {
    assert_eq!(megabytes(0), "0.0");
    assert_eq!(megabytes(104_858), "0.1");
    assert_eq!(megabytes(9 * 1024 * 1024), "9.0");
    assert_eq!(megabytes(52_428_800), "50");
}
