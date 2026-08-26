use super::*;

/// Logical processors the tests reason about.
///
/// A big machine on purpose: one pegged thread is 4.2% of it, which is where a fixed percentage
/// floor would hide a spinning thread.
const CPUS: usize = 24;

/// One sample carrying `cpu` as this process's share of the machine.
fn sample(cpu: f32) -> MetricsSnapshot {
    MetricsSnapshot {
        cpu_process: cpu,
        cpu_system: cpu,
        mem_mb: 1024.0,
        ..Default::default()
    }
}

/// Feed `count` samples a second apart and return the last event, so a test can state a steady
/// state in one line instead of a loop.
fn feed(detector: &mut SpikeDetector, start_ms: i64, count: i64, cpu: f32) -> Option<CpuEvent> {
    let mut last = None;
    for i in 0..count {
        if let Some(event) = detector.observe(start_ms + i * 1000, &sample(cpu)) {
            last = Some(event);
        }
    }
    last
}

/// Open an episode over a 2% baseline and return the detector with the wall clock at 24_000.
fn detector_with_open_episode() -> SpikeDetector {
    let mut detector = SpikeDetector::new(CPUS);
    feed(&mut detector, 0, 20, 2.0);
    let rose = feed(&mut detector, 20_000, 5, 70.0);
    assert!(
        matches!(rose, Some(CpuEvent::Rose { .. })),
        "the fixture must actually open an episode, got {rose:?}"
    );
    detector
}

/// The idle figure for this terminal is 1-3%, and it wobbles. A diagnostic that reports the wobble
/// is one that gets ignored, and then the real episode is ignored with it.
#[test]
fn idle_wobble_never_opens_an_episode() {
    let mut detector = SpikeDetector::new(CPUS);

    assert_eq!(feed(&mut detector, 0, 30, 2.0), None);
    assert_eq!(feed(&mut detector, 30_000, 30, 4.0), None);
}

/// The reported incident: a quiet process jumps to most of the machine. One line must come out, and
/// it must carry the baseline the rise is measured against — "70%" alone does not say whether that
/// is unusual for this install.
#[test]
fn a_real_spike_opens_one_episode_and_closes_it() {
    let mut detector = detector_with_open_episode();

    // Still elevated: the episode stays open and says nothing more until it is adopted or falls.
    assert_eq!(feed(&mut detector, 25_000, 30, 70.0), None);

    let fell =
        feed(&mut detector, 60_000, 3, 2.0).expect("a return to baseline closes the episode");
    let CpuEvent::Fell {
        peak,
        baseline,
        held_secs,
        ..
    } = fell
    else {
        panic!("expected a fall, got {fell:?}");
    };
    assert_eq!(peak, 70.0);
    assert_eq!(baseline, 2.0);
    assert_eq!(held_secs, 38, "opened at 24_000, closed at 62_000");
}

/// THE case this module exists for: the machine sleeps and the spike is already running when
/// sampling resumes. Rebuilding the baseline from the samples that follow a gap makes the spike its
/// own reference — `rise_threshold(70)` is unreachable — and the incident reports nothing at all.
#[test]
fn a_spike_that_is_already_running_after_a_wake_is_still_reported() {
    let mut detector = SpikeDetector::new(CPUS);
    feed(&mut detector, 0, 20, 2.0);

    // The sample ending the gap describes the moment sampling resumed, not the hour of silence
    // before it — see `CpuEvent::Resumed`. The spike shows up in the seconds that follow.
    let resumed = detector
        .observe(3_600_000, &sample(0.5))
        .expect("an hour without samples is reported");
    assert_eq!(
        resumed,
        CpuEvent::Resumed {
            delta_secs: 3581,
            cpu_after_gap: Some(0.5),
            interrupted: None
        }
    );

    let rose = feed(&mut detector, 3_601_000, 5, 70.0).expect("the post-wake spike is reported");
    assert!(
        matches!(rose, CpuEvent::Rose { baseline, .. } if baseline == 2.0),
        "the pre-sleep baseline must survive the gap, got {rose:?}"
    );
}

