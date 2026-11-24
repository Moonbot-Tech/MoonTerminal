//! Alert and detection sound playback over the sound [`catalog`]: the eighteen WAVs embedded in
//! the binary, plus whatever `.wav` files the user drops into the sounds folder (`sources`).
//!
//! Each platform plays with what it already links, so this adds no audio dependency: Windows uses
//! WinMM `PlaySoundW` with `SND_MEMORY | SND_ASYNC`, macOS uses AppKit `NSSound` over an `NSData`
//! view of the same bytes. Both are asynchronous, and on both a new sound replaces the one already
//! playing rather than mixing with it. The scheduler below serializes every caller, including
//! previews, before reaching those platform functions. Linux remains silent — nothing there is
//! linked that can play audio, and adding one is a dependency decision of its own.
//!
//! A sound is chosen by file stem — a strategy's `SoundKind`, a trade-sound setting — or by
//! Moonbot's 1-based ordinal, which the core's own settings carry. Lookup trims whitespace and is
//! ASCII case-insensitive, so `BABYTOY` and `ding1` match their files. A name or number the
//! catalog cannot answer plays [`DEFAULT_SOUND`] and leaves a notice (`missing`) for the shell to
//! show: the one outcome this module refuses is silence about a setting that asked for a sound.

use std::cell::RefCell;
use std::sync::Arc;

use gpui::{App, AppContext as _};

mod catalog;
mod embedded;
mod missing;
mod sources;
mod volume;

#[cfg(test)]
use catalog::Source;
pub(crate) use catalog::{Catalog, Entry, RejectReason, ScanStats};
pub use embedded::DEFAULT_SOUND;
pub(crate) use missing::{MissingSound, take as take_missing_notices};

thread_local! {
    /// The installed sound set. Same thread as playback; the scan builds a replacement off-thread
    /// and hands it over whole through [`install`].
    static CATALOG: RefCell<Catalog> = RefCell::new(Catalog::embedded());
}

/// Reads the installed catalog.
pub(crate) fn with_catalog<R>(read: impl FnOnce(&Catalog) -> R) -> R {
    CATALOG.with(|catalog| read(&catalog.borrow()))
}

/// Rebuilds the catalog from the sounds folder on a background thread and installs the result;
/// `done` runs on the UI thread afterwards, for a view that wants to redraw its stats. Called once
/// at boot and again from the Settings "Rescan" button.
pub(crate) fn rescan(cx: &mut App, done: impl FnOnce(&mut App) + 'static) {
    let dir = moon_core::config::paths::sounds_dir();
    cx.spawn(async move |cx| {
        let catalog = cx
            .background_spawn(async move { sources::scan(&dir) })
            .await;
        cx.update(|cx| {
            install(catalog);
            done(cx);
        });
    })
    .detach();
}

/// Swaps the catalog in and settles the notices that waited for it. A rescan also forgets which
/// names were reported, so a file still missing after the user "fixed" the folder is named again.
fn install(catalog: Catalog) {
    let stats = catalog.stats();
    log::info!(
        "sounds: {} in the catalog, {} from {}, {} rejected{}",
        catalog.entries().len(),
        stats.folder_files,
        stats.dir.display(),
        stats.rejected.len(),
        stats
            .rejected
            .iter()
            .map(|r| format!(" [{}: {:?}]", r.name, r.reason))
            .collect::<String>()
    );
    CATALOG.with(|slot| *slot.borrow_mut() = catalog);
    missing::reset_seen();
    missing::settle_pending(|m| {
        with_catalog(|c| match m {
            MissingSound::Name(name) => c.find(name).is_none(),
            MissingSound::Ordinal(n) => c.by_ordinal(*n).is_none(),
        })
    });
}

/// Stems in catalog order — Moonbot's table, then the user's extras — for the sound pickers.
pub(crate) fn stems() -> Vec<String> {
    with_catalog(|c| c.entries().iter().map(|e| e.stem.clone()).collect())
}

/// The display name of a stem, as its file spells it, or `None` for a stem no file answers to.
pub(crate) fn label_of(name: &str) -> Option<String> {
    with_catalog(|c| c.find(name).map(|e| e.label.clone()))
}

