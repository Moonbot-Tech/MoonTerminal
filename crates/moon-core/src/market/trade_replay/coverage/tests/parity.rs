use super::*;

/// Apply the pre-optimization full-scan merge as an independent parity oracle.
fn legacy_add(spans: &mut Vec<(i64, i64)>, span: (i64, i64)) {
    let (mut from, mut to) = span;
    if from > to {
        return;
    }
    let mut merged = Vec::with_capacity(spans.len() + 1);
    let mut placed = false;
    for &(s_from, s_to) in spans.iter() {
        let touches = s_from <= to.saturating_add(1) && s_to >= from.saturating_sub(1);
        if touches {
            from = from.min(s_from);
            to = to.max(s_to);
        } else if s_to < from {
            merged.push((s_from, s_to));
        } else {
            if !placed {
                merged.push((from, to));
                placed = true;
            }
            merged.push((s_from, s_to));
        }
    }
    if !placed {
        merged.push((from, to));
    }
    *spans = merged;
}

/// Sorted insertion must preserve the legacy result at integer edges and under deterministic
/// shuffles; losing adjacency or an extreme span would silently create false tape gaps.
#[test]
fn add_matches_legacy_full_scan() {
    let mut operations = vec![
        (i64::MIN, i64::MIN),
        (i64::MIN + 1, i64::MIN + 2),
        (i64::MAX - 1, i64::MAX),
        (i64::MAX, i64::MAX),
        (10, 5),
        (0, 0),
        (-5, 5),
        (6, 8),
        (20, 30),
        (9, 19),
    ];
    let mut seed = 17_u64;
    for _ in 0..2_000 {
        seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
        let from = (seed % 4_000) as i64 - 2_000;
        let width = (seed >> 32) as i64 % 50 - 10;
        operations.push((from, from.saturating_add(width)));
    }
    let mut expected = Vec::new();
    let mut actual = Coverage::none();
    for span in operations {
        legacy_add(&mut expected, span);
        actual.add(span);
        assert_eq!(actual.spans(), expected);
    }
}

/// Apply the former nested-loop subtraction as an independent two-pointer parity oracle.
fn legacy_minus(left: &Coverage, right: &Coverage) -> Vec<(i64, i64)> {
    let mut out = Vec::new();
    for &(from, to) in left.spans() {
        let mut cursor = from;
        for &(o_from, o_to) in right.spans() {
            if o_to < cursor || o_from > to {
                continue;
            }
            legacy_add(&mut out, (cursor, o_from.saturating_sub(1)));
            cursor = o_to.saturating_add(1);
            if cursor > to {
                break;
            }
        }
        legacy_add(&mut out, (cursor, to));
    }
    out
}

/// Linear subtraction must preserve exact holes and inclusive edges, including fragmented shared
/// market coverage and integer extremes; a wrong cursor can silently remove protected tape.
#[test]
fn minus_matches_legacy_nested_loop() {
    let inputs: [Vec<(i64, i64)>; 5] = [
        vec![],
        vec![(0, 9)],
        vec![(0, 9), (20, 29)],
        vec![(-10, 15), (30, 40)],
        vec![(i64::MIN, i64::MIN), (i64::MAX, i64::MAX)],
    ];
    for left in &inputs {
        for right in &inputs {
            let left = Coverage::from_spans(left.iter().copied());
            let right = Coverage::from_spans(right.iter().copied());
            assert_eq!(
                left.minus(&right).spans(),
                legacy_minus(&left, &right),
                "left={left:?} right={right:?}"
            );
        }
    }
}
