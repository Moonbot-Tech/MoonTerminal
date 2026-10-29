//! Parity of survivor selection against the frozen synchronous queue algorithm.

use std::collections::VecDeque;

use super::{Incoming, Selected, select};
use crate::panels::detects::tests::{detect_item, detect_row};
use crate::panels::detects::{DetectItem, MAX_DETECT_BTNS};

/// Frozen old replace-in-place search, full stable replay sort, then front eviction.
fn frozen<'a>(
    held: &VecDeque<DetectItem>,
    incoming: &[Incoming<'a>],
    replayed: bool,
) -> Vec<(Option<usize>, Option<u64>)> {
    let mut slots: Vec<(Option<usize>, Option<Incoming<'a>>, f64)> = held
        .iter()
        .enumerate()
        .map(|(i, item)| (Some(i), None, item.born_ms))
        .collect();
    for next in incoming {
        let found = slots.iter_mut().find(|(old, fresh, _)| match fresh {
            Some(fresh) => fresh.core == next.core && fresh.row.market == next.row.market,
            None => {
                let item = &held[old.expect("old card")];
                item.core() == Some(next.core) && item.market == next.row.market
            }
        });
        if let Some(slot) = found {
            if slot.2 == next.row.time_ms {
                continue;
            }
            slot.1 = Some(*next);
            slot.2 = next.row.time_ms;
        } else {
            slots.push((None, Some(*next), next.row.time_ms));
        }
    }
    if replayed {
        slots.sort_by(|a, b| a.2.total_cmp(&b.2));
    }
    while slots.len() > MAX_DETECT_BTNS {
        slots.remove(0);
    }
    slots
        .into_iter()
        .map(|(old, fresh, _)| (old, fresh.map(|v| v.row.seq)))
        .collect()
}

/// Summarize the actual retained positions and replacement row identities, independent of labels.
fn signature(slots: Vec<Selected<'_>>) -> Vec<(Option<usize>, Option<u64>)> {
    slots
        .into_iter()
        .map(|slot| (slot.held, slot.incoming.map(|v| v.row.seq)))
        .collect()
}

/// Changing top-N tie breaking, refresh positions or replay skipping would replace visible cards.
#[test]
fn survivors_match_frozen_queue_for_refresh_replay_ties_and_crowd() {
    let held: VecDeque<_> = (0..48)
        .map(|i| {
            detect_item(
                (i < 43).then_some(1),
                &format!("XALPHA{i}"),
                (i % 7) as f64,
                i + 1,
            )
        })
        .collect();
    let mut rows: Vec<_> = (0..240)
        .map(|i| {
            let mut row = detect_row(i, ((i * 47) % 97) as f64);
            row.market = format!("XALPHA{}", i % 80);
            row
        })
        .collect();
    rows[0].time_ms = held[0].born_ms;
    rows[1].time_ms = f64::NAN;
    rows[2].time_ms = -0.0;
    let incoming: Vec<_> = rows
        .iter()
        .enumerate()
        .map(|(i, row)| Incoming {
            core: (i / 80 + 1) as u64,
            name: "CORE_A",
            color: [1, 2, 3],
            row,
            ttl_ms: 60_000.0,
        })
        .collect();
    for replayed in [false, true] {
        assert_eq!(
            signature(select(&held, incoming.iter().copied(), replayed)),
            frozen(&held, &incoming, replayed)
        );
        assert_eq!(
            signature(select(&VecDeque::new(), incoming.iter().copied(), replayed)),
            frozen(&VecDeque::new(), &incoming, replayed)
        );
    }
    assert_eq!(
        signature(select(&held, [], false)),
        frozen(&held, &[], false)
    );
}

/// Same-instant replay must preserve the frozen chart; a later detect refreshes its existing seat.
#[test]
fn a_replayed_row_keeps_its_snapshot_and_a_refresh_keeps_its_seat() {
    let held = VecDeque::from([detect_item(Some(1), "XALPHA0", 10.0, 7)]);
    let replay = detect_row(0, 10.0);
    let refreshed = detect_row(0, 11.0);
    let incoming = |row| Incoming {
        core: 1,
        name: "CORE_A",
        color: [1, 2, 3],
        row,
        ttl_ms: 60_000.0,
    };
    assert_eq!(
        signature(select(&held, [incoming(&replay)], false)),
        vec![(Some(0), None)]
    );
    assert_eq!(
        signature(select(&held, [incoming(&refreshed)], false)),
        vec![(Some(0), Some(0))]
    );
}

/// Time actual survivor selection at the same ten-core, 20000-market replay as the baseline probe.
#[test]
#[ignore = "synthetic ten-core replay timing"]
fn detects_survivor_scale_probe() {
    let rows: Vec<_> = (0..20_000)
        .map(|i| detect_row(i, (i % 2000) as f64))
        .collect();
    for replayed in [false, true] {
        let held = VecDeque::new();
        let incoming = rows.iter().enumerate().map(|(i, row)| Incoming {
            core: (i / 2000) as u64,
            name: "CORE_A",
            color: [1, 2, 3],
            row,
            ttl_ms: 60_000.0,
        });
        let start = std::time::Instant::now();
        let selected = select(&held, incoming, replayed);
        eprintln!(
            "detects_replay replay={replayed} after_selection_ms={:.6} snapshot_requests={}",
            start.elapsed().as_secs_f64() * 1000.0,
            selected.iter().filter(|v| v.incoming.is_some()).count()
        );
        assert_eq!(selected.len(), 48);
        std::hint::black_box(selected);
    }
}