/// Moonbot's label for a 1-based sound ordinal, or `None` when the table has no such row.
///
/// `None` rather than a fallback to the first sound: a core carrying an ordinal we cannot name is a
/// core whose sound list differs from ours, and showing "Alarm" for it would write that back on the
/// next OK and silently change the user's setting. (Playing it is another matter: see
/// [`play_ordinal`].)
pub(crate) fn mb_sound_name(ordinal: i32) -> Option<String> {
    with_catalog(|c| c.by_ordinal(ordinal).map(|e| e.label.clone()))
}

/// The smallest number a user's file may claim — the one after Moonbot's eighteen — for the
/// texts that tell the user how to name a file.
pub(crate) const FIRST_USER_ORDINAL: i32 = Catalog::FIRST_USER_ORDINAL;

/// Names of the sounds built into every install, Moonbot's own eighteen.
#[cfg(test)]
pub(crate) fn builtin_names() -> &'static [&'static str] {
    embedded::MB_SOUNDS
}

/// The ordinal table as `(ordinal, stem, label)`, for the core-settings sound pickers.
pub(crate) fn ordinals() -> Vec<(i32, String, String)> {
    with_catalog(|c| {
        c.ordinals()
            .map(|(n, e)| (n, e.stem.clone(), e.label.clone()))
            .collect()
    })
}

/// Whether a file answers to this name. Preview buttons ask before enabling themselves; the
/// playback paths do not — they fall back instead.
pub(crate) fn is_playable(name: &str) -> bool {
    with_catalog(|c| c.find(name).is_some())
}

/// Whether the folder scan has landed and still has no file for `name`.
///
/// Before the scan this is false: the embedded table alone cannot say a name the user's folder
/// may hold is missing, which is the same hold-back the missing-sound notices use.
///
/// Args:
///     name: A sound name in any spelling [`Catalog::find`] matches.
///
/// Returns:
///     `true` only after a scan, when no catalog entry answers to `name`.
pub(crate) fn absent_after_scan(name: &str) -> bool {
    with_catalog(|catalog| catalog.scanned() && catalog.find(name).is_none())
}

/// A name or ordinal to a clip. A request the catalog cannot answer yields the default clip and
/// is recorded as missing — held back until the first folder scan has landed, since the embedded
/// set alone cannot say a name is absent. `None` only for an explicit "no sound" — an empty name.
fn resolve(lookup: MissingSound) -> Option<Clip> {
    let (clip, missed) = with_catalog(|c| {
        let found = match &lookup {
            MissingSound::Name(name) if name.trim().is_empty() => return (None, false),
            MissingSound::Name(name) => c.find(name),
            MissingSound::Ordinal(n) => c.by_ordinal(*n),
        };
        match found {
            Some(entry) => (Some(Clip::from(entry)), false),
            // The fallback plays NOW even before the scan: a sound that fires is better than one
            // that waits for a directory listing.
            None => (c.default_entry().map(Clip::from), true),
        }
    });
    if missed {
        missing::note(lookup, with_catalog(|c| c.scanned()));
    }
    clip
}

/// Queue a named notice sound without interrupting the current clip. The application's
/// coordination timer pumps this lane, including Settings previews when no market events arrive.
/// A name no file answers to plays the default and is reported once.
///
/// This is the notice lane — warnings, alarms, previews — which a detect never replaces; core
/// detects go through [`play_detect`] instead (issue #1447).
pub fn play(name: &str) {
    if let Some(clip) = resolve(MissingSound::Name(name.to_string())) {
        queue_notice(clip);
    }
}

/// Hand a notice to the scheduler and log the one it had to drop, if any.
///
/// The drop is logged because "it did not beep" is diagnosed from the detect log, where the alarm
/// and the price-approach beep already write their own lines; a notice dropped before its turn
/// would otherwise read as played.
fn queue_notice(clip: Clip) {
    if PLAYBACK.with(|player| player.borrow_mut().enqueue(clip)) {
        moon_core::detect_diag::line(&format!(
            "[sound] notice lane full ({NOTICE_SLOTS} distinct sounds waiting); the oldest dropped"
        ));
    }
}

