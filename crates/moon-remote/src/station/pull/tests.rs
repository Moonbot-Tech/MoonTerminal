//! Going on from where a station's tape answer stopped.

use moon_core::station_api::{TapeResume, TapeWant};

use super::*;

fn want(market: &str, spans: &[(i64, i64)]) -> TapeWant {
    TapeWant {
        exchange: "x".into(),
        market: market.into(),
        spans: spans.to_vec(),
    }
}

/// The next request starts at the resumed item, its stretches cut to the resumed millisecond;
/// what was answered before is not asked again.
#[test]
fn a_resumed_request_starts_where_the_answer_stopped() {
    let items = vec![
        want("A", &[(0, 10)]),
        want("B", &[(0, 10), (20, 30), (40, 50)]),
        want("C", &[(0, 5)]),
    ];
    let next = resume_from(
        &items,
        &TapeResume {
            item: 1,
            from_ms: 25,
        },
    );
    assert_eq!(
        next,
        vec![want("B", &[(25, 30), (40, 50)]), want("C", &[(0, 5)])]
    );
    // Past the item's last stretch: the item is done, the next one starts whole.
    let past = resume_from(
        &items,
        &TapeResume {
            item: 1,
            from_ms: 51,
        },
    );
    assert_eq!(past, vec![want("C", &[(0, 5)])]);
}

/// A market with more stretches than one request carries is asked in several, nothing lost.
#[test]
fn a_want_past_the_bounds_is_split() {
    let spans: Vec<_> = (0..(MAX_TAPE_SPANS as i64 + 3))
        .map(|i| (i * 10, i * 10 + 5))
        .collect();
    let split = bounded(vec![want("A", &spans), want("B", &[])]);
    assert_eq!(split.len(), 2);
    assert_eq!(split[0].spans.len(), MAX_TAPE_SPANS);
    assert_eq!(
        split
            .iter()
            .flat_map(|w| w.spans.clone())
            .collect::<Vec<_>>(),
        spans
    );
}
