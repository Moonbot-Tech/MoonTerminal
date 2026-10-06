//! Moonbot terminal design tokens extracted from the Moonbot Terminal design reference.
//!
//! This is a thin GPUI-div adapter over MoonPalette tokens. Keep it visual-only:
//! no terminal logic, no chart renderer state.
//!
//! The terminal renders ONE design at ONE size system: ordinary controls on [`CONTROL_TIER`],
//! dense strips pinned to `MoonSize::Xs`, text at [`BODY_TEXT`] plus a local step. The user scales
//! that design as a whole through the window content zoom (`MoonScale::zoom`, installed by
//! `startup::moon_theme_config_for_presentation`), which multiplies every pixel a window draws.
//! MoonUI's own token multipliers stay at 1.0, so [`ui_px`] is an identity adapter kept for the
//! components that read the tokens, and nothing here asks which size the user picked.

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_core::util::fmt::DeltaSign;
use moon_ui::{
    MoonButtonSize, MoonButtonVariant, MoonInputSize, MoonMetrics, MoonPalette, MoonSize,
    MoonTabStrip, MoonTableStyle, MoonTextMetrics, MoonTheme, MoonTone, rgba_from,
};
use std::collections::HashMap;
use std::sync::{Arc, LazyLock};

const M: MoonMetrics = MoonMetrics::TERMINAL;

mod chrome;
mod color;
mod fit;
mod logo;
mod measure;
mod rules;
mod scale;
mod table;
mod window;

pub use chrome::*;
pub use color::*;
pub use fit::*;
pub use logo::*;
pub use measure::*;
pub use rules::*;
pub use scale::*;
pub use table::*;
pub use window::*;

/// The MoonTerminal lockup, one cut per colour scheme.
///
/// Each file already carries its own wordmark colour — `#E7E7E7` in the dark cut, `#17202A` in the
/// light one, with the blue "Terminal" shared — so the terminal PICKS a file instead of patching a
/// fill the way the old single-cut Moonbot asset needed. A custom palette therefore no longer
/// recolours the brand, which is the point of a brand asset.
const LOGO_SVG_DARK: &str = include_str!("../../../assets/brand/moonterminal-logo-blue-dark.svg");
const LOGO_SVG_LIGHT: &str = include_str!("../../../assets/brand/moonterminal-logo-blue-light.svg");
const LOGO_SRC_W: f32 = 283.23;
const LOGO_SRC_H: f32 = 43.0;

/// The two points the inverse is fitted through, chosen clear of the one-pixel floor `font` applies.
const PROBE_LOW: f32 = 10.0;
const PROBE_HIGH: f32 = 20.0;

#[cfg(test)]
mod tests;