/// Queue one detect or figure-alert sound as a single event of `copies` plays, replacing any
/// detect event still waiting behind the current clip.
///
/// Why latest-wins: the detect cards already list every strategy that fired, so the sound is the
/// tap that something happened. Queuing each detect of a burst replayed events that were long
/// over — twenty five-second clips signalled for about two minutes (issue #1447). The clip already
/// playing is never cut; only the waiting detect is replaced, whole, including the remaining
/// copies of a figure alert's `Повторять звук`. With nothing newer arriving, all `copies` play.
///
/// Args:
///     name: The sound's stem; a name no file answers to plays the default and is reported once.
///     copies: How many times the event plays; zero is silence and leaves a waiting detect alone.
pub(crate) fn play_detect(name: &str, copies: u32) {
    let Some(copies) = std::num::NonZeroU32::new(copies) else {
        return;
    };
    if let Some(clip) = resolve(MissingSound::Name(name.to_string())) {
        let replaced = PLAYBACK.with(|player| player.borrow_mut().enqueue_detect(clip, copies));
        if let Some(left) = replaced {
            // "It did not beep" is diagnosed from this log; a detect that was queued but replaced
            // before its turn has to say so, or it reads as played.
            moon_core::detect_diag::line(&format!(
                "[sound] {name} replaced a waiting detect sound ({left} play(s) left)"
            ));
        }
    }
}

/// Queue a trade preview at the chosen gain without changing ordinary notification sounds.
pub(crate) fn play_trade_preview(name: &str, percent: u8) {
    if let Some(mut clip) = resolve(MissingSound::Name(name.to_string())) {
        let Some(wav) = volume::scale(clip.wav, percent) else {
            log::warn!("trade preview skipped: unsupported PCM sample format");
            return;
        };
        clip.wav = wav;
        queue_notice(clip);
    }
}

/// Play the sound a 1-based Moonbot ordinal names; one past the table plays the default and is
/// reported once. It waits in the notice lane — the network alarm and the price-approach beep
/// come through here — so a detect neither replaces it nor goes before it.
pub fn play_ordinal(ordinal: i32) {
    if let Some(clip) = resolve(MissingSound::Ordinal(ordinal)) {
        queue_notice(clip);
    }
}

/// One validated clip; duration is derived from PCM frames, not a fixed timeout.
#[derive(Clone)]
struct Clip {
    wav: Arc<[u8]>,
    duration: std::time::Duration,
}

impl From<&Entry> for Clip {
    fn from(entry: &Entry) -> Self {
        Self {
            wav: entry.wav.clone(),
            duration: entry.duration,
        }
    }
}

/// Read RIFF chunks, including odd-length padding, and measure PCM frames at the sample rate.
fn wav_duration(wav: &[u8]) -> Option<std::time::Duration> {
    if wav.get(..4)? != b"RIFF" || wav.get(8..12)? != b"WAVE" {
        return None;
    }
    let end = 8usize.checked_add(u32::from_le_bytes(wav.get(4..8)?.try_into().ok()?) as usize)?;
    if end > wav.len() {
        return None;
    }
    let mut offset = 12usize;
    let mut format = None;
    let mut data_len = 0u64;
    while offset.checked_add(8)? <= end {
        let tag = wav.get(offset..offset + 4)?;
        let len = u32::from_le_bytes(wav.get(offset + 4..offset + 8)?.try_into().ok()?) as usize;
        offset += 8;
        let chunk_end = offset.checked_add(len)?;
        if chunk_end > end {
            return None;
        }
        let chunk = wav.get(offset..chunk_end)?;
        if tag == b"fmt " {
            let encoding = u16::from_le_bytes(chunk.get(..2)?.try_into().ok()?);
            let rate = u32::from_le_bytes(chunk.get(4..8)?.try_into().ok()?);
            let alignment = u16::from_le_bytes(chunk.get(12..14)?.try_into().ok()?);
            if encoding != 1 || rate == 0 || alignment == 0 {
                return None;
            }
            format = Some((rate, alignment));
        } else if tag == b"data" {
            data_len = data_len.checked_add(len as u64)?;
        }
        offset = chunk_end.checked_add(len & 1)?;
    }
    let (rate, alignment) = format?;
    if data_len == 0 || !data_len.is_multiple_of(u64::from(alignment)) {
        return None;
    }
    let frames = data_len / u64::from(alignment);
    let nanos = frames.checked_mul(1_000_000_000)?.div_ceil(u64::from(rate));
    Some(std::time::Duration::from_nanos(nanos))
}

