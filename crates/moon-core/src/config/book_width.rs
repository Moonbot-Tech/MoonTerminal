//! Shared physical-pixel order-book width defaults and bounds.
//!
//! The width is a per-tab chart-graphics setting (`ChartGraphicsCfg::book_width_px`). It used to
//! be one app-wide value in `settings.toml`; [`legacy_global_width`] reads that retired key once,
//! for the startup pass that carries it into the per-tab settings.

/// Default width preserves the original chart appearance.
pub const DEFAULT: f32 = 220.0;
/// Smallest width offered by the chart-graphics popup.
pub const MIN: f32 = 120.0;
/// Largest width offered by the chart-graphics popup; pane layout may reduce it further.
pub const MAX: f32 = 600.0;

/// Return a finite bounded width, using the shipped default for invalid numbers.
pub fn normalize(value: f32) -> f32 {
    if value.is_finite() && value > 0.0 {
        value.clamp(MIN, MAX)
    } else {
        DEFAULT
    }
}

/// Serde default for stored chart graphics without a book width.
pub fn default_width() -> f32 {
    DEFAULT
}

/// What `settings.toml` says about the retired app-wide width.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LegacyWidth {
    /// No file, or a file without a usable number under the key: nothing to carry.
    Absent,
    /// The stored width, normalized the way the old setting drew it.
    Stored(f32),
    /// The file exists but could not be read, or is not TOML. NOT the same as `Absent`: the key
    /// may be in there, so the carry-over leaves the profile untouched rather than commit its
    /// marker on a guess. A read failure (a locked file) gets its carry on a later launch; a file
    /// that is not TOML at all is quarantined by `AppConfig::load` the same launch, and its width
    /// goes with it.
    Unreadable,
}

/// The retired app-wide width as `settings.toml` still holds it.
///
/// Read raw rather than through `SettingsFile`, which no longer declares the key: the file keeps
/// it only until its next save, and the startup pass that consumes it runs before the config is
/// loaded at all.
pub fn legacy_global_width() -> LegacyWidth {
    match std::fs::read_to_string(super::paths::settings_path()) {
        Ok(text) => legacy_global_width_in(&text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => LegacyWidth::Absent,
        Err(_) => LegacyWidth::Unreadable,
    }
}

/// Parse the retired width out of `settings.toml` text; see [`legacy_global_width`].
///
/// Args:
///     text: The whole `settings.toml` document.
///
/// Returns:
///     The normalized width, `Absent` without a number under the key, or `Unreadable` when the
///     text is not TOML at all.
pub fn legacy_global_width_in(text: &str) -> LegacyWidth {
    let Ok(doc) = toml::from_str::<toml::Value>(text) else {
        return LegacyWidth::Unreadable;
    };
    let Some(value) = doc.get("order_book_width_px") else {
        return LegacyWidth::Absent;
    };
    // A width saved by hand as `340` is an integer to TOML, and it meant the same thing.
    match value
        .as_float()
        .or_else(|| value.as_integer().map(|v| v as f64))
    {
        Some(raw) => LegacyWidth::Stored(normalize(raw as f32)),
        None => LegacyWidth::Absent,
    }
}

#[cfg(test)]
mod tests;
