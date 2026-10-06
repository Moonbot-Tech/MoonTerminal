//! Synthetic measurements pin reuse, invalidation, eviction, and exact algorithm parity.

use super::{FitInput, FitMemo};
use std::cell::Cell;
use std::hint::black_box;
use std::time::Instant;

/// Inputs force truncation so bypassing the memo causes observable measurement work.
fn input() -> FitInput<'static> {
    FitInput {
        prefix: "value: ",
        text: "a long synthetic caption",
        budget: 70.0,
        size: 12.0,
        wraps: false,
    }
}

/// Count actual width requests, including individual characters during truncation.
fn measured(memo: &mut FitMemo, input: FitInput<'_>) -> usize {
    let calls = Cell::new(0);
    black_box(memo.lookup(input, |text, _| {
        calls.set(calls.get() + 1);
        text.chars().count() as f32 * 7.0
    }));
    calls.get()
}

/// Losing previous-pass promotion would reshape every unchanged caption each frame.
#[test]
fn second_identical_pass_measures_nothing() {
    let mut memo = FitMemo::default();
    memo.begin_pass("mono", 1.0, 1.0);
    assert_ne!(measured(&mut memo, input()), 0);
    assert_eq!(measured(&mut memo, input()), 0);
    memo.begin_pass("mono", 1.0, 1.0);
    assert_eq!(measured(&mut memo, input()), 0);
    memo.begin_pass("mono", 1.0, 1.0);
    assert_eq!(measured(&mut memo, input()), 0);
}

/// Omitting any input would display a stale caption or stale truncation after it changes.
#[test]
fn each_key_input_misses() {
    let original = input();
    for changed in [
        FitInput {
            text: "different caption",
            ..original
        },
        FitInput {
            prefix: "price: ",
            ..original
        },
        FitInput {
            budget: 71.0,
            ..original
        },
        FitInput {
            size: 13.0,
            ..original
        },
        FitInput {
            wraps: true,
            ..original
        },
        FitInput {
            prefix: "value:",
            text: " a long synthetic caption",
            ..original
        },
    ] {
        let mut memo = FitMemo::default();
        memo.begin_pass("mono", 1.0, 1.0);
        measured(&mut memo, original);
        memo.begin_pass("mono", 1.0, 1.0);
        assert_ne!(measured(&mut memo, changed), 0);
    }
    for (family, scale, zoom) in [("other", 1.0, 1.0), ("mono", 2.0, 1.0), ("mono", 1.0, 2.0)] {
        let mut memo = FitMemo::default();
        memo.begin_pass("mono", 1.0, 1.0);
        measured(&mut memo, original);
        memo.begin_pass(family, scale, zoom);
        assert_ne!(measured(&mut memo, original), 0);
        assert!(memo.prev.is_empty());
    }
}

/// Keeping absent captions indefinitely would grow the cache as live text changes.
#[test]
fn unused_entries_evicted_after_two_passes() {
    let mut memo = FitMemo::default();
    memo.begin_pass("mono", 1.0, 1.0);
    measured(&mut memo, input());
    memo.begin_pass("mono", 1.0, 1.0);
    assert_eq!(memo.prev.len(), 1);
    memo.begin_pass("mono", 1.0, 1.0);
    assert!(memo.cur.is_empty() && memo.prev.is_empty());
    assert_ne!(measured(&mut memo, input()), 0);
}

/// Cached strings and width bits must equal the independent direct production calls.
#[test]
fn memo_result_equals_direct_fit_text() {
    let mut memo = FitMemo::default();
    for _ in 0..3 {
        memo.begin_pass("mono", 1.0, 1.0);
        for text in [
            "",
            "short",
            "one two three four five six seven eight",
            "abcdefghijk",
            "\u{03bb} \u{4e2d} value",
        ] {
            for prefix in ["", "caption: "] {
                for budget in [0.0, 35.0, 70.0, 500.0] {
                    for wraps in [false, true] {
                        let input = FitInput {
                            prefix,
                            text,
                            budget,
                            wraps,
                            size: 12.0,
                        };
                        let glued = format!("{prefix}{text}");
                        let measure = |text: &str| text.chars().count() as f32 * 7.0;
                        let direct = if wraps {
                            crate::design::wrap_text(
                                &glued,
                                budget,
                                moon_core::config::LABEL_WRAP_LINES,
                                measure,
                            )
                        } else {
                            vec![crate::design::fit_text(&glued, budget, measure)]
                        };
                        let result = memo.lookup(input, |text, _| measure(text));
                        assert_eq!(result.len(), direct.len());
                        for ((text, width), (expected, expected_width)) in
                            result.iter().zip(&direct)
                        {
                            assert_eq!(text, expected);
                            assert_eq!(width.to_bits(), expected_width.to_bits());
                        }
                    }
                }
            }
        }
    }
}

/// Manual two-arm benchmark measures the real memo, so removing reuse fails the call assertion.
#[test]
#[ignore]
fn caption_fit_wrap_bench() {
    let texts: Vec<String> = (0..40)
        .map(|ix| {
            if ix < 10 {
                format!("{ix:02} {}", "synthetic prose words ".repeat(10))
                    .chars()
                    .take(200)
                    .collect()
            } else {
                format!("synthetic caption value {ix:02}")
            }
        })
        .collect();
    let calls = Cell::new(0usize);
    let measure = |text: &str| {
        calls.set(calls.get() + 1);
        text.chars().count() as f32 * 7.0
    };
    let started = Instant::now();
    for _ in 0..1000 {
        for (ix, text) in texts.iter().enumerate() {
            let glued = format!("value: {text}");
            black_box(if ix < 10 {
                crate::design::wrap_text(
                    &glued,
                    105.0,
                    moon_core::config::LABEL_WRAP_LINES,
                    measure,
                )
            } else {
                vec![crate::design::fit_text(&glued, 105.0, measure)]
            });
        }
    }
    let baseline_calls = calls.get();
    println!(
        "BASELINE calls={} elapsed_us={}",
        baseline_calls,
        started.elapsed().as_micros()
    );
    calls.set(0);
    let mut memo = FitMemo::default();
    let started = Instant::now();
    let mut first_pass_calls = 0;
    for pass in 0..1000 {
        memo.begin_pass("synthetic mono", 1.0, 1.0);
        for (ix, text) in texts.iter().enumerate() {
            black_box(memo.lookup(
                FitInput {
                    prefix: "value: ",
                    text,
                    budget: 105.0,
                    size: 12.0,
                    wraps: ix < 10,
                },
                |text, _| measure(text),
            ));
        }
        if pass == 0 {
            first_pass_calls = calls.get();
        }
    }
    println!(
        "MEMO calls={} elapsed_us={} later_pass_calls={}",
        calls.get(),
        started.elapsed().as_micros(),
        calls.get() - first_pass_calls
    );
    assert_eq!(calls.get(), first_pass_calls);
    assert_eq!(baseline_calls, first_pass_calls * 1000);
}
