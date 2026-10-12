//! Face selection and the small pure helpers of the Summary state faces.
//!
//! Explicit imports, never `use super::*`: the parent re-exports `gpui::*`, whose own `test`
//! would shadow the built-in `#[test]` attribute.

use super::{Face, as_of_text, face_of, period_days};
use crate::load_state::Note;
use moon_core::db::{FailCode, FailKind};

/// A classified failure note of `kind`.
fn failed(kind: FailKind) -> Note {
    Note::Failed {
        msg: "x".into(),
        kind,
        path: "data/reports.sqlite".into(),
        operation: "summary".into(),
        code: FailCode::None,
    }
}

/// Every load outcome maps to exactly one face: loading to the skeleton, an empty period to the
/// empty face, a read failure to the error face except access denial, which keeps the shared
/// recovery alert, and the remaining notes to the shared note.
#[test]
fn face_follows_the_load_outcome() {
    assert_eq!(face_of::<()>(&Ok(())), Face::Data);
    assert_eq!(face_of::<()>(&Err(Note::Loading)), Face::Skeleton);
    assert_eq!(face_of::<()>(&Err(Note::Empty)), Face::Empty);
    for kind in [
        FailKind::Busy,
        FailKind::Exhausted,
        FailKind::Corrupt,
        FailKind::Other,
    ] {
        assert_eq!(face_of::<()>(&Err(failed(kind))), Face::Error, "{kind:?}");
    }
    assert_eq!(
        face_of::<()>(&Err(failed(FailKind::ReplicaAccessDenied))),
        Face::Note
    );
    for note in [
        Note::NotReady,
        Note::IncomparableQuote,
        Note::PeriodOutOfRange,
    ] {
        assert_eq!(face_of::<()>(&Err(note)), Face::Note);
    }
}

/// A bounded range counts its days (a partial day rounds up); an unbounded one has none.
#[test]
fn period_days_counts_bounded_ranges_only() {
    assert_eq!(period_days((0, 30 * 86_400)), Some(30));
    assert_eq!(period_days((86_400, 86_400 + 3_600)), Some(1));
    assert_eq!(period_days((-1, 86_400)), None);
    assert_eq!(period_days((100, 100)), None);
}

/// The banner clock prints the snapshot time in the display zone.
#[test]
fn as_of_prints_the_zone_clock() {
    assert_eq!(as_of_text(1_800_000_000, chrono_tz::UTC), "08:00");
    assert_eq!(as_of_text(1_800_000_000, chrono_tz::Europe::Kyiv), "10:00");
}

/// A 25-hour fall-back civil day is one day, and a 23-hour spring-forward month still counts 30:
/// a plain ceiling would title the Today empty face "2 days" on the DST change.
#[test]
fn period_days_tolerates_one_dst_hour() {
    assert_eq!(period_days((0, 90_000)), Some(1));
    assert_eq!(period_days((0, 30 * 86_400 - 3_600)), Some(30));
    assert_eq!(period_days((0, 30 * 86_400 + 3_600)), Some(30));
}
