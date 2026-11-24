//! The ordinal-to-sound table, the folder scan, the fallback, and the playback scheduler. State is
//! thread-local and every test runs on its own thread, so installing a catalog here touches no
//! other test.

use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::Duration;

use super::embedded::{DEFAULT_SOUND, MB_SOUNDS, SOUNDS};
use super::{
    CATALOG, Catalog, Clip, MissingSound, Playback, RejectReason, Source, install, is_playable,
    mb_sound_name, missing, play, play_detect, play_ordinal, resolve, sources, stems,
    take_missing_notices, wav_duration, with_catalog,
};

/// Swap the installed catalog for `catalog`, run `body`, and put the previous one back.
///
/// The catalog is thread-local and other tests on this thread assume the embedded set they
/// started with, including one that asserts a fresh thread has not been scanned yet.
///
/// Args:
///     catalog: Catalog installed for the duration of `body`.
///     body: The test body.
///
/// Returns:
///     Whatever `body` returns.
fn with_temp_catalog<R>(catalog: Catalog, body: impl FnOnce() -> R) -> R {
    struct Restore(Option<Catalog>);
    impl Drop for Restore {
        fn drop(&mut self) {
            if let Some(previous) = self.0.take() {
                CATALOG.with(|slot| *slot.borrow_mut() = previous);
            }
        }
    }
    let previous = CATALOG.with(|slot| std::mem::replace(&mut *slot.borrow_mut(), catalog));
    let _restore = Restore(Some(previous));
    body()
}

/// Run `body` against the embedded set before any folder scan.
///
/// Args:
///     body: The test body.
///
/// Returns:
///     Whatever `body` returns.
pub(crate) fn with_unscanned_embedded<R>(body: impl FnOnce() -> R) -> R {
    with_temp_catalog(Catalog::embedded(), body)
}

/// Run `body` against a finished scan of a folder that is not there: the embedded set, scanned.
///
/// Args:
///     body: The test body.
///
/// Returns:
///     Whatever `body` returns.
pub(crate) fn with_scanned_embedded<R>(body: impl FnOnce() -> R) -> R {
    let catalog = sources::scan(std::path::Path::new(
        r"C:\mt-issue-694-sounds-absent\no-such-folder",
    ));
    with_temp_catalog(catalog, body)
}

/// A clip for a name the catalog is known to hold.
fn clip(name: &str) -> Clip {
    resolve(MissingSound::Name(name.to_string())).expect("a known name resolves")
}

/// A valid one-channel 8 kHz PCM WAV of `frames` samples, so a test can write files the scanner
/// must accept without shipping fixtures.
fn pcm_wav(frames: u32) -> Vec<u8> {
    let data_len = frames * 2;
    let mut out = Vec::new();
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&8000u32.to_le_bytes());
    out.extend_from_slice(&16000u32.to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes()); // block align
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    out.resize(out.len() + data_len as usize, 0);
    out
}

/// A fresh temporary sounds folder for one test.
fn temp_dir(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "moonterminal-sounds-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    ));
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

/// Every name Moonbot offers resolves to a sound this binary actually carries, and the two lists
/// hold exactly the same set in the same order.
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
    for (label, (stem, _)) in MB_SOUNDS.iter().zip(SOUNDS) {
        assert_eq!(
            label.to_ascii_lowercase(),
            *stem,
            "the label and the embedded stem at one index must be the same sound"
        );
    }
    assert!(
        SOUNDS.iter().any(|(stem, _)| *stem == DEFAULT_SOUND),
        "the fallback must be embedded, or a missing sound would fall back to nothing"
    );
}

