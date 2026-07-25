//! Calendar result-publication tests.

use std::sync::Arc;

use moon_core::db::analytics::DayCell;
use moon_core::db::{FailKind, ReadFail};

use super::apply_calendar_results;
use crate::load_state::{LoadState, Note};

/// `calendar/mod.rs:apply_calendar_results` must apply both `ReadFail` values to their
/// `LoadState`; replacing either failure with `None` renders endless Loading or a false
/// "no previous period" KPI instead of the database error.
#[test]
fn current_and_previous_failures_remain_visible() {
    let failure = || ReadFail::Failed {
        kind: FailKind::Corrupt,
        msg: Arc::from("broken calendar"),
    };
    let mut days = LoadState::<Vec<DayCell>>::default();
    let mut previous = LoadState::<Option<(f64, i64, i64)>>::default();

    assert!(apply_calendar_results(
        &mut days,
        &mut previous,
        Err(failure()),
        true,
    ));

    assert!(matches!(
        days.view(|_| false),
        Err(Note::Failed {
            kind: FailKind::Corrupt,
            ..
        })
    ));
    assert!(matches!(
        previous.view(|_| false),
        Err(Note::Failed {
            kind: FailKind::Corrupt,
            ..
        })
    ));
}
