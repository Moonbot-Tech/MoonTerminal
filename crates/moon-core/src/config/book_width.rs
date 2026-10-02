//! Shared physical-pixel order-book width defaults and bounds.

/// Default width preserves the original chart appearance.
pub const DEFAULT: f32 = 220.0;
/// Smallest width offered by Settings.
pub const MIN: f32 = 120.0;
/// Largest width offered by Settings; pane layout may reduce it further.
pub const MAX: f32 = 600.0;

/// Return a finite bounded width, using the shipped default for invalid numbers.
pub fn normalize(value: f32) -> f32 {
    if value.is_finite() && value > 0.0 {
        value.clamp(MIN, MAX)
    } else {
        DEFAULT
    }
}

/// Serde default for older settings files without an order-book width.
pub fn default_width() -> f32 {
    DEFAULT
}
