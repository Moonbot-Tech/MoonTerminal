//! Regression tests for Core Status defaults and the warning detectors.
//!
//! Warnings are the SUSTAINED/trend signals (held CPU, growing memory), deliberately separate from
//! the instantaneous threshold colors. These tests pin that behavior.

use std::collections::VecDeque;

use super::{CPU_SUSTAIN_SECS, CoreStatusMode, WARN_CPU_PCT, mem_grew, natural_cmp, next_high_secs};

/// Build a memory window from a sequence of used-MB samples, one per second.
fn ring(values: &[u16]) -> VecDeque<(i64, u16)> {
    values
        .iter()
        .enumerate()
        .map(|(second, used)| (second as i64, *used))
        .collect()
}

/// `mod.rs:CoreStatusMode::default` must remain By IP; changing it to Flat makes every newly opened
/// Core Status panel bypass the server overview.
#[test]
fn new_core_status_panels_default_to_by_ip() {
    assert_eq!(CoreStatusMode::default(), CoreStatusMode::ByIp);
}

/// `mod.rs:natural_cmp` sorts server names as a human reads them: numbers by value (so `Server 2`
/// precedes `Server 10`, not the lexical reverse) and custom names alphabetically.
#[test]
fn server_names_sort_naturally() {
    let mut names = ["Server 10", "Server 2", "QQ", "F1", "Server 1", "HLFutures2"];
    names.sort_by(|a, b| natural_cmp(a, b));
    assert_eq!(
        names,
        ["F1", "HLFutures2", "QQ", "Server 1", "Server 2", "Server 10"]
    );
}

/// `mod.rs:mem_grew` must ignore a flat footprint: memory that only wiggles around one level is not
/// a leak, so no warning fires.
#[test]
fn steady_memory_is_not_growth() {
    assert!(!mem_grew(&ring(&[500, 502, 499, 501, 500, 500, 501])));
}

/// `mod.rs:mem_grew` must flag a sustained rise above the window minimum — the core's own
/// "suspicious memory growth" case (476 → 544 MB, +68 clears the 64 MB floor).
#[test]
fn sustained_rise_is_growth() {
    assert!(mem_grew(&ring(&[476, 476, 480, 500, 520, 544])));
}

/// `mod.rs:mem_grew` must NOT flag a spike that returns: the window minimum stays low and the
/// current sample comes back down, so the rise-above-minimum is zero.
#[test]
fn spike_then_return_is_not_growth() {
    assert!(!mem_grew(&ring(&[476, 476, 600, 476, 476, 476])));
}

/// `mod.rs:mem_grew` must wait for enough history; two samples cannot distinguish a leak from noise.
#[test]
fn too_few_samples_never_grow() {
    assert!(!mem_grew(&ring(&[400, 700])));
}

/// `mod.rs:next_high_secs` must reset the sustained counter the moment CPU drops below the
/// threshold or goes unknown; otherwise a single high second would latch a warning forever.
#[test]
fn cpu_below_threshold_resets_sustain() {
    assert_eq!(next_high_secs(9, Some(WARN_CPU_PCT as u8 - 1)), 0);
    assert_eq!(next_high_secs(9, None), 0);
}

/// `mod.rs:next_high_secs` must build up only while CPU stays high, and a single high second must
/// not already be "sustained" — that separation is the whole point of the warning vs the color.
#[test]
fn sustained_high_cpu_accumulates_to_warning() {
    assert!(next_high_secs(0, Some(85)) < CPU_SUSTAIN_SECS);

    let mut secs = 0;
    for _ in 0..CPU_SUSTAIN_SECS {
        secs = next_high_secs(secs, Some(85));
    }
    assert!(secs >= CPU_SUSTAIN_SECS);
}
