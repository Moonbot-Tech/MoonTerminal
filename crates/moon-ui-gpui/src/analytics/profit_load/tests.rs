//! Profit load-state publication tests.

use std::sync::Arc;

use moon_core::db::{FailKind, ProfitScope, ProfitUnit, QuoteBreakdown, ReadFail};

use super::ProfitLoadState;
use crate::load_state::Note;

/// A fresh query must remain visibly in flight without publishing stale scalar, unit, or split
/// data before its first database result arrives.
#[test]
fn fresh_state_maps_loading_without_any_payload() {
    let state = ProfitLoadState::<u64>::default();

    assert!(matches!(&state, ProfitLoadState::Loading));
    assert!(matches!(state.view(|_| false), Err(Note::Loading)));
    assert!(state.data().is_none());
    assert_eq!(state.unit(), None);
    assert!(state.split().is_none());
}

/// Dropping the explicit comparable unit would make a percent payload render as quote money.
#[test]
fn comparable_retains_scalar_payload_and_unit() {
    let mut state = ProfitLoadState::default();

    state.apply(Ok(ProfitScope::Comparable {
        unit: ProfitUnit::Percent,
        data: 37_u64,
    }));

    assert_eq!(state.unit(), Some(ProfitUnit::Percent));
    assert_eq!(state.data().map(|data| **data), Some(37));
    assert!(matches!(state.view(|_| false), Ok(data) if **data == 37));
    assert!(state.split().is_none());
}

/// Treating a successful empty query as unavailable would replace the empty-period note with a
/// synchronization warning, while assigning it a unit would imply a currency the query never saw.
#[test]
fn empty_has_scalar_data_without_a_unit() {
    let mut state = ProfitLoadState::default();

    state.apply(Ok(ProfitScope::Empty(Vec::<u8>::new())));

    assert!(state.data().is_some());
    assert_eq!(state.unit(), None);
    assert!(matches!(state.view(Vec::is_empty), Err(Note::Empty)));
    assert!(state.split().is_none());
}

/// Replacing Split with an empty scalar silently discards quote buckets and presents mixed money
/// as a legitimate no-trades result.
#[test]
fn split_retains_the_complete_breakdown_without_scalar_data() {
    let expected =
        QuoteBreakdown::from_groups([(Some(1), 12.5, 2), (Some(8), -3.0, 1), (None, 99.0, 4)]);
    let mut state = ProfitLoadState::<u64>::default();

    state.apply(Ok(ProfitScope::Split(expected.clone())));

    assert_eq!(state.split(), Some(&expected));
    assert!(state.data().is_none());
    assert_eq!(state.unit(), None);
    assert!(matches!(
        state.view(|_| false),
        Err(Note::IncomparableQuote)
    ));
}

/// Mapping NotReady to Empty would claim a period has no trades before the report replica can
/// answer the query.
#[test]
fn not_ready_remains_distinct_from_empty() {
    let mut state = ProfitLoadState::<u64>::default();

    state.apply(Err(ReadFail::NotReady));

    assert!(matches!(&state, ProfitLoadState::NotReady));
    assert!(matches!(state.view(|_| true), Err(Note::NotReady)));
    assert!(state.data().is_none());
    assert_eq!(state.unit(), None);
}

/// IncomparableQuote is a healthy safety boundary and must keep the dedicated split-money note
/// instead of becoming a generic database failure.
#[test]
fn incomparable_read_uses_the_existing_quote_note() {
    let mut state = ProfitLoadState::<u64>::default();

    state.apply(Err(ReadFail::IncomparableQuote));

    assert!(matches!(
        &state,
        ProfitLoadState::Failed(ReadFail::IncomparableQuote)
    ));
    assert!(matches!(
        state.view(|_| false),
        Err(Note::IncomparableQuote)
    ));
}

/// Database failure kinds and their originating messages select different existing guidance; a
/// generic remap would tell users to retry corruption or repair ordinary I/O contention.
#[test]
fn failed_reads_retain_every_failure_kind_and_message() {
    for (kind, message) in [
        (FailKind::Busy, "reports busy"),
        (FailKind::Corrupt, "broken analytics database"),
        (FailKind::Other, "reports unavailable"),
    ] {
        let mut state = ProfitLoadState::<u64>::default();
        state.apply(Err(ReadFail::Failed {
            kind,
            msg: Arc::from(message),
        }));

        assert!(matches!(
            &state,
            ProfitLoadState::Failed(ReadFail::Failed {
                kind: actual,
                msg,
            }) if *actual == kind && msg.as_ref() == message
        ));
        assert!(matches!(
            state.view(|_| false),
            Err(Note::Failed {
                kind: actual,
                msg,
            }) if actual == kind && msg.as_ref() == message
        ));
    }
}