/// An episode running when the machine sleeps must not leave a rise line with no end and no
/// duration; the resume carries what the interrupted episode had reached.
#[test]
fn an_episode_open_when_the_machine_sleeps_is_closed_by_the_resume() {
    let mut detector = detector_with_open_episode();
    feed(&mut detector, 25_000, 10, 70.0);

    let resumed = detector
        .observe(3_634_000, &sample(2.0))
        .expect("the gap is reported");
    assert_eq!(
        resumed,
        CpuEvent::Resumed {
            delta_secs: 3600,
            cpu_after_gap: Some(2.0),
            interrupted: Some(Interrupted {
                peak: 70.0,
                held_secs: 10,
            }),
        },
        "the episode is measured to the last sample before the gap, not across it"
    );
}

/// A short run of high samples can be a chart repaint or a report query. The threshold sits near one
/// core so a spinning thread is caught, and at that sensitivity only the hold separates the two.
#[test]
fn a_short_burst_is_not_an_episode() {
    let mut detector = SpikeDetector::new(CPUS);
    feed(&mut detector, 0, 20, 2.0);

    assert_eq!(feed(&mut detector, 20_000, 4, 70.0), None);
    assert_eq!(feed(&mut detector, 24_000, 10, 2.0), None);
}

/// The peak is the whole point of the closing line, and the samples that OPEN an episode are the
/// ones most likely to hold it: a 95/90/88/86/70 spike that reports "peak was 70%" is wrong.
#[test]
fn the_peak_includes_the_samples_that_opened_the_episode() {
    let mut detector = SpikeDetector::new(CPUS);
    feed(&mut detector, 0, 20, 2.0);

    for (i, cpu) in [95.0, 90.0, 88.0, 86.0, 70.0].into_iter().enumerate() {
        detector.observe(20_000 + i as i64 * 1000, &sample(cpu));
    }
    let fell = feed(&mut detector, 30_000, 3, 2.0).expect("the episode closes");

    assert!(
        matches!(fell, CpuEvent::Fell { peak, .. } if peak == 95.0),
        "expected the run's highest sample, got {fell:?}"
    );
}

/// A rise that holds for five minutes is a changed workload, not a spike. Left open it would repeat
/// itself forever AND trap the detector: inside an episode it looks only for an end, so every later
/// spike would go unreported.
#[test]
fn a_rise_that_never_falls_back_becomes_the_new_baseline() {
    let mut detector = detector_with_open_episode();

    let settled = feed(&mut detector, 25_000, 300, 70.0).expect("five minutes on it is adopted");
    assert_eq!(
        settled,
        CpuEvent::Settled {
            peak: 70.0,
            cur: 70.0,
            held_secs: 300,
            mem_delta_mb: 0.0,
        }
    );

    // And the detector is usable again: a further rise over the ADOPTED level is reported.
    assert_eq!(feed(&mut detector, 325_000, 20, 70.0), None);
    let rose = feed(&mut detector, 345_000, 5, 96.0);
    assert!(
        matches!(rose, Some(CpuEvent::Rose { baseline, .. }) if baseline == 70.0),
        "a rise over the adopted baseline must report, got {rose:?}"
    );
}

/// One pegged thread is 4.2% of a 24-thread machine. A fixed percentage floor set high enough to
/// ignore idle jitter would hide exactly the spin the feed loop was just fixed for.
#[test]
fn one_pegged_thread_is_visible_on_a_big_machine() {
    let mut detector = SpikeDetector::new(CPUS);
    feed(&mut detector, 0, 20, 2.0);

    let rose = feed(&mut detector, 20_000, 5, 2.0 + 100.0 / CPUS as f32);
    assert!(
        matches!(rose, Some(CpuEvent::Rose { .. })),
        "one core over a 2% baseline must report, got {rose:?}"
    );
}

/// A threshold above 100% is one no sample can reach, and a detector that cannot reach its own
/// threshold is silent. Only a baseline this high exercises the final cap, which is why the bracket
/// test below — scoped to where the full guarantee holds — cannot cover it.
#[test]
fn no_baseline_can_push_the_threshold_out_of_reach() {
    for baseline in [95.0_f32, 99.0, 99.4, 99.9] {
        let t = thresholds(baseline, 100.0 / CPUS as f32);
        assert!(
            t.rise <= 100.0,
            "baseline {baseline}: {t:?} must stay inside what a sample can reach"
        );
    }
}

/// A threshold that is a MULTIPLE of the baseline passes 100% once the baseline is a third of the
/// machine, and the detector then cannot report even a fully pegged process — silent exactly where
/// the load is worst.
#[test]
fn a_busy_install_is_still_reportable() {
    let mut detector = SpikeDetector::new(CPUS);
    feed(&mut detector, 0, 20, 40.0);

    let rose = feed(&mut detector, 20_000, 5, 85.0);
    assert!(
        matches!(rose, Some(CpuEvent::Rose { .. })),
        "a doubling over a 40% baseline must report, got {rose:?}"
    );
    assert!(
        thresholds(60.0, 100.0 / CPUS as f32).rise <= CEILING_PCT,
        "no baseline may push the threshold out of reach"
    );
}

