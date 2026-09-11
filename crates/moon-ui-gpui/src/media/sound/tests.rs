//! The ordinal-to-sound table: the one place a wrong index plays the wrong file.

use super::{MB_SOUNDS, SOUNDS, mb_sound_name};

/// Every name Moonbot offers resolves to a sound this binary actually carries, and the two lists
/// hold exactly the same set.
///
/// Plausible breakage: a sound added to `SOUNDS` but not to `MB_SOUNDS` is unreachable from the
/// core's setting; one added to `MB_SOUNDS` alone shifts every ordinal after it onto a different
/// file, silently re-pointing a setting already stored on the core.
#[test]
fn every_moonbot_sound_is_one_this_build_can_play() {
    assert_eq!(
        MB_SOUNDS.len(),
        SOUNDS.len(),
        "the two sound lists must describe the same set"
    );
    for label in MB_SOUNDS {
        let stem = label.to_ascii_lowercase();
        assert!(
            SOUNDS.iter().any(|(s, _)| *s == stem),
            "Moonbot lists {label}, which no embedded sound answers to"
        );
    }
    for (stem, _) in SOUNDS {
        assert!(
            MB_SOUNDS.iter().any(|l| l.to_ascii_lowercase() == *stem),
            "{stem} is embedded but absent from Moonbot's list, so no ordinal reaches it"
        );
    }
}

/// The ordinal is 1-BASED, as the protocol's own field doc states.
///
/// Plausible breakage: a 0-based read shows — and writes back — the neighbouring sound, which no
/// type can catch and which the user only notices as "the wrong thing beeped".
#[test]
fn the_ordinal_is_one_based_and_round_trips() {
    assert_eq!(mb_sound_name(1), Some("Alarm"));
    // The two the developer's own Moonbot had selected when this landed, which is what makes them
    // worth pinning: the sell row showed PFIFF and the buy row HALLO.
    assert_eq!(mb_sound_name(7), Some("PFIFF"));
    assert_eq!(mb_sound_name(6), Some("HALLO"));
    assert_eq!(
        mb_sound_name(MB_SOUNDS.len() as i32),
        Some("ComeGetSome"),
        "the last ordinal must reach the last entry, not fall off it"
    );
    for (i, label) in MB_SOUNDS.iter().enumerate() {
        assert_eq!(
            mb_sound_name(i as i32 + 1),
            Some(*label),
            "the picker builds its ordinal as index + 1; this is the reader agreeing"
        );
    }
}

/// An ordinal outside the list names nothing rather than falling back to the first sound.
///
/// Plausible breakage: clamping an unknown ordinal to `Alarm` makes the popup show a sound the core
/// never held, and the next OK writes that guess into the core's config.
#[test]
fn an_ordinal_outside_the_list_names_nothing() {
    assert_eq!(mb_sound_name(0), None, "zero is below a 1-based list");
    assert_eq!(mb_sound_name(-1), None);
    assert_eq!(mb_sound_name(i32::MIN), None, "the -1 must not overflow");
    assert_eq!(mb_sound_name(MB_SOUNDS.len() as i32 + 1), None);
}

/// Independent sample counts pin full clip durations, including a selectable five-second clip.
#[test]
fn pcm_duration_uses_frames_and_sample_rate() {
    use super::{bytes_of, wav_duration};
    use std::time::Duration;
    assert_eq!(
        wav_duration(bytes_of("ding1").unwrap()),
        Some(Duration::from_millis(1076))
    );
    assert_eq!(
        wav_duration(bytes_of("ding2").unwrap()),
        Some(Duration::from_nanos(5_017_333_334))
    );
    assert_eq!(
        wav_duration(bytes_of("babytoy").unwrap()),
        Some(Duration::from_nanos(293_877_552))
    );
    for (_, wav) in SOUNDS {
        assert!(
            wav_duration(wav).is_some(),
            "every shipped asset must pass the scheduler's format guard"
        );
        assert!(
            wav_duration(&wav[..wav.len() - 1]).is_none(),
            "truncated RIFF must not reserve a player slot"
        );
    }
}

/// Ordinary notices get the first turn, but cannot cut or indefinitely starve waiting trades.
#[test]
fn contention_alternates_complete_clips_and_progresses_without_new_events() {
    use super::{Clip, Playback};
    use std::time::{Duration, Instant};
    let mut player = Playback::default();
    let detect = Clip::named("babytoy").unwrap();
    let alert = Clip::named("pfiff").unwrap();
    let open = Clip::named("ding1").unwrap();
    let close = Clip::named("ding2").unwrap();
    player.enqueue(detect);
    player.enqueue(alert);
    let start = Instant::now();
    let (first, is_trade) = player.next(start, Some(open)).unwrap();
    assert!(!is_trade);
    assert!(std::ptr::eq(first.wav, detect.wav));
    assert!(player.next(start + detect.duration, Some(open)).is_none());
    let open_at = start + detect.duration + Duration::from_millis(50);
    assert!(player.next(open_at, Some(open)).unwrap().1);
    // Repeated producer traffic while the trade plays must queue, never interrupt it.
    player.enqueue(detect);
    assert!(
        player
            .next(open_at + Duration::from_millis(500), Some(close))
            .is_none()
    );
    let alert_at = open_at + open.duration + Duration::from_millis(50);
    let (next, is_trade) = player.next(alert_at, Some(close)).unwrap();
    assert!(!is_trade);
    assert!(std::ptr::eq(next.wav, alert.wav));
    let close_at = alert_at + alert.duration + Duration::from_millis(50);
    assert!(player.next(close_at, Some(close)).unwrap().1);
    assert!(
        player
            .next(close_at + Duration::from_secs(5), None)
            .is_none()
    );
    assert!(
        !player
            .next(close_at + close.duration + Duration::from_millis(50), None)
            .unwrap()
            .1
    );
}

/// Saturating the ordinary lane cannot replace its accepted FIFO head or a pending trade turn.
#[test]
fn ordinary_queue_is_bounded_without_evicting_accepted_clips() {
    use super::{Clip, Playback};
    use std::time::{Duration, Instant};
    let mut player = Playback::default();
    let first = Clip::named("babytoy").unwrap();
    player.enqueue(first);
    for _ in 0..1000 {
        player.enqueue(Clip::named("ding2").unwrap());
    }
    assert_eq!(player.normal.len(), 64);
    let now = Instant::now();
    assert!(std::ptr::eq(
        player.next(now, None).unwrap().0.wav,
        first.wav
    ));
    assert!(
        player
            .next(
                now + first.duration + Duration::from_millis(50),
                Clip::named("ding1")
            )
            .unwrap()
            .1
    );
}

/// Entering quiet spends the old backlog without rejecting later producer-approved exceptions.
#[test]
fn quiet_transition_discards_backlog_but_accepts_later_bypass() {
    use super::{PLAYBACK, discard_pending, play};
    play("ding1");
    play("ding2");
    discard_pending();
    PLAYBACK.with(|player| assert!(player.borrow().normal.is_empty()));
    // Simulates a detect already approved by the existing quiet-bypass policy.
    play("pfiff");
    PLAYBACK.with(|player| assert_eq!(player.borrow().normal.len(), 1));
    discard_pending();
}