/// One detect event waiting to play: the clip and how many plays of it are still owed.
///
/// A figure alert's `Повторять звук` copies are ONE event, so they live in one entry that a newer
/// detect replaces whole rather than as separate clips a newer detect would queue behind.
struct DetectEvent {
    clip: Clip,
    left: std::num::NonZeroU32,
}

/// How many distinct notice sounds may wait at once. Past it the oldest is dropped, so the newest
/// is always heard — the opposite of the old FIFO, which dropped the fresh sound on overflow.
const NOTICE_SLOTS: usize = 4;

/// The playback scheduler: the clip on the device always finishes, and behind it waits the
/// newest news of each kind — never the backlog of a burst (issue #1447).
///
/// Windows `PlaySound` and macOS `NSSound` replace the clip already playing rather than mixing,
/// so clips are serialized; what is bounded is the wait behind the playing one. Three kinds wait
/// apart so that one cannot erase another's news:
///
/// - `notices` — the network-problem alarm, the price-approach beep, core warnings and previews.
///   Each distinct sound waits once: a repeat of one already waiting folds into it, so a burst of
///   alarms is one alarm, while an alarm, a beep and a warning axis are different news and each
///   plays. A detect never replaces one and they go before a waiting detect, so an alarm never
///   sits behind stale detects.
/// - `detect` — the newest core detect or figure alert, with its remaining repeat copies.
/// - the trade edge — held by the backend's trade lane and offered on every [`Playback::next`],
///   so a newer detect cannot drop "the position opened".
///
/// The ordinary kinds and the trade edge alternate when both wait, as before.
#[derive(Default)]
struct Playback {
    notices: std::collections::VecDeque<Clip>,
    detect: Option<DetectEvent>,
    busy_until: Option<std::time::Instant>,
    trade_turn: bool,
}

impl Playback {
    /// Queue a notice unless the same sound already waits, dropping the oldest past
    /// [`NOTICE_SLOTS`].
    ///
    /// Distinct rather than latest-wins: the notice producers are different news — several
    /// warning axes open in one tick, each with its own sound, and an alarm must not be erased by
    /// a beep. The same sound again is the same news, so it folds into the one waiting, which is
    /// what keeps a burst of alarms across drains from replaying the way detects used to.
    ///
    /// Returns:
    ///     Whether a waiting notice was dropped to make room.
    fn enqueue(&mut self, clip: Clip) -> bool {
        // "The same sound" is the same BYTES, not the same allocation: a trade preview is scaled
        // into a fresh buffer on every click, and a rescan rebuilds every entry. `Arc` equality
        // checks the pointer first, so the common case stays a pointer compare.
        if self.notices.iter().any(|w| w.wav == clip.wav) {
            return false;
        }
        let dropped = self.notices.len() >= NOTICE_SLOTS;
        if dropped {
            self.notices.pop_front();
        }
        self.notices.push_back(clip);
        dropped
    }

    /// Put a detect event in its slot, replacing the one waiting there, whole.
    ///
    /// Returns:
    ///     The plays the replaced event still owed, or `None` when the slot was empty.
    fn enqueue_detect(
        &mut self,
        clip: Clip,
        copies: std::num::NonZeroU32,
    ) -> Option<std::num::NonZeroU32> {
        self.detect
            .replace(DetectEvent { clip, left: copies })
            .map(|replaced| replaced.left)
    }

    /// Whether an ordinary clip — a notice or a detect — waits for a turn.
    fn ordinary_waiting(&self) -> bool {
        !self.notices.is_empty() || self.detect.is_some()
    }

