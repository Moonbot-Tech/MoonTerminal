use super::*;

const T0: i64 = 1_700_000_000_000;

fn sample(mem_mb: f32, handles: u32, user: u32, gdi: u32) -> ResourceSample {
    ResourceSample {
        mem_mb: Some(mem_mb),
        handles: Some(handles),
        user_objects: Some(user),
        gdi_objects: Some(gdi),
    }
}

/// Feeds one sample a second from `from_s` to `to_s` inclusive and returns every event raised.
fn feed(
    watch: &mut ResourceWatch,
    from_s: i64,
    to_s: i64,
    s: ResourceSample,
) -> Vec<(i64, ResourceEvent)> {
    (from_s..=to_s)
        .filter_map(|sec| watch.observe(T0 + sec * 1000, s).map(|e| (sec, e)))
        .collect()
}

fn only(crossed: Crossed, what: Count) -> bool {
    crossed.iter().collect::<Vec<_>>() == [what]
}

#[test]
fn baseline_waits_a_minute_and_a_quiet_process_writes_nothing_after_it() {
    let mut w = ResourceWatch::new();
    let quiet = sample(150.0, 900, 30, 60);
    let events = feed(&mut w, 0, 3600, quiet);
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(
        events[0],
        (
            60,
            ResourceEvent::Baseline {
                after_secs: 60,
                sample: quiet
            }
        )
    );
}

#[test]
fn startup_growth_inside_the_first_minute_is_not_a_movement() {
    let mut w = ResourceWatch::new();
    // Memory climbs 400 MiB during startup; the baseline is taken after the climb.
    for sec in 0..60 {
        assert_eq!(
            w.observe(
                T0 + sec * 1000,
                sample(100.0 + sec as f32 * 7.0, 500, 20, 40)
            ),
            None
        );
    }
    let settled = sample(520.0, 900, 30, 60);
    assert!(matches!(
        w.observe(T0 + 60_000, settled),
        Some(ResourceEvent::Baseline { sample, .. }) if sample == settled
    ));
}

