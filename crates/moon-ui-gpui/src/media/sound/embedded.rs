//! The sounds this binary ships with: Moonbot's own eighteen, embedded so packaging never has to
//! carry them beside the executable. They are the floor of the catalog — a user's folder can add
//! to them or replace one by name, never remove one.

/// Embedded sounds as lowercase stems paired with WAV bytes, in MOONBOT'S ORDER (see
/// [`MB_SOUNDS`]). `include_bytes!` paths are relative to this file.
macro_rules! sounds {
    ($($stem:literal => $file:literal),* $(,)?) => {
        pub(super) const SOUNDS: &[(&str, &[u8])] = &[
            $(($stem, include_bytes!(concat!("../../../../../assets/sounds/", $file)))),*
        ];
    };
}

sounds! {
    "alarm" => "Alarm.wav",
    "babytoy" => "BABYTOY.wav",
    "bark" => "BARK.WAV",
    "cork" => "cork.wav",
    "error" => "ERROR.wav",
    "hallo" => "HALLO.wav",
    "pfiff" => "PFIFF.wav",
    "ringin" => "Ringin.wav",
    "ringout" => "ringout.wav",
    "turnon" => "TurnOn.wav",
    "yes_mast" => "YES_MAST.wav",
    "ding1" => "ding1.wav",
    "ding2" => "ding2.wav",
    "fatality" => "Fatality.wav",
    "gold" => "gold.wav",
    "milord" => "milord.wav",
    "letsrock" => "LetsRock.wav",
    "comegetsome" => "ComeGetSome.wav",
}

/// Moonbot's own sound list, in the order its settings dropdown shows it, read off that dropdown
/// on 2026-09-02.
///
/// This is the table the PROTOCOL does not carry. The core's sound fields (`signal_sound_2`,
/// `buy_signal_sound`, `signal_sound`) are 1-based ordinals into this list — the wire says
/// "1-based" and nothing more — so the index here is the ordinal MINUS ONE. A user's own file
/// holds a number only when its name claims one (`19_MySound.wav`; see the catalog); the core
/// never plays a sound itself, so a number only ever means what THIS terminal's table says it
/// means.
///
/// The order is NOT alphabetical and must not be re-sorted: it is Moonbot's, and the ordinal is
/// stored in the core's own config, so a re-ordering here silently re-points every core's setting
/// at a different sound.
///
/// Labels keep Moonbot's own spelling, mixed case and all, because that is what the user picks from
/// there; lowercasing one gives the stem in [`SOUNDS`]. The sibling test pins both halves — every
/// label resolves to an embedded sound, and the two lists hold the same set in the same order.
pub(super) const MB_SOUNDS: &[&str] = &[
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

/// What plays when a chosen sound cannot be found: a strategy naming a file the folder lacks, a
/// core ordinal past the archive, a trade sound whose file was deleted. Embedded, so it always
/// exists; short and neutral, so it reads as "something fired" rather than as any one alert.
pub const DEFAULT_SOUND: &str = "ding1";