    /// Take the next ordinary clip: the oldest notice first, so a pending alarm never waits behind
    /// a detect, then one copy of the waiting detect event, keeping the rest of its copies.
    fn take_ordinary(&mut self) -> Option<Clip> {
        if let Some(clip) = self.notices.pop_front() {
            return Some(clip);
        }
        let event = self.detect.as_mut()?;
        let clip = event.clip.clone();
        match std::num::NonZeroU32::new(event.left.get() - 1) {
            Some(left) => event.left = left,
            None => self.detect = None,
        }
        Some(clip)
    }

    /// Called by the application timer even without feed traffic. The small output-device
    /// allowance prevents timer granularity from starting the next clip before its last frame.
    fn next(&mut self, now: std::time::Instant, trade: Option<Clip>) -> Option<(Clip, bool)> {
        if self.busy_until.is_some_and(|until| now < until) {
            return None;
        }
        let is_trade = trade.is_some() && (self.trade_turn || !self.ordinary_waiting());
        let clip = if is_trade {
            trade?
        } else {
            self.take_ordinary()?
        };
        self.trade_turn = !is_trade;
        self.busy_until = Some(now + clip.duration + std::time::Duration::from_millis(50));
        Some((clip, is_trade))
    }

    /// Drop every waiting ordinary clip; the clip on the device and the trade lane are untouched.
    fn discard_ordinary(&mut self) {
        self.notices.clear();
        self.detect = None;
    }
}

thread_local! {
    /// All playback callers run on the GPUI thread; no lock or sleeping thread is required.
    static PLAYBACK: RefCell<Playback> = RefCell::new(Playback::default());
}

/// Spend pre-sleep ordinary backlog once at the quiet transition. Later producer-authorized
/// quiet exceptions may enqueue normally, and the current asynchronous clip is left to finish.
pub(crate) fn discard_pending() {
    PLAYBACK.with(|player| player.borrow_mut().discard_ordinary());
}

/// Offer the waiting trade edge and advance one fair, noninterrupting playback turn. A trade sound
/// whose file is gone plays the default and is reported, like every other lane.
/// Returns true only when the trade was selected, so contention never consumes its edge.
/// The current trade gain applies at playback time; ordinary detects and alerts keep their bytes.
pub(crate) fn pump(trade: Option<&str>, trade_volume: u8) -> bool {
    let trade = trade.and_then(|name| resolve(MissingSound::Name(name.to_string())));
    let next = PLAYBACK.with(|player| player.borrow_mut().next(std::time::Instant::now(), trade));
    if let Some((clip, is_trade)) = next {
        let wav = if is_trade {
            volume::scale(clip.wav, trade_volume)
        } else {
            Some(clip.wav)
        };
        if let Some(wav) = wav {
            play_bytes(wav);
        } else {
            log::warn!("trade sound skipped: unsupported PCM sample format");
        }
        is_trade
    } else {
        false
    }
}

/// Start one clip after the shared scheduler has released the previous clip's duration.
///
/// With `SND_MEMORY | SND_ASYNC`, WinMM reads the buffer for as long as the sound plays, after
/// this call has returned — so the bytes are kept alive in `CURRENT` until the next call, which
/// stops the previous sound before the slot is overwritten. Embedded sounds were `'static` and
/// never needed this; a user's file is not.
#[cfg(windows)]
fn play_bytes(wav: Arc<[u8]>) {
    use windows::Win32::Media::Audio::{PlaySoundW, SND_ASYNC, SND_MEMORY, SND_NODEFAULT};
    use windows::core::PCWSTR;

    thread_local! {
        /// The buffer WinMM is currently reading. Replaced only after the next `PlaySoundW`, which
        /// stops the sound that was reading it.
        static CURRENT: RefCell<Option<Arc<[u8]>>> = const { RefCell::new(None) };
    }

    unsafe {
        let _ = PlaySoundW(
            PCWSTR(wav.as_ptr() as *const u16),
            None,
            SND_ASYNC | SND_MEMORY | SND_NODEFAULT,
        );
    }
    CURRENT.with(|current| *current.borrow_mut() = Some(wav));
}

