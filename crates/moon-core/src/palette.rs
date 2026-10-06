//! Shared dependency-free legacy/default sRGB palette tokens. Configuration defaults use a subset
//! of these values for chart labels, order lines, and the default server color. Runtime UI chrome
//! uses the active MoonUI palette instead of this module.
//!
//! Chart code converts configured sRGB colors to linear values for shaders.

/// `--bg`: panel, toolbar, chart, and order-book background.
pub const BG: [u8; 3] = [0x13, 0x14, 0x16];
/// Subtle chart grid.
pub const GRID: [u8; 3] = [0x17, 0x18, 0x1a];

/// `--text`: primary text.
pub const TEXT: [u8; 3] = [0xe8, 0xe4, 0xdc];
/// `--text-2`: muted text.
pub const TEXT_2: [u8; 3] = [0x97, 0x92, 0x8a];
/// `--text-3`: dimmest text.
pub const TEXT_3: [u8; 3] = [0x5e, 0x5a, 0x53];

/// Approximately `--lift`: idle button background.
pub const LIFT: [u8; 3] = [0x1d, 0x1f, 0x22];

/// `--accent`: amber accent and default server color.
pub const ACCENT: [u8; 3] = [0xff, 0xb3, 0x47];
/// `--long`: green for the order-book bid side and long positions.
pub const GREEN: [u8; 3] = [0x2f, 0xa8, 0x5c];
/// `--sl`: red for stop-loss and short positions.
pub const RED: [u8; 3] = [0xff, 0x4a, 0x4a];
/// `--short`: orange for the order-book ask side.
pub const ORANGE: [u8; 3] = [0xff, 0x8e, 0x5a];
/// `--tp`: light blue for take-profit.
pub const TP: [u8; 3] = [0x7f, 0xc9, 0xff];

/// Rising candle, shared by both chart themes: a clear green that carries on the dark
/// background and on white alike, one step short of a saturated primary.
pub const CANDLE_UP: [u8; 3] = [0x1a, 0x9e, 0x5c];
/// Falling candle, shared by both chart themes: a clear red one step short of pure `#FF0000`.
pub const CANDLE_DOWN: [u8; 3] = [0xe6, 0x3b, 0x3b];
