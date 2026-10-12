//! Shared chrome of the Summary profit chart: axis labels, core colours, hover fences and the
//! bucket popup.

/// Summary chart axis helpers.
mod axis;
/// Summary chart hover helpers.
mod hover;
/// Summary chart popup helpers.
mod popup;

pub(super) use axis::{bucket_label, distinct_core_colors, dm, fallback_core_color};
pub(in crate::analytics) use hover::PopupHover;
pub(super) use hover::{PopupKey, chart_hover};
pub(super) use popup::{core_color, popup_core_row, popup_overlay, popup_shell, profit_trades};
#[cfg(test)]
use popup::{popup_limits, popup_outer_width};

use super::{fmt_signed, sign_color};

use std::time::Duration;

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    MoonPalette, MoonScrollAxis, MoonScrollbarVisibility, h_flex,
    moon_scrollbar_overlay_with_palette, v_flex,
};

use super::super::AnalyticsView;
use crate::design;
use crate::design::moon;

#[cfg(test)]
mod tests;