/// Play one WAV through AppKit's `NSSound`.
///
/// `NSSound` is used rather than a Rust audio crate because AppKit is already linked: an audio
/// dependency would pull an output-device stack into a build that needs one `play` call.
///
/// Three details this cannot be written without:
///
/// - The sound object must be RETAINED for as long as it plays. `NSSound` does not keep itself
///   alive, so releasing it at the end of this function would cut playback off at its first
///   millisecond — hence the `CURRENT` slot below, which holds exactly one and releases it only
///   when the next sound replaces it. That also reproduces the Windows behaviour, where WinMM
///   plays one sound at a time and a later call interrupts the earlier one.
/// - `dataWithBytesNoCopy:length:freeWhenDone:` with `NO`: the buffer is ours, kept alive in the
///   `CURRENT` slot beside the sound for as long as it plays, so there is nothing to copy and
///   nothing for Foundation to free. `YES` there would hand our allocation to `free()`.
/// - The autorelease pool is explicit because `NSData` comes back autoreleased. Every caller does
///   reach this on the GPUI main thread, which has a pool per run-loop turn, but owning one here
///   means playback does not depend on that staying true.
///
/// `started == NO` is the one spelling that works on both ABIs — `objc` 0.2 types `BOOL` as
/// `c_schar` on x86_64 and as `bool` on aarch64 — so the lint that fires only on the latter is
/// switched off there rather than the comparison being rewritten into something one ABI rejects.
///
/// `unexpected_cfgs` is silenced because `objc` 0.2's `sel_impl!` expands to a `cargo-clippy` cfg
/// that current rustc no longer knows: every `msg_send!` below would otherwise add a warning that
/// says nothing about this code and cannot be fixed from here. `chartdx/metal_backend.rs` carries
/// the same two warnings today for the same reason.
#[cfg(target_os = "macos")]
#[allow(unexpected_cfgs)]
#[cfg_attr(target_arch = "aarch64", allow(clippy::bool_comparison))]
fn play_bytes(wav: Arc<[u8]>) {
    use objc::rc::autoreleasepool;
    use objc::runtime::{BOOL, Class, NO, Object};
    use objc::{msg_send, sel, sel_impl};
    use std::ffi::c_void;

    thread_local! {
        /// The retained `NSSound` currently playing, with the bytes it reads, or `None`.
        /// Thread-local rather than a global: it is a raw Objective-C pointer with no
        /// `Send`/`Sync` story, and every caller is on the one UI thread anyway.
        static CURRENT: RefCell<Option<(*mut Object, Arc<[u8]>)>> = const { RefCell::new(None) };
    }

    // Looked up rather than written as `class!(…)`: that macro panics when a class is missing, and
    // this runs on the feed-drain path, where a panic takes the whole terminal down. A build
    // without AppKit loaded simply stays silent.
    let (Some(data_class), Some(sound_class)) = (Class::get("NSData"), Class::get("NSSound"))
    else {
        return;
    };

    autoreleasepool(|| unsafe {
        // Stop and release the previous sound BEFORE its bytes go out of scope with the tuple.
        if let Some((previous, _bytes)) = CURRENT.with(|current| current.borrow_mut().take()) {
            let _: () = msg_send![previous, stop];
            let _: () = msg_send![previous, release];
        }
        let data: *mut Object = msg_send![
            data_class,
            dataWithBytesNoCopy: wav.as_ptr() as *const c_void
            length: wav.len()
            freeWhenDone: NO
        ];
        if data.is_null() {
            return;
        }
        let sound: *mut Object = msg_send![sound_class, alloc];
        let sound: *mut Object = msg_send![sound, initWithData: data];
        if sound.is_null() {
            return;
        }
        // A format Core Audio cannot decode answers NO rather than raising; release it instead of
        // parking a silent object in CURRENT, where it would swallow the next sound's `stop`.
        let started: BOOL = msg_send![sound, play];
        if started == NO {
            let _: () = msg_send![sound, release];
            return;
        }
        CURRENT.with(|current| *current.borrow_mut() = Some((sound, wav)));
    });
}

#[cfg(not(any(windows, target_os = "macos")))]
fn play_bytes(_wav: Arc<[u8]>) {}

#[cfg(test)]
pub(crate) mod tests;
