//! The edge and the throttle of the network-problem alarm, on the pure state.

use std::collections::HashSet;

use super::{Observation, ProblemSoundState, THROTTLE_MS};

/// Collects synthetic network kinds without involving a live core.
fn kinds(list: &[u8]) -> HashSet<u8> {
    list.iter().copied().collect()
}

/// A core's first list is remembered and never announced; a finding added to a later list is.
///
/// Plausible breakage: announcing the first list beeps for every finding that was already
/// there when the terminal connected — a core that has been paging for a week alarms on every
/// start.
#[test]
fn the_first_list_seeds_silently_and_a_later_addition_is_fresh() {
    let mut state = ProblemSoundState::default();
    assert_eq!(state.observe(1, 1, || kinds(&[3]), 0), Observation::Seeded);
    assert_eq!(
        state.observe(1, 1, || kinds(&[3]), 0),
        Observation::Unchanged
    );
    assert_eq!(
        state.observe(1, 2, || kinds(&[3]), 0),
        Observation::NothingNew
    );
    assert_eq!(
        state.observe(1, 3, || kinds(&[3, 4]), 0),
        Observation::Fresh { throttled: false }
    );
    assert_eq!(
        state.observe(1, 4, || kinds(&[3, 4]), 0),
        Observation::NothingNew,
        "a finding that already sounded must not sound again on the next list"
    );
}

/// A finding that clears and comes back is a new event and sounds again.
#[test]
fn a_finding_that_returns_is_fresh_again() {
    let mut state = ProblemSoundState::default();
    state.observe(1, 1, || kinds(&[3]), 0);
    assert_eq!(
        state.observe(1, 2, || kinds(&[]), 0),
        Observation::NothingNew
    );
    assert_eq!(
        state.observe(1, 3, || kinds(&[3]), 0),
        Observation::Fresh { throttled: false }
    );
}

/// Two alarms on one core within five seconds collapse into one, as Moonbot's own do; another
/// core's alarm is not held back by the first core's.
#[test]
fn the_throttle_is_per_core_and_five_seconds() {
    let mut state = ProblemSoundState::default();
    state.observe(1, 1, || kinds(&[]), 0);
    state.observe(2, 1, || kinds(&[]), 0);
    assert_eq!(
        state.observe(1, 2, || kinds(&[3]), 1_000),
        Observation::Fresh { throttled: false }
    );
    state.sounded(1, 1_000);
    assert_eq!(
        state.observe(1, 3, || kinds(&[3, 4]), 1_000 + THROTTLE_MS - 1),
        Observation::Fresh { throttled: true }
    );
    assert_eq!(
        state.observe(2, 2, || kinds(&[3]), 1_000 + 10),
        Observation::Fresh { throttled: false },
        "the throttle belongs to the core that sounded, not to the terminal"
    );
    assert_eq!(
        state.observe(1, 4, || kinds(&[3, 4, 5]), 1_000 + THROTTLE_MS),
        Observation::Fresh { throttled: false }
    );
}

/// Disarming forgets the core, so arming it again seeds silently from the list current then
/// rather than announcing — or forever suppressing — what accumulated while it was off.
///
/// Plausible breakage: folding lists in while the switch is off marks every finding as seen, and
/// the first alarm after switching on never comes; skipping them without forgetting announces the
/// whole backlog the moment the switch flips.
#[test]
fn disarming_forgets_the_core_so_arming_reseeds() {
    let mut state = ProblemSoundState::default();
    state.observe(1, 1, || kinds(&[3]), 0);
    state.disarm(1);
    assert_eq!(
        state.observe(1, 2, || kinds(&[3, 4]), 0),
        Observation::Seeded,
        "the first list after arming is a seed, whatever accumulated meanwhile"
    );
    assert_eq!(
        state.observe(1, 3, || kinds(&[3, 4, 5]), 0),
        Observation::Fresh { throttled: false }
    );
}

/// Cores are independent: a kind seen on one core is still fresh on another.
#[test]
fn cores_do_not_share_what_they_have_seen() {
    let mut state = ProblemSoundState::default();
    state.observe(1, 1, || kinds(&[3]), 0);
    state.observe(2, 1, || kinds(&[]), 0);
    assert_eq!(
        state.observe(2, 2, || kinds(&[3]), 0),
        Observation::Fresh { throttled: false }
    );
}

/// Collecting before the revision check would allocate lists for unchanged cores on every drain.
#[test]
fn unchanged_revision_never_collects_the_problem_list() {
    let mut state = ProblemSoundState::default();
    let mut collected = 0;
    for pass in 0..10 {
        for core in 1..=200 {
            let observation = state.observe(
                core,
                1,
                || {
                    collected += 1;
                    kinds(&[3, 4])
                },
                0,
            );
            assert_eq!(
                observation,
                if pass == 0 {
                    Observation::Seeded
                } else {
                    Observation::Unchanged
                }
            );
        }
    }
    assert_eq!(collected, 200);
}

/// Measures collection work over unchanged synthetic problem revisions.
#[test]
#[ignore]
fn bench_problem_sound_200_cores_unchanged() {
    let iterations = 1_000;
    let began = std::time::Instant::now();
    let mut collected = 0;
    for _ in 0..iterations {
        let mut state = ProblemSoundState::default();
        for _ in 0..10 {
            for core in 1..=200 {
                std::hint::black_box(state.observe(
                    core,
                    1,
                    || {
                        collected += 1;
                        kinds(std::hint::black_box(&[3, 4, 5, 6]))
                    },
                    0,
                ));
            }
        }
    }
    println!(
        "bench_problem_sound_200_cores_unchanged: {} ns/iter, {} collections/2000 visits",
        began.elapsed().as_nanos() / iterations,
        collected / iterations as usize
    );
    assert_eq!(collected / iterations as usize, 200);
}
