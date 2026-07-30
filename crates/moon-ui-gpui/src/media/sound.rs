//! Alert and detection sound playback. WAV files from `assets/sounds` are embedded in the binary.
//! Windows playback uses WinMM `PlaySoundW` with `SND_MEMORY | SND_ASYNC`, avoiding external
//! dependencies and UI-thread blocking. Playback is a no-op on non-Windows platforms.
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
/// WinMM plays one sound at a time, so a new call interrupts the previous sound as in Moonbot.
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

#[cfg(not(windows))]
fn play_bytes(_wav: &'static [u8]) {}