/// The ordinal is 1-BASED, as the protocol's own field doc states.
///
/// Plausible breakage: a 0-based read shows — and writes back — the neighbouring sound, which no
/// type can catch and which the user only notices as "the wrong thing beeped".
#[test]
fn the_ordinal_is_one_based_and_round_trips() {
    let catalog = Catalog::embedded();
    assert_eq!(
        catalog.by_ordinal(1).map(|e| e.label.as_str()),
        Some("Alarm")
    );
    // The two the developer's own Moonbot had selected when this landed, which is what makes them
    // worth pinning: the sell row showed PFIFF and the buy row HALLO.
    assert_eq!(
        catalog.by_ordinal(7).map(|e| e.label.as_str()),
        Some("PFIFF")
    );
    assert_eq!(
        catalog.by_ordinal(6).map(|e| e.label.as_str()),
        Some("HALLO")
    );
    assert_eq!(
        catalog
            .by_ordinal(MB_SOUNDS.len() as i32)
            .map(|e| e.label.as_str()),
        Some("ComeGetSome"),
        "the last ordinal must reach the last entry, not fall off it"
    );
    for (i, label) in MB_SOUNDS.iter().enumerate() {
        assert_eq!(
            catalog.by_ordinal(i as i32 + 1).map(|e| e.label.as_str()),
            Some(*label),
            "the picker builds its ordinal as index + 1; this is the reader agreeing"
        );
    }
    assert_eq!(mb_sound_name(1).as_deref(), Some("Alarm"));
}

/// An ordinal outside the list names nothing rather than falling back to the first sound.
///
/// Plausible breakage: clamping an unknown ordinal to `Alarm` makes the popup show a sound the core
/// never held, and the next OK writes that guess into the core's config.
#[test]
fn an_ordinal_outside_the_list_names_nothing() {
    let catalog = Catalog::embedded();
    assert!(
        catalog.by_ordinal(0).is_none(),
        "zero is below a 1-based list"
    );
    assert!(catalog.by_ordinal(-1).is_none());
    assert!(
        catalog.by_ordinal(i32::MIN).is_none(),
        "the -1 must not overflow"
    );
    assert!(catalog.by_ordinal(MB_SOUNDS.len() as i32 + 1).is_none());
}

