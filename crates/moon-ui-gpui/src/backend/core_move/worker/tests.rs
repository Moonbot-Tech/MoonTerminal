//! How the move's thread turns the plan's snapshot steps into the settings row and the log.

use std::sync::mpsc;
use std::time::Duration;

use moon_remote::core_move::MoveStep;

use super::Progress;
use crate::backend::core_move::Event;
use crate::backend::core_move::steps::{StepEvent, StepId, StepNote, StepState, Steps};

/// The settings row counts the snapshot as it arrives, with no log line per count — a line a
/// second would push the move's real lines out of its capped log — and goes back to the rest of
/// the step once the snapshot is in, which is said once in the log (#1419).
#[test]
fn the_snapshot_count_rides_the_settings_row_not_the_log() {
    let _locale = crate::test_locale::force("en");
    let (tx, rx) = mpsc::channel();
    let progress = Progress::new(&tx);
    let mut steps = Steps::default();
    let mut lines = Vec::new();
    let drain = |steps: &mut Steps, lines: &mut Vec<String>| {
        for event in rx.try_iter() {
            match event {
                Event::Step(step, at) => steps.apply(step, at),
                Event::Line(line) => lines.push(line),
                _ => panic!("only steps and lines are reported here"),
            }
        }
    };

    progress.saw(&MoveStep::SnapshotReceiving { bytes: 131_072 });
    progress.saw(&MoveStep::SnapshotReceiving { bytes: 3_145_728 });
    drain(&mut steps, &mut lines);
    let row = &steps.rows()[0];
    assert_eq!(row.id, StepId::Settings);
    assert_eq!(row.state, StepState::Running);
    assert_eq!(
        row.note,
        Some(StepNote::ReceivingSettings { bytes: 3_145_728 })
    );
    assert!(lines.is_empty(), "{lines:?}");

    progress.saw(&MoveStep::SnapshotTaken {
        bytes: 3_200_000,
        exchange: Some("Binance".into()),
    });
    drain(&mut steps, &mut lines);
    assert_eq!(steps.rows()[0].state, StepState::Running);
    assert_eq!(steps.rows()[0].note, Some(StepNote::TakingSettings));
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(lines[0].contains("3.1"), "{}", lines[0]);
    // A failure after it still lands on the settings row.
    steps.apply(
        StepEvent::Failed {
            step: progress.current.get(),
            reason: "x".into(),
        },
        Duration::ZERO,
    );
    assert_eq!(steps.rows()[0].state, StepState::Failed);
}
