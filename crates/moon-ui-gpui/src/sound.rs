//! Проигрывание звуков алертов/детектов. Wav-файлы вшиты в бинарь (assets/sounds),
//! воспроизведение на Windows через winmm `PlaySoundW` (SND_MEMORY|SND_ASYNC) —
//! без внешних зависимостей и без блокировки UI-потока. На не-Windows — no-op.
//!
//! Звук у детекта/алерта задаётся полем стратегии (см. `feed::strategies`): имя
//! совпадает со стемом файла (BABYTOY, ding1, …), регистр не важен.

/// Вшитые звуки: (стем в нижнем регистре, байты wav). `include_bytes!` — путь от
/// этого файла: crates/moon-ui-gpui/src → ../../../assets/sounds.
macro_rules! sounds {
    ($($stem:literal => $file:literal),* $(,)?) => {
        pub const SOUNDS: &[(&str, &[u8])] = &[
            $(($stem, include_bytes!(concat!("../../../assets/sounds/", $file)))),*
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

/// Имена звуков (стемы) для выпадашки «Выбор звука».
#[allow(dead_code)] // задействуется при добавлении выбора звука в окно «Алерты»
pub fn names() -> impl Iterator<Item = &'static str> {
    SOUNDS.iter().map(|(n, _)| *n)
}

/// Байты звука по имени (регистр не важен).
fn bytes_of(name: &str) -> Option<&'static [u8]> {
    let name = name.trim().to_ascii_lowercase();
    SOUNDS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, b)| *b)
}

/// Проиграть звук по имени (не блокирует; один звук за раз — новый прерывает
/// предыдущий, как в MoonBot). Неизвестное имя → ничего.
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
    // SND_MEMORY: pszSound — указатель на wav в памяти. Байты 'static, живут всю
    // сессию, поэтому async-воспроизведение безопасно (буфер не освободится).
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