#[test]
fn a_leak_writes_one_line_per_step_at_most_once_a_minute_and_measures_from_the_last_line() {
    let mut w = ResourceWatch::new();
    feed(&mut w, 0, 60, sample(150.0, 900, 30, 60));
    // Handles creep by 5 a second: the step of 250 is reached at +50 s, but the minute gap
    // holds the line until +60 s, by which point the count is 300 higher.
    let mut events = Vec::new();
    for sec in 61..=400 {
        let s = sample(150.0, 900 + (sec as u32 - 60) * 5, 30, 60);
        if let Some(e) = w.observe(T0 + sec * 1000, s) {
            events.push((sec, e));
        }
    }
    let secs: Vec<i64> = events.iter().map(|(s, _)| *s).collect();
    assert_eq!(secs, [120, 180, 240, 300, 360], "{events:?}");
    match events[0].1 {
        ResourceEvent::Moved {
            crossed,
            since_secs,
            prev,
            cur,
        } => {
            assert!(only(crossed, Count::Handles), "{crossed:?}");
            assert_eq!(since_secs[Count::Handles as usize], 60);
            assert_eq!(prev.handles, Some(900));
            assert_eq!(cur.handles, Some(1200));
        }
        other => panic!("{other:?}"),
    }
    // Every later line measures from the line before it, not from the baseline.
    match events[1].1 {
        ResourceEvent::Moved { prev, cur, .. } => {
            assert_eq!(prev.handles, Some(1200));
            assert_eq!(cur.handles, Some(1500));
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_slow_creep_below_the_step_per_minute_still_accumulates_to_a_line() {
    let mut w = ResourceWatch::new();
    feed(&mut w, 0, 60, sample(150.0, 900, 30, 60));
    // One MiB a minute: no single minute reaches 128, the sum does after 128 minutes.
    let mut first = None;
    for min in 1..=200 {
        let s = sample(150.0 + min as f32, 900, 30, 60);
        if let Some(e) = w.observe(T0 + (60 + min * 60) * 1000, s) {
            first = Some((min, e));
            break;
        }
    }
    let (min, event) = first.expect("a line eventually");
    assert_eq!(min, 128);
    assert!(
        matches!(event, ResourceEvent::Moved { crossed, .. } if only(crossed, Count::Memory)),
        "{event:?}"
    );
}

#[test]
fn a_count_that_did_not_cross_keeps_its_own_reference_across_other_lines() {
    let mut w = ResourceWatch::new();
    feed(&mut w, 0, 60, sample(150.0, 900, 30, 60));
    // Memory churns by 200 MiB every two minutes (a chart opening and closing) while handles
    // leak 100 a minute. The handle reference must survive the memory lines.
    let mut handle_line = None;
    for min in 1..=10 {
        let mem = if min % 2 == 1 { 350.0 } else { 150.0 };
        let s = sample(mem, 900 + min as u32 * 100, 30, 60);
        match w.observe(T0 + (60 + min * 60) * 1000, s) {
            Some(ResourceEvent::Moved {
                crossed,
                prev,
                cur,
                since_secs,
            }) if crossed.contains(Count::Handles) => {
                handle_line = Some((min, crossed, prev, cur, since_secs));
                break;
            }
            _ => {}
        }
    }
    let (min, crossed, prev, cur, since) = handle_line.expect("the handle leak is reported");
    assert_eq!(
        min, 3,
        "250 handles over the reference of 900 — not reset by minute 1 and 2"
    );
    assert!(
        crossed.contains(Count::Memory) && crossed.contains(Count::Handles),
        "{crossed:?}"
    );
    assert_eq!(
        prev.handles,
        Some(900),
        "the reference is the last REPORTED handle count"
    );
    assert_eq!(cur.handles, Some(1200));
    // The slope of each count is read over ITS OWN interval: handles over the three minutes since
    // the baseline, memory over the one minute since its last line.
    assert_eq!(since[Count::Handles as usize], 180);
    assert_eq!(since[Count::Memory as usize], 60);
}

#[test]
fn a_count_missing_from_the_baseline_is_adopted_silently_and_watched_from_then_on() {
    let mut w = ResourceWatch::new();
    let blind = ResourceSample {
        handles: None,
        ..sample(150.0, 900, 30, 60)
    };
    let events = feed(&mut w, 0, 60, blind);
    assert!(matches!(events[..], [(60, ResourceEvent::Baseline { .. })]));
    // The next tick has the number: no line, but it becomes the reference.
    assert_eq!(w.observe(T0 + 61_000, sample(150.0, 900, 30, 60)), None);
    // A crossing measured from THAT number, over the time since it was adopted.
    assert!(matches!(
        w.observe(T0 + 181_000, sample(150.0, 1_200, 30, 60)),
        Some(ResourceEvent::Moved { crossed, prev, since_secs, .. })
            if only(crossed, Count::Handles)
                && prev.handles == Some(900)
                && since_secs[Count::Handles as usize] == 120
    ));
}

#[test]
fn a_release_after_a_rise_is_reported_too() {
    let mut w = ResourceWatch::new();
    feed(&mut w, 0, 60, sample(150.0, 900, 30, 60));
    assert!(matches!(
        w.observe(T0 + 121_000, sample(400.0, 900, 30, 60)),
        Some(ResourceEvent::Moved { crossed, .. }) if only(crossed, Count::Memory)
    ));
    assert!(matches!(
        w.observe(T0 + 182_000, sample(160.0, 900, 30, 60)),
        Some(ResourceEvent::Moved { prev, cur, .. })
            if prev.mem_mb == Some(400.0) && cur.mem_mb == Some(160.0)
    ));
}

#[test]
fn a_missed_sampler_tick_is_not_a_release_and_does_not_move_the_reference() {
    let mut w = ResourceWatch::new();
    feed(&mut w, 0, 60, sample(150.0, 900, 30, 60));
    let missed = ResourceSample {
        mem_mb: None,
        ..sample(150.0, 900, 30, 60)
    };
    assert_eq!(w.observe(T0 + 121_000, missed), None);
    // Back with the same figure: still nothing, the reference was never zeroed.
    assert_eq!(w.observe(T0 + 182_000, sample(151.0, 900, 30, 60)), None);
}

#[test]
fn near_limit_warns_once_per_crossing_with_hysteresis_carries_the_snapshot_and_ignores_the_gap() {
    let mut w = ResourceWatch::new();
    feed(&mut w, 0, 60, sample(150.0, 900, 30, 60));
    // One second after the baseline the minute gap would block a movement line; the warning is
    // exempt, and it carries every count.
    let hot = sample(150.0, 900, 8_000, 60);
    assert_eq!(
        w.observe(T0 + 61_000, hot),
        Some(ResourceEvent::NearLimit {
            what: Count::UserObjects,
            value: 8_000,
            sample: hot
        })
    );
    // Hovering above the threshold: silent.
    assert_eq!(w.observe(T0 + 62_000, sample(150.0, 900, 8_100, 60)), None);
    // A dip that stays above the release point does not re-arm it.
    assert_eq!(w.observe(T0 + 63_000, sample(150.0, 900, 7_800, 60)), None);
    assert_eq!(w.observe(T0 + 64_000, sample(150.0, 900, 8_200, 60)), None);
    // Below the release point, then back over: warned again.
    assert_eq!(w.observe(T0 + 65_000, sample(150.0, 900, 7_000, 60)), None);
    assert!(matches!(
        w.observe(T0 + 66_000, sample(150.0, 900, 8_000, 60)),
        Some(ResourceEvent::NearLimit {
            what: Count::UserObjects,
            value: 8_000,
            ..
        })
    ));
    // GDI has its own latch.
    assert!(matches!(
        w.observe(T0 + 67_000, sample(150.0, 900, 8_000, 9_000)),
        Some(ResourceEvent::NearLimit {
            what: Count::GdiObjects,
            value: 9_000,
            ..
        })
    ));
}

#[test]
fn counts_the_platform_lacks_never_trigger_and_print_as_a_dash() {
    let mut w = ResourceWatch::new();
    let mac = ResourceSample {
        mem_mb: Some(150.0),
        handles: None,
        user_objects: None,
        gdi_objects: None,
    };
    feed(&mut w, 0, 60, mac);
    assert_eq!(w.observe(T0 + 200_000, mac), None);
    assert_eq!(
        counts(&mac),
        "RSS 150 MiB, handles -, USER objects -, GDI objects -"
    );
    assert_eq!(value_of(&mac, Count::Handles), "-");
    assert_eq!(value_of(&mac, Count::Memory), "150 MiB");
    assert_eq!(
        value_of(
            &ResourceSample {
                mem_mb: None,
                ..mac
            },
            Count::Memory
        ),
        "- MiB"
    );
}

#[test]
fn a_clock_that_steps_back_does_not_panic_or_report() {
    let mut w = ResourceWatch::new();
    feed(&mut w, 0, 60, sample(150.0, 900, 30, 60));
    assert_eq!(w.observe(T0 - 3_600_000, sample(150.0, 900, 30, 60)), None);
    assert_eq!(w.observe(i64::MIN, sample(150.0, 900, 30, 60)), None);
}