/// The two thresholds must never cross and must never leave a band that is too low to report and too
/// high to close: a load settling in such a band would hold an episode open forever, emitting a
/// heartbeat every five minutes at a level the floor calls quiet.
#[test]
fn the_calm_threshold_always_sits_between_the_baseline_and_the_rise() {
    for cpus in [1_usize, 2, 4, 8, 24, 64, 256] {
        let core_pct = 100.0 / cpus as f32;
        for baseline in [
            0.0_f32, 0.1, 2.0, 4.0, 6.0, 8.0, 20.0, 33.4, 40.0, 60.0, 90.0, 94.9,
        ] {
            let t = thresholds(baseline, core_pct);
            assert!(
                baseline < t.calm && t.calm < t.rise,
                "cpus {cpus}, baseline {baseline}: {t:?} must bracket the baseline"
            );
            // A threshold no sample can reach is a silent detector. The measured value is a share
            // of the whole machine, so 100 is the most any sample can ever be.
            assert!(
                t.rise <= 100.0,
                "cpus {cpus}, baseline {baseline}: {t:?} must stay inside what a sample can reach"
            );
        }
    }
}

/// The closing threshold is derived from the opening one, so any settled level under it ends the
/// episode — including one far below the level that would have opened it.
#[test]
fn a_load_settling_just_under_the_calm_threshold_closes_the_episode() {
    let mut detector = detector_with_open_episode();
    let calm = thresholds(2.0, 100.0 / CPUS as f32).calm;

    let fell = feed(&mut detector, 25_000, 3, calm - 0.05);
    assert!(
        matches!(fell, Some(CpuEvent::Fell { .. })),
        "settling under the calm threshold must close the episode, got {fell:?}"
    );
}

/// An ordinary clock correction steps the wall clock BACK by milliseconds. Treating that as a
/// suspend would report a wake that never happened and reset the run of samples building a rise.
#[test]
fn a_small_backwards_clock_correction_is_not_a_wake() {
    let mut detector = SpikeDetector::new(CPUS);
    feed(&mut detector, 0, 20, 2.0);

    assert_eq!(detector.observe(18_500, &sample(2.0)), None);
}

/// The figure a resume carries is the first sample AFTER the silence, and it must reach the log
/// line: the silence alone says nothing about what the process was doing when sampling came back.
#[test]
fn the_resume_carries_the_first_sample_after_the_gap() {
    let mut detector = SpikeDetector::new(CPUS);
    feed(&mut detector, 0, 20, 2.0);

    let resumed = detector
        .observe(60_000, &sample(80.0))
        .expect("the gap is reported");
    assert!(
        matches!(resumed, CpuEvent::Resumed { cpu_after_gap, .. } if cpu_after_gap == Some(80.0)),
        "the first post-gap figure must survive to the log line, got {resumed:?}"
    );
}

/// A clock set BACK is a discontinuity too, and it must be reported as what it is: a positive
/// "resumed after" figure for a step in the other direction is a duration that never elapsed.
#[test]
fn a_backwards_clock_step_is_reported_as_a_step() {
    let mut detector = SpikeDetector::new(CPUS);
    feed(&mut detector, 3_600_000, 20, 2.0);

    assert_eq!(
        detector.observe(3_600_000, &sample(2.0)),
        Some(CpuEvent::Resumed {
            delta_secs: -19,
            cpu_after_gap: Some(2.0),
            interrupted: None
        })
    );
}

/// A sample that is not a number fails every comparison, so an episode opened on one could never be
/// closed by any later sample.
#[test]
fn a_sample_that_is_not_a_number_is_ignored() {
    let mut detector = SpikeDetector::new(CPUS);
    feed(&mut detector, 0, 20, 2.0);

    assert_eq!(feed(&mut detector, 20_000, 5, f32::NAN), None);

    let rose = feed(&mut detector, 25_000, 5, 70.0);
    assert!(
        matches!(rose, Some(CpuEvent::Rose { baseline, .. }) if baseline == 2.0),
        "the window must be untouched by the discarded samples, got {rose:?}"
    );
}
