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
        let _ = cx.update(|cx| {
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

/// Queue a named sound without interrupting the current clip. The application's coordination
/// timer pumps this lane, including Settings previews when no market events arrive. A name no
/// file answers to plays the default and is reported once.
pub fn play(name: &str) {
    if let Some(clip) = resolve(MissingSound::Name(name.to_string())) {
        PLAYBACK.with(|player| player.borrow_mut().enqueue(clip));
    }
}

/// Play the sound a 1-based Moonbot ordinal names; one past the table plays the default and is
/// reported once.
pub fn play_ordinal(ordinal: i32) {
    if let Some(clip) = resolve(MissingSound::Ordinal(ordinal)) {
        PLAYBACK.with(|player| player.borrow_mut().enqueue(clip));
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

/// The normal lane retains existing producer selection rules; trades have a separate bounded
/// backend lane so a burst of detects cannot occupy every trade slot. Alternate when both wait.
#[derive(Default)]
struct Playback {
    normal: std::collections::VecDeque<Clip>,
    busy_until: Option<std::time::Instant>,
    trade_turn: bool,
}

impl Playback {
    /// Preserve accepted FIFO entries on overflow; never evict a clip already waiting to play.
    fn enqueue(&mut self, clip: Clip) {
        if self.normal.len() < 64 {
            self.normal.push_back(clip);
        } else {
            log::warn!("notification sound queue full; newest ordinary sound omitted");
        }
    }

    /// Called by the application timer even without feed traffic. The small output-device
    /// allowance prevents timer granularity from starting the next clip before its last frame.
    fn next(&mut self, now: std::time::Instant, trade: Option<Clip>) -> Option<(Clip, bool)> {
        if self.busy_until.is_some_and(|until| now < until) {
            return None;
        }
        let is_trade = trade.is_some() && (self.trade_turn || self.normal.is_empty());
        let clip = if is_trade {
            trade?
        } else {
            self.normal.pop_front()?
        };
        self.trade_turn = !is_trade;
        self.busy_until = Some(now + clip.duration + std::time::Duration::from_millis(50));
        Some((clip, is_trade))
    }
}

thread_local! {
    /// All playback callers run on the GPUI thread; no lock or sleeping thread is required.
    static PLAYBACK: RefCell<Playback> = RefCell::new(Playback::default());
}

/// Spend pre-sleep ordinary backlog once at the quiet transition. Later producer-authorized
/// quiet exceptions may enqueue normally, and the current asynchronous clip is left to finish.
pub(crate) fn discard_pending() {
    PLAYBACK.with(|player| player.borrow_mut().normal.clear());
}

/// Offer the trade-lane head and advance one fair, noninterrupting playback turn. A trade sound
/// whose file is gone plays the default and is reported, like every other lane.
/// Returns true only when the trade was selected, so contention never consumes its edge.
pub(crate) fn pump(trade: Option<&str>) -> bool {
    let trade = trade.and_then(|name| resolve(MissingSound::Name(name.to_string())));
    let next = PLAYBACK.with(|player| player.borrow_mut().next(std::time::Instant::now(), trade));
    if let Some((clip, is_trade)) = next {
        play_bytes(clip.wav);
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
mod tests;