/// Independent sample counts pin full clip durations, including a selectable five-second clip.
#[test]
fn pcm_duration_uses_frames_and_sample_rate() {
    let bytes = |stem: &str| SOUNDS.iter().find(|(s, _)| *s == stem).unwrap().1;
    assert_eq!(
        wav_duration(bytes("ding1")),
        Some(Duration::from_millis(1076))
    );
    assert_eq!(
        wav_duration(bytes("ding2")),
        Some(Duration::from_nanos(5_017_333_334))
    );
    assert_eq!(
        wav_duration(bytes("babytoy")),
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

/// A file holds a number only when its name claims one, and that number is pinned: it does not
/// move when another file is added, and a file without a claim has no number at all. A file
/// under an embedded name replaces that sound's bytes and keeps its number.
///
/// Plausible breakage: numbering files by their position in the folder makes sound number 19
/// mean a different file the moment an alphabetically earlier file lands beside it — which is a
/// core setting silently re-pointed at the wrong noise.
#[test]
fn a_file_holds_the_number_its_name_claims_and_nothing_else() {
    let dir = temp_dir("numbers");
    std::fs::write(dir.join("40_Zulu.Wav"), pcm_wav(400)).unwrap();
    std::fs::write(dir.join("19-alpha.wav"), pcm_wav(400)).unwrap();
    std::fs::write(dir.join("plain.wav"), pcm_wav(400)).unwrap();
    std::fs::write(dir.join("BARK.WAV"), pcm_wav(400)).unwrap();
    let catalog = sources::scan(&dir);
    let first = super::FIRST_USER_ORDINAL;
    assert_eq!(first, MB_SOUNDS.len() as i32 + 1);
    assert_eq!(
        catalog
            .by_ordinal(19)
            .map(|e| (e.stem.as_str(), e.label.as_str(), e.ordinal)),
        Some(("alpha", "alpha", Some(19))),
        "either separator claims the number; the stem carries no prefix"
    );
    assert_eq!(
        catalog
            .by_ordinal(40)
            .map(|e| (e.stem.as_str(), e.label.as_str())),
        Some(("zulu", "Zulu")),
        "the stem is lowercase for matching; the label keeps the file's spelling; the extension \
         matches in any case; gaps in the numbering are fine"
    );
    assert!(
        catalog.by_ordinal(20).is_none(),
        "an unclaimed number is nobody's"
    );
    let plain = catalog
        .find("plain")
        .expect("an unnumbered file plays by name");
    assert_eq!(plain.ordinal, None, "no claim, no number");
    assert_eq!(
        catalog
            .by_ordinal(3)
            .map(|e| (e.stem.as_str(), e.source, e.ordinal)),
        Some(("bark", Source::Folder, Some(3))),
        "a file under an embedded name replaces its bytes and keeps its number"
    );
    assert_eq!(catalog.stats().folder_files, 4);
    assert_eq!(catalog.stats().numbered, 2);
    assert_eq!(
        catalog.ordinals().map(|(n, _)| n).collect::<Vec<_>>(),
        (1..=first - 1).chain([19, 40]).collect::<Vec<_>>(),
        "numbered files follow the eighteen in number order"
    );
    assert_eq!(
        catalog.entries().last().map(|e| e.stem.as_str()),
        Some("plain"),
        "unnumbered files list last"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A claim that cannot be honoured is refused with its reason, and the sound is not loaded under
/// some other number instead: a number of Moonbot's own, a number another file holds, a name that
/// is already a sound.
///
/// Plausible breakage: silently giving the second `19_` file the next free number makes the
/// core's 19 play the wrong file with no trace of why.
#[test]
fn a_claim_that_cannot_be_honoured_is_refused_with_its_reason() {
    let dir = temp_dir("claims");
    std::fs::write(dir.join("19_first.wav"), pcm_wav(400)).unwrap();
    std::fs::write(dir.join("19_second.wav"), pcm_wav(400)).unwrap();
    std::fs::write(dir.join("05_low.wav"), pcm_wav(400)).unwrap();
    std::fs::write(dir.join("21_ding1.wav"), pcm_wav(400)).unwrap();
    let catalog = sources::scan(&dir);
    assert_eq!(
        catalog.by_ordinal(19).map(|e| e.stem.as_str()),
        Some("first"),
        "the alphabetically first claimant keeps the number"
    );
    assert!(catalog.find("second").is_none());
    assert!(catalog.find("low").is_none());
    assert!(catalog.by_ordinal(21).is_none());
    assert_eq!(
        catalog.find("ding1").map(|e| (e.source, e.ordinal)),
        Some((Source::Embedded, Some(12))),
        "a numbered file cannot take over an embedded name"
    );
    let reasons: Vec<(&str, &RejectReason)> = catalog
        .stats()
        .rejected
        .iter()
        .map(|r| (r.name.as_str(), &r.reason))
        .collect();
    assert_eq!(
        reasons,
        vec![
            ("05_low.wav", &RejectReason::NumberReserved),
            ("19_second.wav", &RejectReason::NumberTaken("first".into())),
            ("21_ding1.wav", &RejectReason::NameTaken),
        ]
    );
    assert_eq!(catalog.stats().numbered, 1);
    let _ = std::fs::remove_dir_all(&dir);
}

/// What a file name says: the prefix is digits plus `_` or `-`; anything else is a plain name.
#[test]
fn a_number_prefix_needs_digits_a_separator_and_a_name() {
    use super::sources::wav_name;
    let claim = |name: &str| wav_name(name).map(|w| (w.stem, w.label, w.ordinal));
    assert_eq!(
        claim("19_My Sound.WAV"),
        Some(("my sound".into(), "My Sound".into(), Some(19)))
    );
    assert_eq!(
        claim("007-bond.wav"),
        Some(("bond".into(), "bond".into(), Some(7)))
    );
    assert_eq!(
        claim("19.wav"),
        Some(("19".into(), "19".into(), None)),
        "digits alone are a name, not a claim"
    );
    assert_eq!(
        claim("19_.wav"),
        Some(("19_".into(), "19_".into(), None)),
        "a claim with no name after it is a plain name"
    );
    assert_eq!(
        claim("2026alpha.wav"),
        Some(("2026alpha".into(), "2026alpha".into(), None)),
        "digits without a separator are part of the name"
    );
    assert_eq!(
        claim("99999999999_big.wav"),
        Some(("99999999999_big".into(), "99999999999_big".into(), None)),
        "a number past i32 is not a claim"
    );
    assert_eq!(claim("readme.txt"), None);
    assert_eq!(claim(".wav"), None);
}

/// A refused file is named with its reason rather than dropped in silence, and a non-WAV file is
/// not a refusal at all — the folder may hold a readme.
#[test]
fn bad_files_are_reported_and_other_files_ignored() {
    let dir = temp_dir("folder");
    std::fs::write(dir.join("ding1.wav"), pcm_wav(400)).unwrap();
    std::fs::write(dir.join("Extra.wav"), pcm_wav(400)).unwrap();
    std::fs::write(dir.join("broken.wav"), b"not a wav at all").unwrap();
    std::fs::write(dir.join("notes.txt"), b"ignored").unwrap();
    let catalog = sources::scan(&dir);
    assert_eq!(catalog.stats().folder_files, 2);
    let ding1 = catalog.find("ding1").unwrap();
    assert_eq!(ding1.source, Source::Folder);
    assert_eq!(ding1.duration, Duration::from_millis(50));
    assert_eq!(
        catalog.by_ordinal(12).map(|e| e.source),
        Some(Source::Folder),
        "overriding the bytes must keep the sound's ordinal"
    );
    assert_eq!(catalog.stats().rejected.len(), 1);
    assert_eq!(catalog.stats().rejected[0].name, "broken.wav");
    assert_eq!(catalog.stats().rejected[0].reason, RejectReason::NotPcmWav);
    assert!(catalog.find("broken").is_none());
    // Extras come after Moonbot's block, so the pickers list the familiar names first.
    let last = catalog.entries().last().unwrap();
    assert_eq!(last.stem, "extra");
    assert_eq!(catalog.entries().len(), MB_SOUNDS.len() + 1);
    let _ = std::fs::remove_dir_all(&dir);
}

/// No folder at all is the common case, and it is the embedded set — scanned, not pending.
#[test]
fn a_missing_folder_is_the_embedded_set() {
    let dir = temp_dir("absent").join("nope");
    let catalog = sources::scan(&dir);
    assert!(catalog.scanned());
    assert_eq!(catalog.entries().len(), MB_SOUNDS.len());
    assert!(catalog.stats().rejected.is_empty());
}

/// A name no file answers to plays the default and is reported exactly once — in any spelling
/// of that name, since the catalog would have matched them all; an empty name is an explicit
/// mute and plays nothing.
///
/// Plausible breakage: returning `None` for the unknown name is the old behaviour — the strategy
/// asked for a sound and the operator heard nothing, with no way to tell which setting failed.
#[test]
fn an_unknown_name_falls_back_and_is_reported_once() {
    install(sources::scan(&temp_dir("fallback").join("nope")));
    let fallback = clip(DEFAULT_SOUND);
    let resolved = resolve(MissingSound::Name("nosuch".into())).expect("falls back");
    assert!(Arc::ptr_eq(&resolved.wav, &fallback.wav));
    assert!(resolve(MissingSound::Name("   ".into())).is_none());
    play("nosuch");
    play("NOSUCH.wav");
    play_ordinal(99);
    let notices = take_missing_notices();
    assert_eq!(
        notices,
        vec![
            MissingSound::Name("nosuch".into()),
            MissingSound::Ordinal(99),
        ],
        "one missing sound in two spellings is one toast, worded as first asked for"
    );
    play("nosuch");
    assert!(
        take_missing_notices().is_empty(),
        "the same miss must not toast again this session"
    );
    assert!(!is_playable("nosuch"));
    super::discard_pending();
}

/// A miss before the first scan waits for it: the folder may hold the file. Once the scan lands,
/// what it still cannot answer becomes a notice, and what it can does not.
#[test]
fn a_miss_before_the_scan_is_settled_by_it() {
    with_catalog(|c| assert!(!c.scanned(), "a fresh thread starts unscanned"));
    let dir = temp_dir("pending");
    std::fs::write(dir.join("Later.wav"), pcm_wav(400)).unwrap();
    play("later");
    play("never");
    assert!(
        take_missing_notices().is_empty(),
        "nothing is reported before the folder has been read"
    );
    install(sources::scan(&dir));
    assert_eq!(
        take_missing_notices(),
        vec![MissingSound::Name("never".into())],
        "only the name the scanned folder still lacks is reported"
    );
    assert!(is_playable("later"));
    assert!(stems().contains(&"later".to_string()));
    missing::reset_seen();
    super::discard_pending();
    let _ = std::fs::remove_dir_all(&dir);
}

/// A repeat count for [`Playback::enqueue_detect`].
fn copies(n: u32) -> NonZeroU32 {
    NonZeroU32::new(n).expect("a test asks for at least one play")
}

/// The tick the scheduler simulation pumps at: the order of the application's coordination timer.
const TICK: Duration = Duration::from_millis(100);

/// The output-device allowance the scheduler adds after every clip.
const GAP: Duration = Duration::from_millis(50);

/// Pump a scheduler over virtual time the way the coordination timer does, with a backend trade
/// slot beside it that the scheduler consumes only when it picks the trade.
///
/// Args:
///     player: The scheduler under test.
///     horizon: How long to keep pumping.
///     arrive: Called on every tick before the pump, with the tick's offset, the scheduler and
///         the trade slot, to model what the feed delivers at that moment.
///
/// Returns:
///     Every clip started, as `(offset, is_trade, bytes)`, in start order.
fn simulate(
    player: &mut Playback,
    horizon: Duration,
    mut arrive: impl FnMut(Duration, &mut Playback, &mut Option<Clip>),
) -> Vec<(Duration, bool, Arc<[u8]>)> {
    let start = std::time::Instant::now();
    let mut trade = None;
    let mut starts = Vec::new();
    let mut t = Duration::ZERO;
    while t < horizon {
        arrive(t, player, &mut trade);
        if let Some((clip, is_trade)) = player.next(start + t, trade.clone()) {
            if is_trade {
                trade = None;
            }
            starts.push((t, is_trade, clip.wav));
        }
        t += TICK;
    }
    starts
}

/// Issue #1447: twenty detects that each landed in their own drain must not replay for minutes.
///
/// Plausible breakage: going back to a FIFO behind the playing clip — the last of twenty `ding2`
/// detects then started 96.9 s after the burst.
#[test]
fn lane_1447_a_burst_does_not_replay_in_full() {
    let mut player = Playback::default();
    let detect = clip("ding2");
    // One detect per drain, a tick apart: the first starts at once, nineteen arrive behind it.
    let starts = simulate(&mut player, Duration::from_secs(300), |t, player, _| {
        if t < TICK * 20 {
            player.enqueue_detect(detect.clone(), copies(1));
        }
    });
    let last_start = starts.last().map(|s| s.0).unwrap_or_default();
    eprintln!(
        "#1447 burst of 20 detects: {} starts, last at {last_start:?}",
        starts.len()
    );
    // The playing clip finishes and at most the latest one follows it.
    let bound = (detect.duration + GAP) * 2;
    assert!(
        last_start <= bound,
        "ordinary sounds still starting {last_start:?} after the burst (bound {bound:?})"
    );
    assert_eq!(
        starts.len(),
        2,
        "the clip that started, then the newest detect"
    );
}

/// The issue's own arithmetic, twenty detects with twenty trade opens arriving drain by drain:
/// about two minutes as a FIFO. Behind the first clip wait one trade edge and one detect, still
/// alternating, so the burst lasts one detect, one open and one detect.
///
/// Plausible breakage: keeping a FIFO in either lane, or letting a newer detect take the trade's
/// turn.
#[test]
fn a_burst_with_trades_ends_after_one_turn_of_each_kind() {
    let mut player = Playback::default();
    let detect = clip("ding2");
    let open = clip("ringin");
    let starts = simulate(&mut player, Duration::from_secs(300), |t, player, trade| {
        if t < TICK * 20 {
            player.enqueue_detect(detect.clone(), copies(1));
            *trade = Some(open.clone());
        }
    });
    let last_start = starts.last().map(|s| s.0).unwrap_or_default();
    eprintln!(
        "#1447 burst of 20 detects + 20 opens: {} starts, last at {last_start:?}",
        starts.len()
    );
    assert_eq!(
        starts.iter().map(|s| s.1).collect::<Vec<_>>(),
        [false, true, false],
        "the first detect, the newest open, the newest detect — nothing else"
    );
    let bound = detect.duration + open.duration + GAP * 2 + TICK * 2;
    assert!(
        last_start <= bound,
        "still starting {last_start:?} (bound {bound:?})"
    );
}

/// Ordinary sounds get the first turn, but cannot cut or indefinitely starve a waiting trade; a
/// notice goes before a waiting detect; a clip waiting in its slot plays without new events.
///
/// Rewritten for #1447: it used to queue two clips FIFO in one ordinary lane. The notice and the
/// detect now wait in their own slots, the notice first, and a detect re-sent while the trade
/// plays replaces the waiting one instead of queuing a second copy.
#[test]
fn contention_alternates_complete_clips_and_progresses_without_new_events() {
    use std::time::Instant;
    let mut player = Playback::default();
    let detect = clip("babytoy");
    let alert = clip("pfiff");
    let open = clip("ding1");
    let close = clip("ding2");
    player.enqueue_detect(detect.clone(), copies(1));
    player.enqueue(alert.clone());
    let start = Instant::now();
    let (first, is_trade) = player.next(start, Some(open.clone())).unwrap();
    assert!(!is_trade);
    assert!(
        Arc::ptr_eq(&first.wav, &alert.wav),
        "a notice never waits behind a detect"
    );
    assert!(
        player
            .next(start + alert.duration, Some(open.clone()))
            .is_none()
    );
    let open_at = start + alert.duration + GAP;
    assert!(player.next(open_at, Some(open.clone())).unwrap().1);
    // Repeated producer traffic while the trade plays never interrupts it, and the re-sent detect
    // takes the waiting one's place rather than queuing behind it.
    assert_eq!(
        player.enqueue_detect(detect.clone(), copies(1)),
        Some(copies(1))
    );
    assert!(
        player
            .next(open_at + Duration::from_millis(500), Some(close.clone()))
            .is_none()
    );
    let detect_at = open_at + open.duration + GAP;
    let (next, is_trade) = player.next(detect_at, Some(close.clone())).unwrap();
    assert!(!is_trade);
    assert!(Arc::ptr_eq(&next.wav, &detect.wav));
    let close_at = detect_at + detect.duration + GAP;
    assert!(player.next(close_at, Some(close.clone())).unwrap().1);
    player.enqueue(alert.clone());
    assert!(
        player
            .next(close_at + Duration::from_secs(5), None)
            .is_none(),
        "the five-second close is never cut"
    );
    let (after, is_trade) = player.next(close_at + close.duration + GAP, None).unwrap();
    assert!(!is_trade);
    assert!(Arc::ptr_eq(&after.wav, &alert.wav));
    assert!(
        player
            .next(close_at + close.duration + GAP + alert.duration + GAP, None)
            .is_none(),
        "the burst is over: nothing replays"
    );
}

/// Saturating the detect lane cannot cut the clip on the device or take a pending trade turn,
/// and leaves exactly one clip waiting — the newest.
///
/// Rewritten for #1447 (was `ordinary_queue_is_bounded_without_evicting_accepted_clips`): it
/// pinned a 64-clip FIFO whose overflow dropped the NEWEST sound. The clip that already started
/// is what is now protected, and the newest is what waits.
#[test]
fn a_detect_burst_never_cuts_the_playing_clip_and_waits_as_its_newest() {
    use std::time::Instant;
    let mut player = Playback::default();
    let first = clip("babytoy");
    let newest = clip("ding1");
    player.enqueue_detect(first.clone(), copies(1));
    let now = Instant::now();
    assert!(Arc::ptr_eq(
        &player.next(now, None).unwrap().0.wav,
        &first.wav
    ));
    for _ in 0..1000 {
        player.enqueue_detect(clip("ding2"), copies(1));
    }
    player.enqueue_detect(newest.clone(), copies(1));
    assert!(
        player.next(now + Duration::from_millis(1), None).is_none(),
        "the burst never cuts the clip already playing"
    );
    let free = now + first.duration + GAP;
    assert!(
        player.next(free, Some(clip("pfiff"))).unwrap().1,
        "the trade keeps its turn against a full ordinary lane"
    );
    let (next, _) = player.next(free + Duration::from_secs(2), None).unwrap();
    assert!(Arc::ptr_eq(&next.wav, &newest.wav));
    assert!(
        player
            .next(free + Duration::from_secs(2) + newest.duration + GAP, None)
            .is_none(),
        "one clip waited behind the first, not a thousand"
    );
}

/// A trade waiting in its slot is never dropped because a detect is newer, whichever side holds
/// the turn: losing the turn does not consume the edge.
///
/// Plausible breakage: one shared "latest sound" slot — the burst's newest detect would erase
/// "the position opened".
#[test]
fn a_newer_detect_never_takes_a_waiting_trade() {
    let mut player = Playback::default();
    let detect = clip("ding2");
    let open = clip("ringin");
    let starts = simulate(&mut player, Duration::from_secs(60), |t, player, trade| {
        // The first open plays at once and hands the next turn to the ordinary lane; the second
        // waits while a detect is offered that turn and every later one.
        if t == Duration::ZERO || t == TICK {
            *trade = Some(open.clone());
        }
        if t >= TICK && t < TICK * 30 {
            player.enqueue_detect(detect.clone(), copies(1));
        }
    });
    assert_eq!(
        starts.iter().map(|s| s.1).collect::<Vec<_>>(),
        [true, false, true, false],
        "open, detect, the waiting open, the newest detect"
    );
}

/// A figure alert's `Повторять звук` copies are one event: alone, all of them play; a newer
/// detect replaces whatever copies are still owed.
///
/// Plausible breakage: storing the event as a single clip turns a stored repeat of 2 into one
/// play; storing copies as separate clips lets a burst queue behind them again.
#[test]
fn figure_alert_repeats_are_one_event() {
    let alert = clip("pfiff");
    let detect = clip("ding1");
    for n in [2, 20] {
        let mut player = Playback::default();
        assert_eq!(player.enqueue_detect(alert.clone(), copies(n)), None);
        let starts = simulate(&mut player, Duration::from_secs(60), |_, _, _| {});
        assert_eq!(starts.len(), n as usize, "an empty queue plays every copy");
        assert!(starts.iter().all(|s| Arc::ptr_eq(&s.2, &alert.wav)));
    }
    let mut player = Playback::default();
    player.enqueue_detect(alert.clone(), copies(3));
    let starts = simulate(&mut player, Duration::from_secs(60), |t, player, _| {
        if t == TICK {
            assert_eq!(
                player.enqueue_detect(detect.clone(), copies(1)),
                Some(copies(2)),
                "the newer detect replaces the two copies still owed"
            );
        }
    });
    assert_eq!(starts.len(), 2);
    assert!(Arc::ptr_eq(&starts[0].2, &alert.wav));
    assert!(Arc::ptr_eq(&starts[1].2, &detect.wav));
}

/// The network alarm and the price-approach beep wait in the notice lane: a detect never erases
/// one, and one never sits behind detects, in whichever order they arrived.
///
/// Plausible breakage: a priority that only holds when the notice arrives last, or a notice kept
/// in the detect slot where the next detect replaces it.
#[test]
fn a_notice_is_never_replaced_by_a_detect_nor_waits_behind_one() {
    // Five seconds on the device, so the whole burst below arrives while it plays.
    let playing = clip("ding2");
    let alarm = clip("alarm");
    let detect = clip("ding1");
    for notice_first in [true, false] {
        let mut player = Playback::default();
        player.enqueue(playing.clone());
        let starts = simulate(&mut player, Duration::from_secs(60), |t, player, _| {
            if t == TICK && notice_first {
                player.enqueue(alarm.clone());
            }
            if t >= TICK && t < TICK * 20 {
                player.enqueue_detect(detect.clone(), copies(1));
            }
            if t == TICK * 19 && !notice_first {
                player.enqueue(alarm.clone());
            }
        });
        let heard: Vec<_> = starts.iter().map(|s| s.2.clone()).collect();
        assert_eq!(heard.len(), 3, "playing clip, alarm, newest detect");
        assert!(Arc::ptr_eq(&heard[0], &playing.wav));
        assert!(Arc::ptr_eq(&heard[1], &alarm.wav), "the alarm goes first");
        assert!(Arc::ptr_eq(&heard[2], &detect.wav));
    }
}

/// A figure alert whose repeat is zero is silence and leaves a detect already waiting in place.
///
/// Plausible breakage: clearing the slot for a zero-repeat alert would silence the detect before
/// it.
#[test]
fn a_zero_repeat_alert_leaves_the_waiting_detect() {
    use super::{PLAYBACK, discard_pending};
    discard_pending();
    let waiting = clip("ding1");
    play_detect("ding1", 1);
    play_detect("pfiff", 0);
    PLAYBACK.with(|player| {
        let player = player.borrow();
        let event = player.detect.as_ref().expect("the detect still waits");
        assert!(Arc::ptr_eq(&event.clip.wav, &waiting.wav));
        assert_eq!(event.left, copies(1));
    });
    discard_pending();
}

/// Distinct notices each keep their turn — several warning axes opening in one tick, an alarm
/// and a beep — while a repeat of a sound already waiting folds into it, and past the lane's
/// size the OLDEST is dropped so the newest is always heard.
///
/// Plausible breakage: a single latest-wins notice slot (a beep erases a waiting alarm, and only
/// the last of several warning axes sounds); a FIFO without folding (a burst of alarms across
/// drains replays in full); overflow that drops the newest, as the old FIFO did.
#[test]
fn distinct_notices_each_wait_once_and_overflow_drops_the_oldest() {
    let playing = clip("ding2");
    let alarm = clip("alarm");
    let beep = clip("pfiff");
    let warn = clip("hallo");
    let mut player = Playback::default();
    player.enqueue(playing.clone());
    let starts = simulate(&mut player, Duration::from_secs(60), |t, player, _| {
        if t == TICK {
            player.enqueue(alarm.clone());
            player.enqueue(beep.clone());
            player.enqueue(warn.clone());
        }
        if t > TICK && t < TICK * 20 {
            assert!(!player.enqueue(alarm.clone()), "a repeat drops nothing");
        }
    });
    let heard: Vec<_> = starts.iter().map(|s| s.2.clone()).collect();
    assert_eq!(
        heard.len(),
        4,
        "playing clip, then alarm, beep and warning once each"
    );
    for (got, want) in heard.iter().zip([&playing, &alarm, &beep, &warn]) {
        assert!(Arc::ptr_eq(got, &want.wav), "arrival order is kept");
    }

    let mut player = Playback::default();
    let names = ["alarm", "pfiff", "hallo", "ding1", "babytoy", "ringin"];
    let dropped: Vec<bool> = names.iter().map(|n| player.enqueue(clip(n))).collect();
    assert_eq!(dropped, [false, false, false, false, true, true]);
    let starts = simulate(&mut player, Duration::from_secs(60), |_, _, _| {});
    let heard: Vec<_> = starts.iter().map(|s| s.2.clone()).collect();
    assert_eq!(heard.len(), super::NOTICE_SLOTS);
    for (got, name) in heard.iter().zip(&names[2..]) {
        assert!(
            Arc::ptr_eq(got, &clip(name).wav),
            "the newest four are kept"
        );
    }
}

/// Routing a trade preview through ordinary `play` would ignore its gain; scaling the shared
/// catalog instead would also lower subsequent detect/alert sounds using the same file. A second
/// click on the same preview folds into the first although its bytes are a fresh buffer.
#[test]
fn trade_preview_gain_leaves_ordinary_playback_and_catalog_unchanged() {
    super::discard_pending();
    let original = clip("ringin");
    super::play_trade_preview("ringin", 37);
    play("ringin");
    super::play_trade_preview("ringin", 37);
    super::PLAYBACK.with(|player| {
        assert_eq!(
            player.borrow().notices.len(),
            2,
            "a repeated preview click is the same sound, not a third clip"
        );
    });
    super::PLAYBACK.with(|player| {
        let player = player.borrow();
        let preview = &player.notices[0];
        let ordinary = &player.notices[1];
        assert_eq!(
            preview.wav,
            super::volume::scale(original.wav.clone(), 37).unwrap()
        );
        assert!(!Arc::ptr_eq(&preview.wav, &original.wav));
        assert!(Arc::ptr_eq(&ordinary.wav, &original.wav));
        assert_eq!(preview.duration, original.duration);
    });
    assert!(Arc::ptr_eq(&clip("ringin").wav, &original.wav));
    super::discard_pending();
}

/// Entering quiet spends the old backlog — notice and detect alike — without rejecting later
/// producer-approved exceptions.
#[test]
fn quiet_transition_discards_backlog_but_accepts_later_bypass() {
    use super::{PLAYBACK, discard_pending};
    play_detect("ding1", 2);
    play("ding2");
    discard_pending();
    PLAYBACK.with(|player| assert!(!player.borrow().ordinary_waiting()));
    // Simulates a detect already approved by the existing quiet-bypass policy.
    play_detect("pfiff", 1);
    PLAYBACK.with(|player| {
        let player = player.borrow();
        assert!(player.notices.is_empty());
        assert_eq!(player.detect.as_ref().map(|e| e.left), Some(copies(1)));
    });
    discard_pending();
}
