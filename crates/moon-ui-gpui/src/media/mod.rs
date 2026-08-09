//! Embedded-asset loaders: coin, exchange and UI icon textures, and sound playback.

use std::path::PathBuf;

pub(crate) mod coin_icons;
pub(crate) mod exchange_logos;
pub(crate) mod icons;
pub(crate) mod sound;

/// Locate one `assets/<name>` directory: under the working directory first, then beside the
/// executable.
///
/// Every asset loader here ships its set embedded with `include_dir` and lets a file on disk take
/// priority, so a deployed installation can add or replace one without a rebuild. This is that
/// override rule, in one place — it used to be copied verbatim into three loaders, which meant the
/// beside-the-executable fallback had three definitions that could drift apart.
///
/// Args:
///     name: Directory name under `assets`, such as `"coins"`.
///
/// Returns:
///     The first existing directory, or the relative path when neither exists — callers treat a
///     failed read as "no override" and fall back to the embedded copy.
pub(crate) fn asset_dir(name: &str) -> PathBuf {
    let relative = PathBuf::from("assets").join(name);
    if relative.is_dir() {
        return relative;
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        let beside_exe = dir.join("assets").join(name);
        if beside_exe.is_dir() {
            return beside_exe;
        }
    }
    relative
}
