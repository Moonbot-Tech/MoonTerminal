//! Alert and detection sound playback. WAV files from `assets/sounds` are embedded in the binary
//! on every platform, so packaging never has to ship them beside the executable.
//!
//! Each platform plays them with what it already links, so this file adds no audio dependency:
//! Windows uses WinMM `PlaySoundW` with `SND_MEMORY | SND_ASYNC`, macOS uses AppKit `NSSound`
//! over an `NSData` view of the same bytes. Both are asynchronous, and on both a new sound
//! replaces the one already playing rather than mixing with it. Linux remains silent — nothing
//! there is linked that can play audio, and adding one is a dependency decision of its own.
//!
//! A detection or alert strategy selects a sound by file stem; lookup trims whitespace and is
//! ASCII case-insensitive, so names such as `BABYTOY` and `ding1` match their embedded files.

/// Embedded sounds as lowercase stems paired with WAV bytes.
/// `include_bytes!` paths are relative to this file in `crates/moon-ui-gpui/src/media`.
macro_rules! sounds {
    ($($stem:literal => $file:literal),* $(,)?) => {
        pub const SOUNDS: &[(&str, &[u8])] = &[
            $(($stem, include_bytes!(concat!("../../../../assets/sounds/", $file)))),*
        ];
    };
}

sounds! {
    "alarm" => "Alarm.wav",
    "babytoy" => "BABYTOY.wav",
    "bark" => "BARK.WAV",
    "comegetsome" => "ComeGetSome.wav",
    "cork" => "cork.wav",
    "ding1" => "ding1.wav",
    "ding2" => "ding2.wav",
    "error" => "ERROR.wav",
    "fatality" => "Fatality.wav",
    "gold" => "gold.wav",
    "hallo" => "HALLO.wav",
    "letsrock" => "LetsRock.wav",
    "milord" => "milord.wav",
    "pfiff" => "PFIFF.wav",
    "ringin" => "Ringin.wav",
    "ringout" => "ringout.wav",
    "turnon" => "TurnOn.wav",
    "yes_mast" => "YES_MAST.wav",
}

/// Moonbot's own sound list, in the order its settings dropdown shows it, read off that dropdown
/// on 2026-09-02.
///
/// This is the table the PROTOCOL does not carry. `SignalsSettings`'s three sound fields are
/// 1-based ordinals into this list — the wire says "1-based" and nothing more — so the index here
/// is the ordinal MINUS ONE. Without it the settings popup could only show a bare number, which is
/// what `as_alarm_no` still does.
///
/// The order is NOT alphabetical and must not be re-sorted: it is Moonbot's, and the ordinal is
/// stored in the core's own config, so a re-ordering here silently re-points every core's setting
/// at a different sound.
///
/// Labels keep Moonbot's own spelling, mixed case and all, because that is what the user picks from
/// there; lowercasing one gives the stem in [`SOUNDS`], which is what [`play`] matches on. The
/// sibling test pins both halves — every label resolves to an embedded sound, and the two lists
/// hold the same set.
pub const MB_SOUNDS: &[&str] = &[
    "Alarm",
    "BABYTOY",
    "BARK",
    "cork",
    "ERROR",
    "HALLO",
    "PFIFF",
    "Ringin",
    "ringout",
    "TurnOn",
    "YES_MAST",
    "ding1",
    "ding2",
    "Fatality",
    "gold",
    "milord",
    "LetsRock",
    "ComeGetSome",
];

/// Moonbot's label for a 1-based sound ordinal, or `None` when the core holds one this list has no
/// entry for.
///
/// `None` rather than a fallback to the first sound: a core carrying an ordinal we cannot name is a
/// core whose sound list differs from ours, and showing "Alarm" for it would write that back on the
/// next OK and silently change the user's setting.
pub fn mb_sound_name(ordinal: i32) -> Option<&'static str> {
    usize::try_from(ordinal.checked_sub(1)?)
        .ok()
        .and_then(|i| MB_SOUNDS.get(i).copied())
}

/// Play the sound a 1-based Moonbot ordinal names, doing nothing when it names none.
pub fn play_ordinal(ordinal: i32) {
    if let Some(name) = mb_sound_name(ordinal) {
        play(name);
    }
}

/// Return sound stems for the sound-selection dropdowns (the Alerts window and the Core Status
/// alert popup).
pub fn names() -> impl Iterator<Item = &'static str> {
    SOUNDS.iter().map(|(n, _)| *n)
}

/// Find embedded WAV bytes by a trimmed, ASCII case-insensitive stem.
fn bytes_of(name: &str) -> Option<&'static [u8]> {
    let name = name.trim().to_ascii_lowercase();
    SOUNDS.iter().find(|(n, _)| *n == name).map(|(_, b)| *b)
}

/// Play a named sound asynchronously, doing nothing when the stem is unknown.
/// Each backend plays one sound at a time, so a new call interrupts the previous one as in Moonbot.
pub fn play(name: &str) {
    let Some(wav) = bytes_of(name) else {
        return;
    };
    play_bytes(wav);
}

#[cfg(windows)]
fn play_bytes(wav: &'static [u8]) {
    use windows::Win32::Media::Audio::{PlaySoundW, SND_ASYNC, SND_MEMORY, SND_NODEFAULT};
    use windows::core::PCWSTR;
    // With SND_MEMORY, `pszSound` points directly into the WAV bytes. The embedded buffer is
    // `'static`, so it remains valid for the entire asynchronous playback operation.
    unsafe {
        let _ = PlaySoundW(
            PCWSTR(wav.as_ptr() as *const u16),
            None,
            SND_ASYNC | SND_MEMORY | SND_NODEFAULT,
        );
    }
}

/// Play one embedded WAV through AppKit's `NSSound`.
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
/// - `dataWithBytesNoCopy:length:freeWhenDone:` with `NO`: the buffer is `'static` embedded data,
///   so there is nothing to copy and nothing for Foundation to free. `YES` there would hand a
///   pointer into our own binary image to `free()`.
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
fn play_bytes(wav: &'static [u8]) {
    use objc::rc::autoreleasepool;
    use objc::runtime::{BOOL, Class, NO, Object};
    use objc::{msg_send, sel, sel_impl};
    use std::cell::Cell;
    use std::ffi::c_void;

    thread_local! {
        /// The retained `NSSound` currently playing, or null. Thread-local rather than a global:
        /// it is a raw Objective-C pointer with no `Send`/`Sync` story, and every caller is on the
        /// one UI thread anyway.
        static CURRENT: Cell<*mut Object> = const { Cell::new(std::ptr::null_mut()) };
    }

    // Looked up rather than written as `class!(…)`: that macro panics when a class is missing, and
    // this runs on the feed-drain path, where a panic takes the whole terminal down. A build
    // without AppKit loaded simply stays silent.
    let (Some(data_class), Some(sound_class)) = (Class::get("NSData"), Class::get("NSSound"))
    else {
        return;
    };

    autoreleasepool(|| unsafe {
        let previous = CURRENT.with(|current| current.replace(std::ptr::null_mut()));
        if !previous.is_null() {
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
        CURRENT.with(|current| current.set(sound));
    });
}

#[cfg(not(any(windows, target_os = "macos")))]
fn play_bytes(_wav: &'static [u8]) {}

#[cfg(test)]
mod tests;
